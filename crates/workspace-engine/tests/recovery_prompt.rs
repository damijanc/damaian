//! What the recovery prompt says, per
//! `docs/specs/45_crash_recovery_prompt.md` §5.2.
//!
//! Requirement 1 is that the prompt names the *specific* in-flight action and
//! never degrades to a generic "session interrupted". The sentence is built in
//! the engine rather than in `app.js` precisely so it can be asserted here: the
//! desktop shell has no JS test suite, so a sentence assembled in the webview
//! could not be checked against a session log at all.

use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};
use workspace_engine::{
    AuditLog, SecretScanner, SessionStore, Task, TaskStatus, classify_session, headline,
    resume_blocked_reason,
};

static COUNTER: AtomicU64 = AtomicU64::new(1);

struct Fixture {
    store: SessionStore,
    audit: AuditLog,
    session_id: String,
}

fn fixture(name: &str) -> Fixture {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock should work")
        .as_nanos();
    let data_dir: PathBuf = std::env::temp_dir().join(format!(
        "damaian-recovery-prompt-{name}-{now}-{}",
        COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir_all(&data_dir).expect("temp dir should be created");
    let store = SessionStore::new(&data_dir);
    let audit = AuditLog::new(&data_dir, true, SecretScanner::default());
    let session = store.create_session("repo_1", "Recovery").unwrap();
    Fixture {
        store,
        audit,
        session_id: session.id,
    }
}

fn task_left_in(
    fixture: &Fixture,
    prompt: &str,
    status: TaskStatus,
    dangling: Option<(&str, &str, bool)>,
) -> Task {
    let task = fixture
        .store
        .create_task(&fixture.session_id, prompt, "mock", "m")
        .unwrap();
    let task = fixture
        .store
        .update_task_status(&task, status, None)
        .unwrap();
    if let Some((action, reference, side_effecting)) = dangling {
        // Started and never finished: the signature of the process dying here.
        let _marker = fixture
            .store
            .start_action(&task, action, reference, side_effecting)
            .unwrap();
    }
    task
}

/// Requirement 1 for the `unknown_external_outcome` classification.
#[test]
fn the_headline_names_a_patch_application_whose_outcome_is_unknown() {
    let fixture = fixture("headline-patch");
    task_left_in(
        &fixture,
        "fix the parser",
        TaskStatus::ApplyingPatch,
        Some(("apply_patch", "patch_7", true)),
    );

    let recovered = classify_session(&fixture.store, &fixture.audit, &fixture.session_id).unwrap();

    assert_eq!(
        headline(&recovered[0]),
        "A patch application was in progress and its outcome is unknown"
    );
}

/// Requirement 1 for the `interrupted` classification. A read-only action is
/// named just as specifically — the classification changes the clause about the
/// outcome, not whether the action is named.
#[test]
fn the_headline_names_an_interrupted_read() {
    let fixture = fixture("headline-read");
    task_left_in(
        &fixture,
        "explain the indexer",
        TaskStatus::PreparingContext,
        Some(("read_file", "src/lib.rs", false)),
    );

    let recovered = classify_session(&fixture.store, &fixture.audit, &fixture.session_id).unwrap();

    assert_eq!(
        headline(&recovered[0]),
        "Reading a file was interrupted before it finished"
    );
}

/// A crash with no marker on disk has no action to name. The sentence then says
/// what *is* known — the state the task was left in — rather than falling back
/// to the generic "session interrupted" requirement 1 forbids.
#[test]
fn a_task_with_no_dangling_action_is_described_by_the_state_it_was_left_in() {
    let fixture = fixture("headline-nomarker");
    task_left_in(
        &fixture,
        "summarise the diff",
        TaskStatus::WaitingForModel,
        None,
    );

    let recovered = classify_session(&fixture.store, &fixture.audit, &fixture.session_id).unwrap();

    assert_eq!(
        headline(&recovered[0]),
        "This turn stopped while waiting for the model"
    );
    assert_ne!(headline(&recovered[0]), "Session interrupted");
}

/// An action name this version does not know is printed rather than swallowed.
/// A later version can add an action without silently turning its recovery
/// prompt generic.
#[test]
fn an_unrecognised_action_is_named_verbatim() {
    let fixture = fixture("headline-unknown-action");
    task_left_in(
        &fixture,
        "do the new thing",
        TaskStatus::RunningTool,
        Some(("teleport_files", "somewhere", true)),
    );

    let recovered = classify_session(&fixture.store, &fixture.audit, &fixture.session_id).unwrap();

    assert_eq!(
        headline(&recovered[0]),
        "teleport_files was in progress and its outcome is unknown"
    );
}

/// Requirement 2: `Resume` is absent for an unknown outcome, and the absence is
/// explained. The reason is engine-side because the engine is what decided it.
#[test]
fn an_unknown_outcome_states_why_resume_is_not_offered() {
    let fixture = fixture("blocked-reason");
    task_left_in(
        &fixture,
        "run the migration",
        TaskStatus::RunningTool,
        Some(("run_command", "psql -f migrate.sql", true)),
    );

    let recovered = classify_session(&fixture.store, &fixture.audit, &fixture.session_id).unwrap();

    let reason =
        resume_blocked_reason(&recovered[0]).expect("resume is blocked, so it has a reason");
    assert_eq!(
        reason,
        "A command was running when Damaian stopped, and there is no way to tell \
         whether it finished. Damaian will not run it again on its own."
    );
}

/// The counterpart: an interrupted task has no reason, because nothing is
/// blocked. A UI that renders the reason whenever it is present then cannot
/// state a blockage that does not exist.
#[test]
fn an_interrupted_task_has_no_blocked_reason() {
    let fixture = fixture("no-blocked-reason");
    task_left_in(
        &fixture,
        "explain this file",
        TaskStatus::PreparingContext,
        Some(("read_file", "src/lib.rs", false)),
    );

    let recovered = classify_session(&fixture.store, &fixture.audit, &fixture.session_id).unwrap();

    assert!(resume_blocked_reason(&recovered[0]).is_none());
}

/// §5.4: `Resume` re-runs the turn, which needs the prompt the user typed. It
/// is in the log but `read_task_statuses` returns statuses only.
#[test]
fn read_tasks_returns_the_prompt_a_resume_has_to_re_send() {
    let fixture = fixture("read-tasks");
    let task = task_left_in(
        &fixture,
        "rename the widget",
        TaskStatus::WaitingForModel,
        None,
    );

    let tasks = fixture.store.read_tasks(&fixture.session_id).unwrap();

    let found = tasks
        .iter()
        .find(|candidate| candidate.id == task.id)
        .expect("the task should be replayed from the log");
    assert_eq!(found.user_prompt, "rename the widget");
    assert_eq!(found.status, TaskStatus::WaitingForModel);
    assert_eq!(found.model_name, "m");
}

/// The one that caught a real bug. A recovery operation writes a status event
/// whose `Task` is a skeleton — only `id` and `session_id` are load-bearing,
/// because tasks are replayed rather than stored — so a reader that substitutes
/// the later record wholesale loses the prompt. Which is the prompt a resume
/// exists to re-send, so it broke exactly the case it was added for.
#[test]
fn a_status_event_without_metadata_does_not_erase_the_prompt() {
    let fixture = fixture("read-tasks-skeleton");
    let task = task_left_in(
        &fixture,
        "rename the widget",
        TaskStatus::PreparingContext,
        None,
    );
    let recovered = classify_session(&fixture.store, &fixture.audit, &fixture.session_id).unwrap();
    // Writes `task_status_updated` with an empty prompt, provider and model.
    workspace_engine::resume(&fixture.store, &fixture.audit, &recovered[0]).unwrap();

    let tasks = fixture.store.read_tasks(&fixture.session_id).unwrap();

    let found = tasks
        .iter()
        .find(|candidate| candidate.id == task.id)
        .expect("the task should still be readable");
    assert_eq!(found.user_prompt, "rename the widget");
    assert_eq!(found.model_name, "m");
    assert_eq!(found.status, TaskStatus::PreparingContext);
}

/// The status must come from the *latest* event, not from `task_created`, or a
/// resumed task would still look like the state it crashed in.
#[test]
fn read_tasks_reports_the_latest_status() {
    let fixture = fixture("read-tasks-latest");
    let task = task_left_in(&fixture, "tidy up", TaskStatus::PreparingContext, None);
    fixture
        .store
        .update_task_status(&task, TaskStatus::Complete, None)
        .unwrap();

    let tasks = fixture.store.read_tasks(&fixture.session_id).unwrap();

    assert_eq!(
        tasks
            .iter()
            .find(|candidate| candidate.id == task.id)
            .map(|candidate| candidate.status.clone()),
        Some(TaskStatus::Complete)
    );
}
