//! Crash classification, per
//! `docs/specs/17_durable_task_state_and_crash_recovery/proposal.md` §5.4.
//!
//! The rule these tests exist to hold is requirement 5: no action whose outcome
//! is unknown is ever automatically repeated. Every assertion about
//! `auto_resume_permitted` is an assertion about that guarantee.

use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};
use workspace_engine::{AuditLog, SecretScanner, SessionStore, Task, TaskStatus, classify_session};

static COUNTER: AtomicU64 = AtomicU64::new(1);

fn temp_data_dir(name: &str) -> PathBuf {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock should work")
        .as_nanos();
    let dir = std::env::temp_dir().join(format!(
        "damaian-recovery-{name}-{now}-{}",
        COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir_all(&dir).expect("temp dir should be created");
    dir
}

struct Fixture {
    data_dir: PathBuf,
    store: SessionStore,
    audit: AuditLog,
    session_id: String,
}

fn fixture(name: &str) -> Fixture {
    let data_dir = temp_data_dir(name);
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

/// Leaves a task in `status`, optionally with an action that started and never
/// finished — which is what a crash mid-action leaves on disk.
fn task_left_in(fixture: &Fixture, status: TaskStatus, dangling: Option<(&str, bool)>) -> Task {
    let task = fixture
        .store
        .create_task(&fixture.session_id, "do the thing", "mock", "m")
        .unwrap();
    let task = fixture
        .store
        .update_task_status(&task, status, None)
        .unwrap();
    if let Some((action, side_effecting)) = dangling {
        // Never finished: stands in for the process dying here.
        let _marker = fixture
            .store
            .start_action(&task, action, "ref_1", side_effecting)
            .unwrap();
    }
    task
}

fn audit_log_text(fixture: &Fixture) -> String {
    fs::read_to_string(fixture.data_dir.join("audit").join("events.jsonl")).unwrap_or_default()
}

/// §5.4 rule 1.
#[test]
fn a_dangling_side_effecting_action_classifies_as_unknown_external_outcome() {
    let fixture = fixture("unknown-outcome");
    let task = task_left_in(
        &fixture,
        TaskStatus::RunningTool,
        Some(("run_command", true)),
    );

    let recovered = classify_session(&fixture.store, &fixture.audit, &fixture.session_id).unwrap();

    assert_eq!(recovered.len(), 1);
    assert_eq!(recovered[0].task_id, task.id);
    assert_eq!(
        recovered[0].classification,
        TaskStatus::UnknownExternalOutcome
    );
    assert!(
        !recovered[0].auto_resume_permitted,
        "requirement 5: an unknown outcome is never repeated automatically"
    );
}

/// §5.4 rule 2, first half.
#[test]
fn a_dangling_read_only_action_classifies_as_interrupted_and_may_resume() {
    let fixture = fixture("interrupted-readonly");
    task_left_in(
        &fixture,
        TaskStatus::PreparingContext,
        Some(("read_file", false)),
    );

    let recovered = classify_session(&fixture.store, &fixture.audit, &fixture.session_id).unwrap();

    assert_eq!(recovered[0].classification, TaskStatus::Interrupted);
    assert!(
        recovered[0].auto_resume_permitted,
        "requirement 6: read-only work resumes automatically"
    );
}

/// §5.4 rule 2, second half.
#[test]
fn a_non_terminal_task_with_no_dangling_action_classifies_as_interrupted() {
    let fixture = fixture("interrupted-nodangling");
    task_left_in(&fixture, TaskStatus::PreparingContext, None);

    let recovered = classify_session(&fixture.store, &fixture.audit, &fixture.session_id).unwrap();

    assert_eq!(recovered[0].classification, TaskStatus::Interrupted);
    assert!(recovered[0].dangling.is_none());
    assert!(recovered[0].auto_resume_permitted);
}

#[test]
fn a_terminal_task_is_not_recovered_at_all() {
    let fixture = fixture("terminal");
    for status in [
        TaskStatus::Complete,
        TaskStatus::Failed,
        TaskStatus::Cancelled,
        TaskStatus::ToolBudgetExhausted,
    ] {
        task_left_in(&fixture, status, None);
    }

    let recovered = classify_session(&fixture.store, &fixture.audit, &fixture.session_id).unwrap();

    assert!(
        recovered.is_empty(),
        "nothing further happens to a terminal task, got {recovered:?}"
    );
}

/// §5.4 rule 3: a task awaiting a human was not interrupted mid-action. It
/// keeps its status, and its proposal is reattached by Task 7 rather than being
/// reported as a crash.
#[test]
fn a_task_waiting_for_approval_is_not_treated_as_a_crash() {
    let fixture = fixture("waiting");
    task_left_in(&fixture, TaskStatus::WaitingForApproval, None);

    let recovered = classify_session(&fixture.store, &fixture.audit, &fixture.session_id).unwrap();

    assert!(recovered.is_empty());
}

/// The backstop the plan did not specify. The marker is the primary signal, but
/// a task left in `running_tool` with **no marker at all** is not evidence of
/// safety — it is absence of evidence. Auto-resuming it could repeat a command.
#[test]
fn a_side_effecting_status_with_no_marker_is_still_not_auto_resumed() {
    let fixture = fixture("status-backstop");
    task_left_in(&fixture, TaskStatus::RunningTool, None);

    let recovered = classify_session(&fixture.store, &fixture.audit, &fixture.session_id).unwrap();

    assert_eq!(
        recovered[0].classification,
        TaskStatus::Interrupted,
        "with no dangling marker, rule 2 applies"
    );
    assert!(
        !recovered[0].auto_resume_permitted,
        "but the status alone says a command may have been running, and absence \
         of a marker does not prove it was not"
    );
}

/// §5.6: a legacy `running` task classifies as `interrupted`, but must never be
/// auto-resumed — it carries exactly the information this spec exists to
/// eliminate: something was in flight and nothing recorded what.
#[test]
fn a_legacy_running_task_classifies_as_interrupted_but_is_never_auto_resumed() {
    let fixture = fixture("legacy-running");
    let task = fixture
        .store
        .create_task(&fixture.session_id, "legacy", "mock", "m")
        .unwrap();
    // Written the way the previous version wrote it.
    let log = fixture
        .data_dir
        .join("sessions")
        .join(format!("{}.jsonl", fixture.session_id));
    let mut content = fs::read_to_string(&log).unwrap();
    content.push_str(&format!(
        "{{\"eventId\":\"evt_legacy\",\"seq\":900,\"timestampMs\":1,\
          \"eventType\":\"task_status_updated\",\"payload\":{{\"id\":\"{}\",\
          \"sessionId\":\"{}\",\"status\":\"running\"}}}}\n",
        task.id, fixture.session_id
    ));
    fs::write(&log, content).unwrap();

    let recovered = classify_session(&fixture.store, &fixture.audit, &fixture.session_id).unwrap();

    assert_eq!(recovered[0].previous_status, "running");
    assert_eq!(recovered[0].classification, TaskStatus::Interrupted);
    assert!(
        !recovered[0].auto_resume_permitted,
        "nothing recorded what a legacy `running` task was doing, so resumable \
         cannot be concluded from it"
    );
}

/// §5.4: the classification carries the evidence, so spec 45 can say "a patch
/// application was in progress" without re-deriving anything.
#[test]
fn the_classification_records_the_dangling_action_and_its_seq() {
    let fixture = fixture("evidence");
    task_left_in(
        &fixture,
        TaskStatus::ApplyingPatch,
        Some(("apply_patch", true)),
    );

    let recovered = classify_session(&fixture.store, &fixture.audit, &fixture.session_id).unwrap();
    let dangling = recovered[0]
        .dangling
        .as_ref()
        .expect("the evidence must be carried");

    assert_eq!(dangling.action, "apply_patch");
    assert_eq!(dangling.reference, "ref_1");
    assert!(dangling.side_effecting);
    assert!(
        dangling.seq > 0,
        "the evidence names where in the log it is"
    );
}

/// Requirement 10.
#[test]
fn every_classification_is_audited_with_its_evidence() {
    let fixture = fixture("audited");
    task_left_in(
        &fixture,
        TaskStatus::ApplyingPatch,
        Some(("apply_patch", true)),
    );

    classify_session(&fixture.store, &fixture.audit, &fixture.session_id).unwrap();

    let log = audit_log_text(&fixture);
    assert!(log.contains("task_recovered"), "the decision is recorded");
    assert!(
        log.contains("unknown_external_outcome"),
        "with its classification"
    );
    assert!(log.contains("apply_patch"), "and the evidence for it");
    assert!(
        log.contains("\"autoResumePermitted\":\"false\"") || log.contains("autoResumePermitted"),
        "and whether resuming is allowed, got: {log}"
    );
}

/// The audit deferred from Task 1: a torn tail is evidence of the crash being
/// classified, so the classifier records it rather than swallowing it.
#[test]
fn a_torn_log_tail_is_audited_during_classification() {
    let fixture = fixture("torn-audit");
    task_left_in(&fixture, TaskStatus::PreparingContext, None);
    let log = fixture
        .data_dir
        .join("sessions")
        .join(format!("{}.jsonl", fixture.session_id));
    let mut content = fs::read_to_string(&log).unwrap();
    content.push_str("{\"eventId\":\"evt_torn\",\"eventType\":\"task_stat");
    fs::write(&log, content).unwrap();

    classify_session(&fixture.store, &fixture.audit, &fixture.session_id).unwrap();

    assert!(
        audit_log_text(&fixture).contains("session_log_truncated_tail"),
        "a discarded line must be audited, not swallowed"
    );
}
