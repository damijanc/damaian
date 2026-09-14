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
    AuditLog, CommandStore, PatchStore, PausedTurns, PendingApprovalRef, PlanStep,
    ReattachedApproval, SecretScanner, SessionStore, StepStatus, Task, TaskPlan, TaskStatus,
    classify_session, headline, reattach_pending_approvals, resume_blocked_reason,
};

static COUNTER: AtomicU64 = AtomicU64::new(1);

struct Fixture {
    data_dir: PathBuf,
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
        data_dir,
        store,
        audit,
        session_id: session.id,
    }
}

/// A task paused for plan review, with the plan in the log and — unless
/// `paused_turn_survives` is false — the paused turn on disk where
/// `ChatOrchestrator` writes it.
fn task_awaiting_plan_review(
    fixture: &Fixture,
    proposal_id: &str,
    paused_turn_survives: bool,
) -> Task {
    let task = fixture
        .store
        .create_task(&fixture.session_id, "add retry to the client", "mock", "m")
        .unwrap();
    let mut plan = TaskPlan::new(&task.id, 1);
    plan.steps.push(PlanStep {
        id: "step_1".to_string(),
        title: "Add a retry helper".to_string(),
        detail: None,
        status: StepStatus::Pending,
        depends_on: Vec::new(),
        started_at_ms: None,
        completed_at_ms: None,
        evidence: Vec::new(),
    });
    fixture.store.create_plan(&task, &plan).unwrap();
    if paused_turn_survives {
        let pending = fixture.data_dir.join("chat").join("pending");
        fs::create_dir_all(&pending).unwrap();
        // The on-disk shape `PendingCommandStore` writes. Written by hand
        // because `PendingChatTurn` is private, which is the point: `PausedTurns`
        // reads this file as loose JSON, so the field names are the contract.
        fs::write(
            pending.join(format!("{proposal_id}.json")),
            r#"{"proposal_id":"x","plan_review":{"deferred_action":"apply a patch to client.rs"}}"#,
        )
        .unwrap();
    }
    fixture
        .store
        .await_approval(
            &task,
            &PendingApprovalRef {
                kind: "plan".to_string(),
                proposal_id: proposal_id.to_string(),
            },
        )
        .unwrap();
    task
}

fn reattach(fixture: &Fixture) -> Vec<ReattachedApproval> {
    reattach_pending_approvals(
        &fixture.store,
        &fixture.audit,
        &CommandStore::new(&fixture.data_dir),
        &PatchStore::new(&fixture.data_dir),
        &PausedTurns::new(&fixture.data_dir),
        &fixture.session_id,
    )
    .unwrap()
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

/// Spec 21's markers, which the fallback was printing raw as "propose_plan was
/// interrupted before it finished". The fallback is for actions this version
/// has never heard of, not for ones it ships alongside.
#[test]
fn the_headline_names_spec_21s_plan_actions() {
    let planning_fixture = fixture("headline-plan-actions");
    task_left_in(
        &planning_fixture,
        "add retry",
        TaskStatus::PreparingContext,
        Some(("propose_plan", "plan_1", false)),
    );
    let other = fixture("headline-step-actions");
    task_left_in(
        &other,
        "add retry",
        TaskStatus::RunningTool,
        Some(("complete_step", "step_2", false)),
    );

    let planning = classify_session(
        &planning_fixture.store,
        &planning_fixture.audit,
        &planning_fixture.session_id,
    )
    .unwrap();
    let stepping = classify_session(&other.store, &other.audit, &other.session_id).unwrap();

    assert_eq!(
        headline(&planning[0]),
        "Drawing up a plan was interrupted before it finished"
    );
    assert_eq!(
        headline(&stepping[0]),
        "Finishing a plan step was interrupted before it finished"
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

/// Spec 21 added a third kind of pending approval, and this surface did not
/// know it: a crash during a plan review failed the task with "unknown pending
/// approval kind plan" and destroyed a turn that was in fact recoverable, since
/// its paused turn is on disk the whole time.
#[test]
fn a_plan_review_interrupted_by_a_crash_is_re_presented() {
    let fixture = fixture("plan-review");
    let task = task_awaiting_plan_review(&fixture, "planprop_1", true);

    let reattached = reattach(&fixture);

    match &reattached[0] {
        ReattachedApproval::Plan {
            task_id,
            proposal_id,
            plan,
            deferred_action,
        } => {
            assert_eq!(task_id, &task.id);
            assert_eq!(proposal_id, "planprop_1");
            assert_eq!(plan.steps[0].title, "Add a retry helper");
            assert_eq!(deferred_action, "apply a patch to client.rs");
        }
        other => panic!("a plan review should be reattached, got {other:?}"),
    }
    // Still the user's to answer. Failing it was the bug.
    assert_eq!(
        fixture
            .store
            .read_task_statuses(&fixture.session_id)
            .unwrap()
            .get(&task.id)
            .map(String::as_str),
        Some("waiting_for_approval")
    );
}

/// The half that keeps §5.5's rule: a plan whose paused turn is gone cannot be
/// resumed, so presenting it would give the user a card whose buttons have
/// nothing to continue. That fails the task, with the reason said out loud.
#[test]
fn a_plan_review_whose_paused_turn_is_gone_fails_the_task() {
    let fixture = fixture("plan-review-orphan");
    let task = task_awaiting_plan_review(&fixture, "planprop_2", false);

    let reattached = reattach(&fixture);

    match &reattached[0] {
        ReattachedApproval::Unavailable { task_id, reason } => {
            assert_eq!(task_id, &task.id);
            assert!(
                reason.contains("paused turn behind plan review planprop_2 is gone"),
                "got {reason}"
            );
        }
        other => panic!("an unresumable review should be unavailable, got {other:?}"),
    }
    assert_eq!(
        fixture
            .store
            .read_task_statuses(&fixture.session_id)
            .unwrap()
            .get(&task.id)
            .map(String::as_str),
        Some("failed")
    );
}
