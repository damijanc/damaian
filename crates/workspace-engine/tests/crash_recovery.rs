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

/// §5.5: a pending approval survives restart with the proposal it refers to.
#[test]
fn a_pending_approval_survives_restart_with_its_proposal() {
    let fixture = fixture("reattach-ok");
    let commands = workspace_engine::CommandStore::new(&fixture.data_dir);
    let patches = workspace_engine::PatchStore::new(&fixture.data_dir);

    let patch = workspace_engine::ProposedPatch {
        id: "patch_pending".to_string(),
        session_id: fixture.session_id.clone(),
        task_id: Some("task_x".to_string()),
        summary: "waiting".to_string(),
        status: "pending".to_string(),
        created_at_ms: 1,
        files: Vec::new(),
    };
    patches.save(&patch).unwrap();

    let task = fixture
        .store
        .create_task(&fixture.session_id, "edit", "mock", "m")
        .unwrap();
    fixture
        .store
        .await_approval(
            &task,
            &workspace_engine::PendingApprovalRef {
                kind: "patch".to_string(),
                proposal_id: patch.id.clone(),
            },
        )
        .unwrap();

    // A fresh store, as after a restart.
    let restarted = SessionStore::new(&fixture.data_dir);
    let reattached = workspace_engine::reattach_pending_approvals(
        &restarted,
        &fixture.audit,
        &commands,
        &patches,
        &fixture.session_id,
    )
    .unwrap();

    assert_eq!(reattached.len(), 1);
    match &reattached[0] {
        workspace_engine::ReattachedApproval::Patch { task_id, patch } => {
            assert_eq!(task_id, &task.id);
            assert_eq!(patch.id, "patch_pending");
        }
        other => panic!("expected the patch to be reattached, got {other:?}"),
    }
}

/// §5.5: never an approval card reconstructed from partial data — the user
/// would be approving a command Damaian is guessing at. The task fails instead.
#[test]
fn a_missing_proposal_file_fails_the_task_with_a_reason() {
    let fixture = fixture("reattach-missing");
    let commands = workspace_engine::CommandStore::new(&fixture.data_dir);
    let patches = workspace_engine::PatchStore::new(&fixture.data_dir);

    let task = fixture
        .store
        .create_task(&fixture.session_id, "edit", "mock", "m")
        .unwrap();
    fixture
        .store
        .await_approval(
            &task,
            &workspace_engine::PendingApprovalRef {
                kind: "patch".to_string(),
                proposal_id: "patch_vanished".to_string(),
            },
        )
        .unwrap();

    let reattached = workspace_engine::reattach_pending_approvals(
        &fixture.store,
        &fixture.audit,
        &commands,
        &patches,
        &fixture.session_id,
    )
    .unwrap();

    match &reattached[0] {
        workspace_engine::ReattachedApproval::Unavailable { task_id, reason } => {
            assert_eq!(task_id, &task.id);
            assert!(
                reason.contains("patch_vanished"),
                "the reason must name what is missing, got: {reason}"
            );
        }
        other => panic!("expected Unavailable, got {other:?}"),
    }

    assert_eq!(
        fixture
            .store
            .read_task_statuses(&fixture.session_id)
            .unwrap()
            .get(&task.id)
            .map(String::as_str),
        Some("failed"),
        "the task must be failed, not left waiting on something that is gone"
    );
    assert!(audit_log_text(&fixture).contains("pending_approval_unavailable"));
}

/// A corrupt file must fail the same way as a missing one: the alternative is
/// presenting a card built from whatever happened to parse.
#[test]
fn a_corrupt_proposal_file_fails_the_task_rather_than_reconstructing_a_card() {
    let fixture = fixture("reattach-corrupt");
    let commands = workspace_engine::CommandStore::new(&fixture.data_dir);
    let patches = workspace_engine::PatchStore::new(&fixture.data_dir);

    let pending = fixture.data_dir.join("patches").join("pending");
    fs::create_dir_all(&pending).unwrap();
    fs::write(
        pending.join("patch_corrupt.dpatch"),
        "DAMAIAN_STORED_PATCH_V2\nPATCH_ID 7\ntrunca",
    )
    .unwrap();

    let task = fixture
        .store
        .create_task(&fixture.session_id, "edit", "mock", "m")
        .unwrap();
    fixture
        .store
        .await_approval(
            &task,
            &workspace_engine::PendingApprovalRef {
                kind: "patch".to_string(),
                proposal_id: "patch_corrupt".to_string(),
            },
        )
        .unwrap();

    let reattached = workspace_engine::reattach_pending_approvals(
        &fixture.store,
        &fixture.audit,
        &commands,
        &patches,
        &fixture.session_id,
    )
    .unwrap();

    assert!(
        matches!(
            reattached[0],
            workspace_engine::ReattachedApproval::Unavailable { .. }
        ),
        "a corrupt proposal must not become a card, got {:?}",
        reattached[0]
    );
}

/// A task whose approval was recorded by a version that did not store the link
/// has nothing to reattach and nothing to guess from, so it fails too.
#[test]
fn a_task_awaiting_approval_with_no_recorded_link_fails() {
    let fixture = fixture("reattach-nolink");
    let commands = workspace_engine::CommandStore::new(&fixture.data_dir);
    let patches = workspace_engine::PatchStore::new(&fixture.data_dir);
    task_left_in(&fixture, TaskStatus::WaitingForApproval, None);

    let reattached = workspace_engine::reattach_pending_approvals(
        &fixture.store,
        &fixture.audit,
        &commands,
        &patches,
        &fixture.session_id,
    )
    .unwrap();

    match &reattached[0] {
        workspace_engine::ReattachedApproval::Unavailable { reason, .. } => {
            assert!(reason.contains("no pending proposal"), "got: {reason}");
        }
        other => panic!("expected Unavailable, got {other:?}"),
    }
}

fn classify_one(fixture: &Fixture) -> workspace_engine::RecoveredTask {
    classify_session(&fixture.store, &fixture.audit, &fixture.session_id)
        .unwrap()
        .into_iter()
        .next()
        .expect("one recovered task")
}

fn status_of(fixture: &Fixture, task_id: &str) -> Option<String> {
    fixture
        .store
        .read_task_statuses(&fixture.session_id)
        .unwrap()
        .get(task_id)
        .cloned()
}

/// Requirement 5's enforcement point. Spec 45 will present this from a webview,
/// and a guarantee that lives only in a webview is not a guarantee — so the
/// refusal is here, where a caller cannot widen it.
#[test]
fn resume_is_refused_for_a_task_whose_outcome_is_unknown() {
    let fixture = fixture("resume-refused");
    task_left_in(
        &fixture,
        TaskStatus::ApplyingPatch,
        Some(("apply_patch", true)),
    );
    let recovered = classify_one(&fixture);
    assert_eq!(recovered.classification, TaskStatus::UnknownExternalOutcome);

    let error = workspace_engine::resume(&fixture.store, &fixture.audit, &recovered)
        .expect_err("resuming an unknown-outcome task must be refused");

    let text = format!("{error:?}");
    assert!(
        text.contains("apply_patch"),
        "the refusal must name the action, got: {text}"
    );
    assert!(text.contains("unknown"), "and say why, got: {text}");
    assert!(
        audit_log_text(&fixture).contains("\"outcome\":\"refused\""),
        "the refusal itself is auditable"
    );
}

#[test]
fn resume_is_permitted_for_an_interrupted_read_only_task() {
    let fixture = fixture("resume-ok");
    let task = task_left_in(
        &fixture,
        TaskStatus::PreparingContext,
        Some(("read_file", false)),
    );
    let recovered = classify_one(&fixture);

    workspace_engine::resume(&fixture.store, &fixture.audit, &recovered)
        .expect("read-only work may resume");

    assert_eq!(
        status_of(&fixture, &task.id).as_deref(),
        Some("preparing_context")
    );
}

/// `auto_resume_permitted` and "a human may choose Resume" are different
/// questions, and this is the case that separates them: a legacy `running` task
/// is never resumed *automatically*, because nothing recorded what it was
/// doing — but requirement 5 forbids repeating an unknown outcome
/// automatically, and an informed human decision is not automatic.
#[test]
fn a_human_may_resume_a_task_that_auto_resume_declined() {
    let fixture = fixture("resume-human");
    task_left_in(&fixture, TaskStatus::RunningTool, None);
    let recovered = classify_one(&fixture);

    assert!(
        !recovered.auto_resume_permitted,
        "no marker means Damaian must not continue on its own"
    );
    assert!(
        workspace_engine::resume_allowed(&recovered),
        "but the classification is `interrupted`, so a human may still choose it"
    );
    workspace_engine::resume(&fixture.store, &fixture.audit, &recovered).expect("allowed");
}

#[test]
fn mark_failed_is_terminal_and_notes_the_unknown_outcome() {
    let fixture = fixture("mark-failed");
    let task = task_left_in(
        &fixture,
        TaskStatus::RunningTool,
        Some(("run_command", true)),
    );
    let recovered = classify_one(&fixture);

    workspace_engine::mark_failed(&fixture.store, &fixture.audit, &recovered).unwrap();

    assert_eq!(status_of(&fixture, &task.id).as_deref(), Some("failed"));
    assert!(
        TaskStatus::parse("failed").unwrap().is_terminal(),
        "and `failed` is terminal, so the classifier will not pick it up again"
    );
    let log = audit_log_text(&fixture);
    assert!(log.contains("mark_failed"));
    assert!(
        log.contains("outcome is unknown"),
        "the note must say the outcome was unknown, got: {log}"
    );
}

#[test]
fn abandon_is_terminal_and_does_not_retry_the_turn() {
    let fixture = fixture("abandon");
    let task = task_left_in(
        &fixture,
        TaskStatus::ApplyingPatch,
        Some(("apply_patch", true)),
    );
    let recovered = classify_one(&fixture);

    workspace_engine::abandon(&fixture.store, &fixture.audit, &recovered).unwrap();

    assert_eq!(status_of(&fixture, &task.id).as_deref(), Some("cancelled"));
    // Re-classifying finds nothing: the task is closed, not queued for a retry.
    let again = classify_session(&fixture.store, &fixture.audit, &fixture.session_id).unwrap();
    assert!(again.is_empty(), "an abandoned task is not picked up again");
}

/// Requirement 10.
#[test]
fn every_recovery_decision_is_audited_with_its_evidence() {
    let fixture = fixture("decision-audit");
    task_left_in(
        &fixture,
        TaskStatus::ApplyingPatch,
        Some(("apply_patch", true)),
    );
    let recovered = classify_one(&fixture);

    let _ = workspace_engine::resume(&fixture.store, &fixture.audit, &recovered);
    workspace_engine::mark_failed(&fixture.store, &fixture.audit, &recovered).unwrap();

    let log = audit_log_text(&fixture);
    assert!(log.contains("task_recovery_decision"));
    assert!(
        log.contains("unknown_external_outcome"),
        "the classification"
    );
    assert!(log.contains("apply_patch"), "and the evidence it rested on");
}
