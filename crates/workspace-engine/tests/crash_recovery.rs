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

/// The legacy fixture, whose shape was verified against 30 real session logs
/// from the released version before it was written: no `seq` on any line (777
/// of 828 real events lack it), `task_status_updated` payloads carrying the
/// flattened task fields, and one event using the `{"task":…,"error":…}` wrapper
/// that an error path produces.
fn legacy_session(fixture: &Fixture) -> &'static str {
    let sessions = fixture.data_dir.join("sessions");
    fs::create_dir_all(&sessions).unwrap();
    fs::copy(
        concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/legacy_session.jsonl"
        ),
        sessions.join("session_legacy_fixture.jsonl"),
    )
    .unwrap();
    "session_legacy_fixture"
}

/// Requirement 10: a session written by the current released version loads
/// after the upgrade with no data loss.
#[test]
fn a_session_written_before_this_change_loads_with_no_data_loss() {
    let fixture = fixture("legacy-load");
    let session_id = legacy_session(&fixture);

    let messages = fixture.store.read_messages(session_id).unwrap();
    assert_eq!(messages.len(), 2, "both messages must survive");
    assert_eq!(messages[0].content, "explain the upload client");
    assert_eq!(messages[1].content, "It lives in src/upload.rs.");

    let statuses = fixture.store.read_task_statuses(session_id).unwrap();
    assert_eq!(statuses.len(), 4, "all four tasks must replay");
    assert_eq!(
        statuses.get("task_legacy_done").map(String::as_str),
        Some("complete")
    );
    assert_eq!(
        statuses.get("task_legacy_running").map(String::as_str),
        Some("running")
    );
    assert_eq!(
        statuses.get("task_legacy_waiting").map(String::as_str),
        Some("waiting_for_approval")
    );
    // The wrapped `{"task":…,"error":…}` form must read the same as the flat one.
    assert_eq!(
        statuses.get("task_legacy_errored").map(String::as_str),
        Some("failed")
    );

    // Events with no `seq` are numbered by line order, so the newest is 11.
    assert_eq!(fixture.store.latest_event_seq(session_id).unwrap(), 11);
    assert_eq!(
        fixture.store.unreadable_event_count(session_id).unwrap(),
        0,
        "a legacy log is fully readable, not partially discarded"
    );
}

/// §5.6: a legacy `running` task classifies as `interrupted`.
#[test]
fn a_legacy_session_classifies_its_running_task_as_interrupted() {
    let fixture = fixture("legacy-classify");
    let session_id = legacy_session(&fixture);

    let recovered = classify_session(&fixture.store, &fixture.audit, session_id).unwrap();

    // `complete` and `failed` are terminal; `waiting_for_approval` is rule 3's
    // job, not a crash. Only the `running` task is recovered.
    assert_eq!(
        recovered
            .iter()
            .map(|task| task.task_id.as_str())
            .collect::<Vec<_>>(),
        vec!["task_legacy_running"]
    );
    assert_eq!(recovered[0].classification, TaskStatus::Interrupted);
    assert_eq!(recovered[0].previous_status, "running");
    assert!(
        !recovered[0].auto_resume_permitted,
        "nothing recorded what it was doing"
    );
}

/// The upgrade consequence, asserted rather than left to be discovered: a task
/// left awaiting approval by the previous version has no recorded link, so it
/// is failed with a reason. The proposal files themselves are untouched.
#[test]
fn a_legacy_task_awaiting_approval_is_failed_because_no_link_was_recorded() {
    let fixture = fixture("legacy-approval");
    let session_id = legacy_session(&fixture);
    let commands = workspace_engine::CommandStore::new(&fixture.data_dir);
    let patches = workspace_engine::PatchStore::new(&fixture.data_dir);

    let reattached = workspace_engine::reattach_pending_approvals(
        &fixture.store,
        &fixture.audit,
        &commands,
        &patches,
        session_id,
    )
    .unwrap();

    assert_eq!(reattached.len(), 1);
    match &reattached[0] {
        workspace_engine::ReattachedApproval::Unavailable { task_id, reason } => {
            assert_eq!(task_id, "task_legacy_waiting");
            assert!(reason.contains("no pending proposal"), "got: {reason}");
        }
        other => panic!("expected Unavailable, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// The kill matrix (§7)
//
// §7 calls this the load-bearing test and the one most likely to be quietly
// reduced to "a few representative states". Every state is crossed with every
// shape a crash can leave on disk, and every cell has a written-down answer.
// ---------------------------------------------------------------------------

/// What the crash left behind alongside the status. `NoMarker` is a crash
/// *between* actions; the other two are a crash *inside* one, told apart by
/// whether the action could be observed from outside this process.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CrashShape {
    NoMarker,
    ReadOnlyAction,
    SideEffectingAction,
}

impl CrashShape {
    fn all() -> [Self; 3] {
        [
            Self::NoMarker,
            Self::ReadOnlyAction,
            Self::SideEffectingAction,
        ]
    }

    fn marker(self) -> Option<(&'static str, bool)> {
        match self {
            Self::NoMarker => None,
            Self::ReadOnlyAction => Some(("read_file", false)),
            Self::SideEffectingAction => Some(("run_command", true)),
        }
    }
}

/// The recovery outcome for one cell.
#[derive(Debug, PartialEq, Eq)]
enum Expected {
    /// Absent from the recovery list, because there is nothing to recover.
    NotRecovered,
    Recovered {
        classification: TaskStatus,
        /// May Damaian continue on its own?
        auto_resume_permitted: bool,
        /// May the user, shown the evidence, choose to continue?
        human_resume_allowed: bool,
    },
}

fn resumable() -> Expected {
    Expected::Recovered {
        classification: TaskStatus::Interrupted,
        auto_resume_permitted: true,
        human_resume_allowed: true,
    }
}

/// Interrupted, so a human may continue it, but not without being asked.
fn needs_a_decision() -> Expected {
    Expected::Recovered {
        classification: TaskStatus::Interrupted,
        auto_resume_permitted: false,
        human_resume_allowed: true,
    }
}

/// Requirement 5: nothing may repeat this, not even a human choosing to.
fn unknown_outcome() -> Expected {
    Expected::Recovered {
        classification: TaskStatus::UnknownExternalOutcome,
        auto_resume_permitted: false,
        human_resume_allowed: false,
    }
}

/// The matrix, as one expectation per cell.
///
/// This `match` is deliberately exhaustive rather than a lookup with a default:
/// a `TaskStatus` variant added later does not compile until someone writes
/// down what recovery should do with it. That is half of what stops the matrix
/// shrinking; `TaskStatus::all()` in the driver is the other half.
fn expected(status: &TaskStatus, shape: CrashShape) -> Expected {
    let in_a_side_effecting_action = shape == CrashShape::SideEffectingAction;
    match status {
        // Nothing further happens to a terminal task, whatever the log holds.
        // A dangling marker under a terminal status means the crash landed
        // between the action and the status write, and the status won.
        TaskStatus::Complete
        | TaskStatus::Failed
        | TaskStatus::Cancelled
        | TaskStatus::ToolBudgetExhausted => Expected::NotRecovered,

        // §5.4 rule 3: a task awaiting a human was not interrupted mid-action.
        // Its proposal is reattached (§5.5) instead.
        TaskStatus::WaitingForApproval => Expected::NotRecovered,

        // §5.1: a crash in these states leaves nothing half-done, so read-only
        // work resumes on its own. A billed-but-unanswered model call counts as
        // resumable — the cost of a second call, not an unrepeatable effect.
        TaskStatus::Created
        | TaskStatus::PreparingContext
        | TaskStatus::WaitingForModel
        | TaskStatus::Interrupted => {
            if in_a_side_effecting_action {
                unknown_outcome()
            } else {
                resumable()
            }
        }

        // §5.1's three "never auto-retry" states. With a side-effecting marker
        // the outcome is unknown. Without one, rule 2 still classifies the task
        // `interrupted` — but the status alone says a command may have been
        // running, and a missing marker is absence of evidence, not evidence of
        // safety, so it is never resumed unasked.
        TaskStatus::RunningTool | TaskStatus::ApplyingPatch | TaskStatus::Validating => {
            if in_a_side_effecting_action {
                unknown_outcome()
            } else {
                needs_a_decision()
            }
        }

        // Not written during normal operation — the classifier's own output,
        // which only reaches disk if something persists it. Should that happen,
        // a status saying the outcome is unknown must not re-derive as
        // resumable just because its marker is out of reach.
        TaskStatus::UnknownExternalOutcome => {
            if in_a_side_effecting_action {
                unknown_outcome()
            } else {
                needs_a_decision()
            }
        }
    }
}

fn observed(recovered: &[workspace_engine::RecoveredTask], task_id: &str) -> Expected {
    match recovered.iter().find(|task| task.task_id == task_id) {
        None => Expected::NotRecovered,
        Some(task) => Expected::Recovered {
            classification: task.classification.clone(),
            auto_resume_permitted: task.auto_resume_permitted,
            human_resume_allowed: workspace_engine::resume_allowed(task),
        },
    }
}

/// Every state, crossed with every crash shape.
///
/// Two rows cannot be produced from `TaskStatus::all()` and are covered
/// separately rather than dropped: the legacy `running` string, by
/// `a_legacy_running_task_classifies_as_interrupted_but_is_never_auto_resumed`
/// (only `NoMarker` is reachable for it — action markers did not exist in the
/// version that wrote `running`), and an unrecognised status from a future
/// version, by `a_status_this_version_does_not_understand_is_never_resumed`.
#[test]
fn every_state_and_crash_shape_recovers_the_way_the_matrix_says() {
    let mut cells = 0;
    for status in TaskStatus::all() {
        for shape in CrashShape::all() {
            let fixture = fixture("matrix");
            let task = task_left_in(&fixture, status.clone(), shape.marker());

            let recovered =
                classify_session(&fixture.store, &fixture.audit, &fixture.session_id).unwrap();

            assert_eq!(
                observed(&recovered, &task.id),
                expected(&status, shape),
                "state `{}` after a crash with {shape:?}",
                status.as_str()
            );
            cells += 1;
        }
    }
    // Thirteen states by three crash shapes. If this number moves, a state or
    // a shape was added or removed — check the matrix is still complete before
    // updating it.
    assert_eq!(cells, 39, "the matrix must not shrink");
}

/// The row `TaskStatus::all()` cannot reach: a status written by a *later*
/// version, read by this one. Guessing is least safe exactly here, so an
/// unreadable status is treated as non-terminal and never resumed unasked.
#[test]
fn a_status_this_version_does_not_understand_is_never_resumed() {
    let fixture = fixture("future-status");
    let task = fixture
        .store
        .create_task(&fixture.session_id, "from the future", "mock", "m")
        .unwrap();
    let log = fixture
        .data_dir
        .join("sessions")
        .join(format!("{}.jsonl", fixture.session_id));
    let mut content = fs::read_to_string(&log).unwrap();
    content.push_str(&format!(
        "{{\"eventId\":\"evt_future\",\"seq\":900,\"timestampMs\":1,\
          \"eventType\":\"task_status_updated\",\"payload\":{{\"id\":\"{}\",\
          \"sessionId\":\"{}\",\"status\":\"negotiating_with_the_compiler\"}}}}\n",
        task.id, fixture.session_id
    ));
    fs::write(&log, content).unwrap();

    let recovered = classify_session(&fixture.store, &fixture.audit, &fixture.session_id).unwrap();

    assert_eq!(recovered.len(), 1);
    assert_eq!(
        recovered[0].previous_status,
        "negotiating_with_the_compiler"
    );
    assert_eq!(
        recovered[0].classification,
        TaskStatus::Interrupted,
        "not terminal: a status this version cannot read is not assumed finished"
    );
    assert!(
        !recovered[0].auto_resume_permitted,
        "and not assumed safe either"
    );
}

/// The one row of the matrix a constructed log cannot prove: that the session
/// log on disk is still fully parsable after a **real** `SIGKILL` mid-action.
/// Every other test in this file assumes that and then reasons about
/// classification; this one checks the assumption.
///
/// `#[ignore]`d per `AGENTS.md` because it spawns and kills a real process. Run
/// it by hand:
///
/// ```sh
/// cargo test -p workspace-engine --test crash_recovery -- --ignored --exact \
///   a_real_sigkill_mid_action_leaves_a_readable_log_and_an_unknown_outcome
/// ```
#[test]
#[ignore]
fn a_real_sigkill_mid_action_leaves_a_readable_log_and_an_unknown_outcome() {
    use std::os::unix::process::ExitStatusExt;
    use std::process::Command;
    use std::time::{Duration, Instant};

    let fixture = fixture("sigkill");
    // The child is this same test binary, re-executed into the helper below so
    // it writes through the very `SessionStore` under test.
    let mut child = Command::new(std::env::current_exe().expect("test binary path"))
        .args([
            "--ignored",
            "--exact",
            "sigkill_helper_starts_a_side_effecting_action_and_waits_to_be_killed",
        ])
        .env("DAMAIAN_SIGKILL_DIR", &fixture.data_dir)
        .env("DAMAIAN_SIGKILL_SESSION", &fixture.session_id)
        .spawn()
        .expect("child should spawn");

    // Wait for the marker to actually reach disk, rather than sleeping a guessed
    // interval — the point is to kill the child *inside* the action.
    let log = fixture
        .data_dir
        .join("sessions")
        .join(format!("{}.jsonl", fixture.session_id));
    let deadline = Instant::now() + Duration::from_secs(30);
    while !fs::read_to_string(&log)
        .unwrap_or_default()
        .contains("action_started")
    {
        assert!(
            Instant::now() < deadline,
            "the child never reached the action"
        );
        std::thread::sleep(Duration::from_millis(20));
    }

    // By PID, through the handle we own. Never by name: the user's real app
    // shares binary names with test processes on this machine.
    child.kill().expect("SIGKILL should be delivered");
    let status = child.wait().expect("child should be reaped");
    assert_eq!(
        status.signal(),
        Some(9),
        "the child must have died by SIGKILL rather than exiting on its own, \
         or this proves nothing about a crash"
    );

    assert_eq!(
        fixture
            .store
            .unreadable_event_count(&fixture.session_id)
            .unwrap(),
        0,
        "an append-only log must survive a kill with every line parsable"
    );

    let recovered = classify_session(&fixture.store, &fixture.audit, &fixture.session_id).unwrap();

    assert_eq!(recovered.len(), 1, "got {recovered:?}");
    assert_eq!(
        recovered[0].classification,
        TaskStatus::UnknownExternalOutcome
    );
    assert!(!recovered[0].auto_resume_permitted);
    assert_eq!(
        recovered[0].dangling.as_ref().expect("the marker").action,
        "run_command",
        "the evidence must survive the kill, not just the status"
    );
}

/// Not a test — the child half of the `SIGKILL` test above, which re-executes
/// this binary into it. It never returns on its own; it waits to be killed.
///
/// `#[ignore]`d so the normal suite never runs it, and it returns immediately
/// unless the parent's environment is present, so a bare `-- --ignored` run
/// does not hang.
#[test]
#[ignore]
fn sigkill_helper_starts_a_side_effecting_action_and_waits_to_be_killed() {
    let (Ok(data_dir), Ok(session_id)) = (
        std::env::var("DAMAIAN_SIGKILL_DIR"),
        std::env::var("DAMAIAN_SIGKILL_SESSION"),
    ) else {
        return;
    };

    let store = SessionStore::new(&data_dir);
    let task = store
        .create_task(&session_id, "get killed", "mock", "m")
        .unwrap();
    let task = store
        .update_task_status(&task, TaskStatus::RunningTool, None)
        .unwrap();
    // Never finished. `ActionMarker` has no `Drop` impl by design, so letting
    // it fall out of scope does not close it either — which is the whole point:
    // only an explicit `finish_action` closes an action.
    let _marker = store
        .start_action(&task, "run_command", "cmd_1", true)
        .unwrap();

    loop {
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
}
