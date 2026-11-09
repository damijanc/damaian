//! The completion gate's model and durable record, per
//! `docs/specs/101_closed_task_completion.md`.
//!
//! The reason matrix is tested without a session: it is a property of the
//! durable facts, so a failure here should not need a turn to reproduce. The
//! service, storage and repository checks use a throwaway checkout and log.

use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use workspace_engine::hash::{file_hash, sha256};
use workspace_engine::plan::StepStatus;
use workspace_engine::{
    CheckId, CheckOutcome, ClientError, CommandPurpose, CompletionFacts, CompletionGateReport,
    CompletionGateService, CompletionReasonCode, CompletionSettlement, Evidence,
    PLANNED_CHECK_VERSION, PendingApprovalRef, PendingDecision, PendingQuestionRef, PlanStep,
    PlannedCheck, Session, SessionStore, SkipReason, Task, TaskPlan, TaskStatus, VerificationEvent,
    VerificationOutcome, VerificationState, fold_verification, repository_id_for_root,
};

static COUNTER: AtomicU64 = AtomicU64::new(1);

fn temp_dir(name: &str) -> PathBuf {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock should work")
        .as_nanos();
    let dir = std::env::temp_dir().join(format!(
        "damaian-completion-{name}-{now}-{}",
        COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir_all(&dir).expect("temp dir should be created");
    dir
}

fn step(id: &str, status: StepStatus) -> PlanStep {
    PlanStep {
        id: id.to_string(),
        title: format!("step {id}"),
        detail: None,
        status,
        depends_on: Vec::new(),
        started_at_ms: None,
        completed_at_ms: None,
        evidence: Vec::new(),
        check: None,
    }
}

fn plan(task_id: &str, steps: Vec<PlanStep>) -> TaskPlan {
    let mut plan = TaskPlan::new(task_id, 0);
    plan.steps = steps;
    plan
}

fn check(directory: &str, command: &str) -> CheckId {
    CheckId::new(directory, command)
}

/// A plan step carrying a `required: true` structured check at the root.
fn checked_step(id: &str, command: &str, status: StepStatus) -> PlanStep {
    checked_step_in(id, command, "", status)
}

/// A plan step carrying a `required: true` structured check in a directory.
fn checked_step_in(id: &str, command: &str, directory: &str, status: StepStatus) -> PlanStep {
    let mut step = step(id, status);
    step.check = Some(PlannedCheck {
        version: PLANNED_CHECK_VERSION,
        command: command.to_string(),
        working_directory: directory.to_string(),
        purpose: "validates the final tree".to_string(),
        required: true,
    });
    step
}

/// The `CommandExit` a deferred required check leaves on its plan step when the
/// worker runs the exact command as an ordinary command.
fn command_exit(command: &str, exit_code: Option<i32>) -> Evidence {
    Evidence::CommandExit {
        marker_id: "marker".to_string(),
        exit_code,
        purpose: CommandPurpose::Check,
        command_hash: Some(sha256(command)),
        working_directory: String::new(),
    }
}

/// The `PatchApplied` a real apply appends to the step that was open when it
/// landed. Its file list does not matter to the gate, only that a mutation was
/// recorded.
fn patch_applied() -> Evidence {
    Evidence::PatchApplied {
        marker_id: "apply_marker".to_string(),
        files: Vec::new(),
    }
}

/// A `CommandExit` as a session log written before the working directory was
/// recorded: the JSON has no directory field at all. Loaded through serde so
/// the compatibility of an old log is what the test exercises.
fn legacy_command_exit(command: &str, exit_code: Option<i32>) -> Evidence {
    serde_json::from_value(serde_json::json!({
        "kind": "commandExit",
        "markerId": "marker",
        "exitCode": exit_code,
        "purpose": "check",
        "commandHash": sha256(command),
    }))
    .expect("a pre-directory command exit should load")
}

fn due(task_id: &str) -> VerificationEvent {
    VerificationEvent::Due {
        task_id: task_id.to_string(),
        patch_id: "patch".to_string(),
        paths: Vec::new(),
        checks: Vec::new(),
    }
}

fn started(task_id: &str, checks: &[CheckId]) -> VerificationEvent {
    VerificationEvent::Started {
        task_id: task_id.to_string(),
        attempt: 0,
        checks: checks.to_vec(),
    }
}

fn ran(
    task_id: &str,
    check: CheckId,
    exit_code: Option<i32>,
    hashes: &[(&str, &str)],
    generation: u32,
) -> VerificationEvent {
    VerificationEvent::Check {
        task_id: task_id.to_string(),
        check,
        outcome: CheckOutcome::Ran {
            exit_code,
            marker_id: "marker".to_string(),
            termination: None,
        },
        file_hashes: hashes
            .iter()
            .map(|(path, hash)| ((*path).to_string(), (*hash).to_string()))
            .collect(),
        generation,
    }
}

fn skipped(
    task_id: &str,
    check: CheckId,
    reason: SkipReason,
    generation: u32,
) -> VerificationEvent {
    VerificationEvent::Check {
        task_id: task_id.to_string(),
        check,
        outcome: CheckOutcome::Skipped(reason),
        file_hashes: BTreeMap::new(),
        generation,
    }
}

fn finished(task_id: &str, outcome: VerificationOutcome) -> VerificationEvent {
    VerificationEvent::Finished {
        task_id: task_id.to_string(),
        outcome,
    }
}

fn state(events: &[VerificationEvent]) -> VerificationState {
    fold_verification(events)
}

fn evaluate(
    task_id: &str,
    plan: Option<&TaskPlan>,
    verification: &VerificationState,
    pending: Option<&PendingDecision>,
    current_hashes: &BTreeMap<String, String>,
) -> CompletionGateReport {
    CompletionGateReport::evaluate(CompletionFacts {
        task_id,
        plan,
        verification,
        pending,
        current_hashes,
        floor_checks: &[],
    })
}

fn fixture(name: &str) -> (PathBuf, SessionStore, Session, Task) {
    let root = temp_dir(name);
    let store = SessionStore::new(root.join(".damaian"));
    let session = store
        .create_session(&repository_id_for_root(&root), "Closed task")
        .unwrap();
    let task = store
        .create_task(&session.id, "do the work", "mock", "mock-model")
        .unwrap();
    (root, store, session, task)
}

#[test]
fn every_blocking_fact_produces_its_stable_reason_code() {
    let task_id = "task_1";
    let passing = check("", "cargo test");
    let failing = check("crates/engine", "cargo test");
    let missing = check("packages/web", "npm test");
    let blocked = check("", "cargo clippy");
    let declined = check("", "cargo fmt --check");
    let stale = check("", "cargo deny check");

    // The required checks come from the plan's structured checks, not from the
    // verification selection. Six terminal primary steps sit at generation 5,
    // so every recorded run names that generation to stay current; the stale
    // check is stale by file hash rather than by generation.
    let plan = plan(
        task_id,
        vec![
            step("implement", StepStatus::Pending),
            checked_step_in(
                "failing",
                "cargo test",
                "crates/engine",
                StepStatus::Completed,
            ),
            checked_step_in("missing", "npm test", "packages/web", StepStatus::Completed),
            checked_step("blocked", "cargo clippy", StepStatus::Completed),
            checked_step("declined", "cargo fmt --check", StepStatus::Completed),
            checked_step("stale", "cargo deny check", StepStatus::Completed),
            checked_step("passing", "cargo test", StepStatus::Completed),
        ],
    );
    let verification = state(&[
        started(
            task_id,
            &[
                passing.clone(),
                failing.clone(),
                missing.clone(),
                blocked.clone(),
                declined.clone(),
                stale.clone(),
            ],
        ),
        ran(task_id, passing, Some(0), &[], 5),
        ran(task_id, failing, Some(1), &[], 5),
        skipped(
            task_id,
            blocked,
            SkipReason::Blocked {
                reason: "not allowed".to_string(),
            },
            5,
        ),
        skipped(task_id, declined.clone(), SkipReason::DeclinedByUser, 5),
        ran(
            task_id,
            stale.clone(),
            Some(0),
            &[("src/lib.rs", "sha256:old")],
            5,
        ),
        // Finished first, then a fresh due: the run is due without also being
        // started-but-unfinished.
        finished(task_id, VerificationOutcome::Passed),
        due(task_id),
    ]);
    let pending = PendingDecision::Approval(PendingApprovalRef {
        kind: "patch".to_string(),
        proposal_id: "patch_1".to_string(),
    });
    let current = BTreeMap::from([("src/lib.rs".to_string(), "sha256:new".to_string())]);

    let report = evaluate(
        task_id,
        Some(&plan),
        &verification,
        Some(&pending),
        &current,
    );

    assert!(!report.eligible);
    assert_eq!(
        report.reason_codes(),
        vec![
            CompletionReasonCode::OpenPlanSteps,
            CompletionReasonCode::VerificationDue,
            CompletionReasonCode::PendingDecision,
            CompletionReasonCode::RequiredCheckFailed,
            CompletionReasonCode::RequiredCheckMissing,
            CompletionReasonCode::RequiredCheckSkipped,
            CompletionReasonCode::RequiredCheckStale,
        ]
    );
    assert_eq!(report.waivers.len(), 1);
    assert_eq!(report.waivers[0].check, declined);
    assert!(
        report
            .reasons
            .iter()
            .all(|reason| !reason.explanation.is_empty())
    );
}

/// R1 regression from the 2026-11-04 review of specs 101–105: the gate
/// filtered on `StepStatus::is_terminal`, which counts `Blocked`, so a plan
/// whose last step failed its command had no open steps and answered itself
/// `complete`. A blocked step is outstanding work with its own reason.
#[test]
fn a_blocked_plan_step_blocks_the_gate_with_its_own_reason() {
    let task_id = "task_1";
    let plan = plan(task_id, vec![step("run-tests", StepStatus::Blocked)]);

    let report = evaluate(task_id, Some(&plan), &state(&[]), None, &BTreeMap::new());

    assert!(
        !report.eligible,
        "a blocked step is not finished: {report:?}"
    );
    assert_eq!(
        report
            .reason_codes()
            .iter()
            .map(|code| code.as_str())
            .collect::<Vec<_>>(),
        vec!["blocked_plan_steps"]
    );
    assert!(
        report.reasons[0].explanation.contains("run-tests"),
        "{}",
        report.reasons[0].explanation
    );
}

/// The other side of R1: `Skipped` is a settled step, so a plan holding only a
/// skipped step has nothing outstanding. Only `Blocked` changed meaning.
#[test]
fn a_skipped_plan_step_is_closed() {
    let task_id = "task_1";
    let plan = plan(task_id, vec![step("run-tests", StepStatus::Skipped)]);

    let report = evaluate(task_id, Some(&plan), &state(&[]), None, &BTreeMap::new());

    assert!(report.eligible, "{report:?}");
    assert!(report.reasons.is_empty());
}

#[test]
fn the_service_settles_a_blocked_step_incomplete_and_a_skipped_step_complete() {
    let (root, store, _session, blocked_task) = fixture("settle-blocked");
    let blocked_plan = plan(
        &blocked_task.id,
        vec![step("run-tests", StepStatus::Blocked)],
    );
    store.create_plan(&blocked_task, &blocked_plan).unwrap();

    let service = CompletionGateService::new(&store, &root);
    let settled = service
        .settle(&blocked_task, CompletionSettlement::Gate)
        .unwrap();
    assert_eq!(
        settled.task.status,
        TaskStatus::Incomplete,
        "{:?}",
        settled.report
    );

    let (root, store, _session, skipped_task) = fixture("settle-skipped");
    let skipped_plan = plan(
        &skipped_task.id,
        vec![step("run-tests", StepStatus::Skipped)],
    );
    store.create_plan(&skipped_task, &skipped_plan).unwrap();

    let service = CompletionGateService::new(&store, &root);
    let settled = service
        .settle(&skipped_task, CompletionSettlement::Gate)
        .unwrap();
    assert_eq!(
        settled.task.status,
        TaskStatus::Complete,
        "{:?}",
        settled.report
    );
}

#[test]
fn a_started_verification_without_a_finish_blocks_on_its_own() {
    let task_id = "task_1";
    let report = evaluate(
        task_id,
        None,
        &state(&[started(task_id, &[])]),
        None,
        &BTreeMap::new(),
    );

    assert!(!report.eligible);
    assert_eq!(
        report.reason_codes(),
        vec![CompletionReasonCode::VerificationUnfinished]
    );
}

#[test]
fn a_complete_plan_with_a_passing_current_check_is_eligible() {
    let task_id = "task_1";
    let check = check("", "cargo test");
    let plan = plan(
        task_id,
        vec![checked_step(
            "implement",
            "cargo test",
            StepStatus::Completed,
        )],
    );
    let report = evaluate(
        task_id,
        Some(&plan),
        &state(&[
            started(task_id, std::slice::from_ref(&check)),
            ran(task_id, check, Some(0), &[], 0),
            finished(task_id, VerificationOutcome::Passed),
        ]),
        None,
        &BTreeMap::new(),
    );

    assert!(report.eligible, "{report:?}");
    assert!(report.reasons.is_empty());
    assert!(report.waivers.is_empty());
}

#[test]
fn a_declined_check_is_a_named_waiver_and_never_a_pass() {
    let task_id = "task_1";
    let declined = check("", "cargo test");
    let plan = plan(
        task_id,
        vec![checked_step(
            "implement",
            "cargo test",
            StepStatus::Completed,
        )],
    );
    let report = evaluate(
        task_id,
        Some(&plan),
        &state(&[
            started(task_id, std::slice::from_ref(&declined)),
            skipped(task_id, declined.clone(), SkipReason::DeclinedByUser, 0),
            finished(task_id, VerificationOutcome::NothingVerified),
        ]),
        None,
        &BTreeMap::new(),
    );

    assert!(report.eligible);
    assert!(report.reasons.is_empty());
    assert_eq!(report.waivers.len(), 1);
    assert_eq!(report.waivers[0].check, declined);
    assert!(report.waivers[0].explanation.contains("declined"));
}

/// A deferred required check never produces a `CheckRun`: the worker runs it as
/// an ordinary command at its plan step, recording a `CommandExit`. The gate
/// reads that evidence from the step when the loop recorded nothing.
#[test]
fn a_deferred_required_check_passes_through_its_step_command() {
    let task_id = "task_1";
    let mut test = checked_step("test", "cargo test", StepStatus::Completed);
    test.evidence = vec![command_exit("cargo test", Some(0))];
    let plan = plan(task_id, vec![test]);

    let report = evaluate(task_id, Some(&plan), &state(&[]), None, &BTreeMap::new());

    assert!(report.eligible, "{report:?}");
    assert!(report.reasons.is_empty());
    assert!(report.waivers.is_empty());
}

#[test]
fn a_deferred_required_check_fails_through_its_step_command() {
    let task_id = "task_1";
    let mut test = checked_step("test", "cargo test", StepStatus::Completed);
    test.evidence = vec![command_exit("cargo test", Some(1))];
    let plan = plan(task_id, vec![test]);

    let report = evaluate(task_id, Some(&plan), &state(&[]), None, &BTreeMap::new());

    assert!(!report.eligible);
    assert_eq!(
        report.reason_codes(),
        vec![CompletionReasonCode::RequiredCheckFailed]
    );
}

#[test]
fn a_deferred_required_check_without_a_command_is_missing() {
    let task_id = "task_1";
    let plan = plan(
        task_id,
        vec![checked_step("test", "cargo test", StepStatus::Completed)],
    );

    let report = evaluate(task_id, Some(&plan), &state(&[]), None, &BTreeMap::new());

    assert!(!report.eligible);
    assert_eq!(
        report.reason_codes(),
        vec![CompletionReasonCode::RequiredCheckMissing]
    );
}

#[test]
fn a_deferred_required_check_is_stale_when_a_later_step_begins() {
    let task_id = "task_1";
    let mut test = checked_step("test", "cargo test", StepStatus::Completed);
    test.evidence = vec![command_exit("cargo test", Some(0))];
    let plan = plan(task_id, vec![test, step("document", StepStatus::Completed)]);

    let report = evaluate(task_id, Some(&plan), &state(&[]), None, &BTreeMap::new());

    assert!(!report.eligible);
    assert_eq!(
        report.reason_codes(),
        vec![CompletionReasonCode::RequiredCheckStale]
    );
}

/// R4 regression from the 2026-11-04 review of specs 101–105: within the same
/// step, a patch applied after the check was ignored because currentness only
/// looked at step position. The latest durable record here is a mutation, so
/// the check no longer describes the tree the task would close on.
#[test]
fn a_patch_applied_after_the_deferred_check_makes_its_evidence_stale() {
    let task_id = "task_1";
    let mut test = checked_step("test", "cargo test", StepStatus::Completed);
    test.evidence = vec![command_exit("cargo test", Some(0)), patch_applied()];
    let plan = plan(task_id, vec![test]);

    let report = evaluate(task_id, Some(&plan), &state(&[]), None, &BTreeMap::new());

    assert!(!report.eligible);
    assert_eq!(
        report.reason_codes(),
        vec![CompletionReasonCode::RequiredCheckStale]
    );
}

/// The rerun is the newest evidence, so it is current even though a patch sits
/// between it and the earlier pass.
#[test]
fn a_deferred_check_rerun_after_a_patch_is_current() {
    let task_id = "task_1";
    let mut test = checked_step("test", "cargo test", StepStatus::Completed);
    test.evidence = vec![
        command_exit("cargo test", Some(0)),
        patch_applied(),
        command_exit("cargo test", Some(0)),
    ];
    let plan = plan(task_id, vec![test]);

    let report = evaluate(task_id, Some(&plan), &state(&[]), None, &BTreeMap::new());

    assert!(report.eligible, "{report:?}");
}

/// A patch in a later step is also a mutation after the check. The step-position
/// rule already caught this one; it stays current only if no mutation follows.
#[test]
fn a_patch_applied_in_a_later_step_makes_the_check_stale() {
    let task_id = "task_1";
    let mut test = checked_step("test", "cargo test", StepStatus::Completed);
    test.evidence = vec![command_exit("cargo test", Some(0))];
    let mut later = step("document", StepStatus::Completed);
    later.evidence = vec![patch_applied()];
    let plan = plan(task_id, vec![test, later]);

    let report = evaluate(task_id, Some(&plan), &state(&[]), None, &BTreeMap::new());

    assert!(!report.eligible);
    assert_eq!(
        report.reason_codes(),
        vec![CompletionReasonCode::RequiredCheckStale]
    );
}

/// A `CommandExit` written before the working directory was recorded carries no
/// directory. It must not satisfy a required check whose directory is not the
/// repository root: the old event cannot prove where the command ran.
#[test]
fn a_command_exit_written_before_the_directory_was_recorded_does_not_satisfy_a_subdirectory_check()
{
    let task_id = "task_1";
    let mut test = checked_step_in("test", "cargo test", "crates/foo", StepStatus::Completed);
    test.evidence = vec![legacy_command_exit("cargo test", Some(0))];
    let plan = plan(task_id, vec![test]);

    let report = evaluate(task_id, Some(&plan), &state(&[]), None, &BTreeMap::new());

    assert_eq!(
        report.reason_codes(),
        vec![CompletionReasonCode::RequiredCheckMissing],
        "an old event with no directory cannot stand for a subdirectory check"
    );
}

/// For a root-directory check an event with no recorded directory still
/// satisfies it. Before the directory was recorded, every matching command was
/// treated as a root check, so this keeps an in-flight session's existing
/// evidence meaningful instead of invalidating it on upgrade. The choice is
/// recorded in spec 105's review-fix note.
#[test]
fn a_command_exit_written_before_the_directory_was_recorded_still_satisfies_a_root_check() {
    let task_id = "task_1";
    let mut test = checked_step("test", "cargo test", StepStatus::Completed);
    test.evidence = vec![legacy_command_exit("cargo test", Some(0))];
    let plan = plan(task_id, vec![test]);

    let report = evaluate(task_id, Some(&plan), &state(&[]), None, &BTreeMap::new());

    assert!(report.eligible, "{report:?}");
    assert!(report.reasons.is_empty());
}

/// Spec 105 requirement 10: the gate reads structured checks, never plan
/// prose. A historical plan whose step text names a command — "Run `cargo
/// test`" — but carries no `check` field is eligible: nothing invents a required
/// check from prose, and the `CommandExit` a prose step happened to record does
/// not become one either.
#[test]
fn a_prose_only_plan_invents_no_required_check_for_the_gate() {
    let task_id = "task_1";
    let mut prose = step("test", StepStatus::Completed);
    prose.title = "Run the suite".to_string();
    prose.detail = Some("Run `cargo test` to validate the final tree".to_string());
    // No `check` field: the prose names a command but carries no structured
    // check, exactly what a pre-spec plan or a plan the model wrote without the
    // check object looks like.
    let plan = plan(task_id, vec![prose]);

    let report = evaluate(task_id, Some(&plan), &state(&[]), None, &BTreeMap::new());

    assert!(report.eligible, "{report:?}");
    assert!(report.reasons.is_empty());
    assert!(report.waivers.is_empty());
}

#[test]
fn a_pending_question_blocks_and_never_becomes_a_waiver() {
    let task_id = "task_1";
    let plan = plan(task_id, vec![step("implement", StepStatus::Completed)]);
    let pending = PendingDecision::Question(PendingQuestionRef::new());
    let report = evaluate(
        task_id,
        Some(&plan),
        &state(&[]),
        Some(&pending),
        &BTreeMap::new(),
    );

    assert!(!report.eligible);
    assert_eq!(
        report.reason_codes(),
        vec![CompletionReasonCode::PendingDecision]
    );
    assert!(report.waivers.is_empty());
}

#[test]
fn a_report_round_trips_beside_the_status_and_the_newest_wins() {
    let (_root, store, session, task) = fixture("round-trip");
    let first = evaluate(&task.id, None, &state(&[]), None, &BTreeMap::new());
    assert!(first.eligible);

    let stopped = store
        .update_task_status_with_completion(&task, TaskStatus::Incomplete, &first)
        .unwrap();
    assert_eq!(stopped.status, TaskStatus::Incomplete);
    assert!(stopped.completed_at_ms.is_some());
    assert_eq!(
        store.read_task_statuses(&session.id).unwrap().get(&task.id),
        Some(&"incomplete".to_string())
    );
    assert_eq!(
        store.read_completion_report(&session.id, &task.id).unwrap(),
        Some(first)
    );

    let open = plan(&task.id, vec![step("implement", StepStatus::Pending)]);
    let second = evaluate(&task.id, Some(&open), &state(&[]), None, &BTreeMap::new());
    assert!(!second.eligible);
    store
        .update_task_status_with_completion(&task, TaskStatus::Incomplete, &second)
        .unwrap();
    assert_eq!(
        store.read_completion_report(&session.id, &task.id).unwrap(),
        Some(second)
    );
}

#[test]
fn a_report_for_another_task_is_refused() {
    let (_root, store, _session, task) = fixture("wrong-task");
    let report = evaluate("task_other", None, &state(&[]), None, &BTreeMap::new());

    let error = store
        .update_task_status_with_completion(&task, TaskStatus::Incomplete, &report)
        .unwrap_err();

    assert!(matches!(error, ClientError::InvalidInput(_)));
}

#[test]
fn a_historical_status_event_has_no_report_and_stays_valid() {
    let (_root, store, session, task) = fixture("historical");
    store
        .update_task_status(&task, TaskStatus::Complete, None)
        .unwrap();

    assert_eq!(
        store.read_task_statuses(&session.id).unwrap().get(&task.id),
        Some(&"complete".to_string())
    );
    assert_eq!(
        store.read_completion_report(&session.id, &task.id).unwrap(),
        None
    );
}

#[test]
fn the_service_accepts_a_complete_passing_task_and_notices_staleness() {
    let root = temp_dir("service");
    let store = SessionStore::new(root.join(".damaian"));
    let session = store
        .create_session(&repository_id_for_root(&root), "Service")
        .unwrap();
    let task = store
        .create_task(&session.id, "change the file", "mock", "mock-model")
        .unwrap();
    let plan = plan(
        &task.id,
        vec![checked_step(
            "implement",
            "cargo test",
            StepStatus::Completed,
        )],
    );
    store.create_plan(&task, &plan).unwrap();

    fs::create_dir_all(root.join("src")).unwrap();
    fs::write(root.join("src/lib.rs"), "one").unwrap();
    let hash = file_hash(root.join("src/lib.rs")).unwrap();
    let cargo_test = check("", "cargo test");
    store
        .append_verification_event(
            &session.id,
            &started(&task.id, std::slice::from_ref(&cargo_test)),
        )
        .unwrap();
    store
        .append_verification_event(
            &session.id,
            &ran(
                &task.id,
                cargo_test,
                Some(0),
                &[("src/lib.rs", hash.as_str())],
                0,
            ),
        )
        .unwrap();
    store
        .append_verification_event(
            &session.id,
            &finished(&task.id, VerificationOutcome::Passed),
        )
        .unwrap();

    let service = CompletionGateService::new(&store, &root);
    let report = service.report(&session.id, &task.id).unwrap();
    assert!(report.eligible, "{report:?}");

    fs::write(root.join("src/lib.rs"), "two").unwrap();
    let report = service.report(&session.id, &task.id).unwrap();
    assert!(!report.eligible);
    assert_eq!(
        report.reason_codes(),
        vec![CompletionReasonCode::RequiredCheckStale]
    );
}

#[test]
fn the_service_sees_checks_from_a_sibling_continuation_but_not_another_task() {
    let (root, store, session, first) = fixture("chain");
    let first_plan = plan(
        &first.id,
        vec![checked_step(
            "implement",
            "cargo test",
            StepStatus::Completed,
        )],
    );
    store.create_plan(&first, &first_plan).unwrap();

    let verification = store
        .create_task(&session.id, "verify", "mock", "mock-model")
        .unwrap();
    store.resume_plan(&verification, &first.id).unwrap();
    let continuation = store
        .create_task(&session.id, "continue", "mock", "mock-model")
        .unwrap();
    store.resume_plan(&continuation, &first.id).unwrap();
    let unrelated = store
        .create_task(&session.id, "unrelated", "mock", "mock-model")
        .unwrap();
    let unrelated_plan = plan(
        &unrelated.id,
        vec![step("implement", StepStatus::Completed)],
    );
    store.create_plan(&unrelated, &unrelated_plan).unwrap();

    let cargo_test = check("", "cargo test");
    store
        .append_verification_event(
            &session.id,
            &started(&verification.id, std::slice::from_ref(&cargo_test)),
        )
        .unwrap();
    store
        .append_verification_event(
            &session.id,
            &ran(&verification.id, cargo_test, Some(1), &[], 0),
        )
        .unwrap();
    store
        .append_verification_event(
            &session.id,
            &finished(&verification.id, VerificationOutcome::Failed),
        )
        .unwrap();

    let service = CompletionGateService::new(&store, &root);
    // The continuation and the verification turn are siblings under the same
    // plan task, so the failing check belongs to the continuation's chain.
    let report = service.report(&session.id, &continuation.id).unwrap();
    assert!(!report.eligible);
    assert_eq!(
        report.reason_codes(),
        vec![CompletionReasonCode::RequiredCheckFailed]
    );

    // A user-started task has no continuation edge and inherits nothing from a
    // sibling's verification.
    let report = service.report(&session.id, &unrelated.id).unwrap();
    assert!(report.eligible, "{report:?}");
}

#[test]
fn the_service_refuses_another_repository_and_an_unknown_task() {
    let root = temp_dir("repository");
    let other = temp_dir("other-repository");
    let store = SessionStore::new(root.join(".damaian"));
    let session = store
        .create_session(&repository_id_for_root(&root), "Bound")
        .unwrap();
    let task = store
        .create_task(&session.id, "do the work", "mock", "mock-model")
        .unwrap();

    let mismatch = CompletionGateService::new(&store, &other)
        .report(&session.id, &task.id)
        .unwrap_err();
    assert!(matches!(mismatch, ClientError::AccessDenied(_)));

    let service = CompletionGateService::new(&store, &root);
    let unknown_task = service.report(&session.id, "task_unknown").unwrap_err();
    assert!(matches!(unknown_task, ClientError::InvalidInput(_)));

    let unknown_session = service.report("session_unknown", &task.id).unwrap_err();
    assert!(matches!(unknown_session, ClientError::InvalidInput(_)));
}

#[test]
fn incomplete_is_terminal_not_successful_and_round_trips() {
    assert!(TaskStatus::Incomplete.is_terminal());
    assert!(!TaskStatus::Incomplete.is_active());
    assert!(!TaskStatus::Incomplete.is_successful());
    assert!(!TaskStatus::Incomplete.may_have_side_effect_in_flight());
    assert!(TaskStatus::all().contains(&TaskStatus::Incomplete));
    assert_eq!(
        TaskStatus::parse(TaskStatus::Incomplete.as_str()),
        Some(TaskStatus::Incomplete)
    );

    // The historical spelling still parses as success; adding Incomplete did
    // not rewrite what older logs recorded.
    assert_eq!(TaskStatus::parse("complete"), Some(TaskStatus::Complete));
    assert!(TaskStatus::Complete.is_successful());
    assert!(!TaskStatus::Failed.is_successful());

    let tasks: Vec<&str> = TaskStatus::all()
        .iter()
        .filter(|status| status.is_successful())
        .map(TaskStatus::as_str)
        .collect();
    assert_eq!(tasks, vec!["complete"]);
}

#[test]
fn a_verification_step_does_not_keep_the_plan_open() {
    let task_id = "task_1";
    let plan = plan(
        task_id,
        vec![
            step("implement", StepStatus::Completed),
            step("verify_1", StepStatus::InProgress),
        ],
    );
    let report = evaluate(task_id, Some(&plan), &state(&[]), None, &BTreeMap::new());

    assert!(report.eligible, "{report:?}");
}

// --- Spec 106 Task 1: an unchanged pre-existing failure (context D1) --------
//
// A required check whose post-change result is the same failure the task
// started with is a named disposition, never a pass. Only a real task-start
// baseline qualifies, only the classification written after the latest run
// counts, and any introduced finding still blocks.

mod pre_existing {
    use super::*;
    use workspace_engine::finding::{Finding, FindingDraft, FindingSource, Severity};
    use workspace_engine::{
        CheckBaseline, CheckTermination, FailureFingerprint, FailureProvenance, FindingEvidence,
        PostPatchResult, SecretScanner, classify,
    };

    const TASK: &str = "task_1";

    fn root() -> CheckId {
        check("", "cargo test")
    }

    /// The generic finding an unparsed check produces from its output.
    fn output(details: &str) -> FindingEvidence {
        let finding = Finding::new(
            FindingDraft {
                source: FindingSource::Command,
                severity: Severity::Error,
                summary: "cargo test failed".to_string(),
                details: Some(details.to_string()),
                range: None,
                code: None,
            },
            &SecretScanner::new(Vec::new()),
        );
        FindingEvidence::from_finding(&finding)
    }

    fn legacy() -> FindingEvidence {
        output("test legacy::tests::broken ... FAILED\n")
    }

    fn legacy_and_sibling() -> FindingEvidence {
        output(
            "test legacy::tests::broken ... FAILED\ntest sibling::tests::uses_member ... FAILED\n",
        )
    }

    fn exited() -> CheckTermination {
        CheckTermination::Exited { exit_code: 1 }
    }

    fn failed_baseline(findings: &[FindingEvidence]) -> CheckBaseline {
        CheckBaseline::Failed {
            fingerprint: FailureFingerprint::compute(&root(), findings, &exited()),
            termination: exited(),
            findings: findings.to_vec(),
        }
    }

    fn baseline_event(baseline: CheckBaseline) -> VerificationEvent {
        VerificationEvent::Baseline {
            task_id: TASK.to_string(),
            check: root(),
            baseline,
        }
    }

    fn provenance_event(
        baseline: &CheckBaseline,
        findings: Vec<FindingEvidence>,
        previous: Option<&workspace_engine::CheckProvenanceRecord>,
    ) -> (VerificationEvent, workspace_engine::CheckProvenanceRecord) {
        let result = PostPatchResult::new(root(), exited(), findings);
        let record = classify(&result, Some(baseline), previous).record();
        (
            VerificationEvent::Provenance {
                task_id: TASK.to_string(),
                record: record.clone(),
            },
            record,
        )
    }

    fn finished_plan() -> TaskPlan {
        plan(TASK, vec![step("work", StepStatus::Completed)])
    }

    fn gate(events: &[VerificationEvent]) -> CompletionGateReport {
        let plan = finished_plan();
        let verification = state(events);
        CompletionGateReport::evaluate(CompletionFacts {
            task_id: TASK,
            plan: Some(&plan),
            verification: &verification,
            pending: None,
            current_hashes: &BTreeMap::new(),
            floor_checks: &[root()],
        })
    }

    fn codes(report: &CompletionGateReport) -> Vec<&'static str> {
        report
            .reason_codes()
            .iter()
            .map(|code| code.as_str())
            .collect()
    }

    #[test]
    fn an_unchanged_pre_existing_floor_failure_lets_the_task_complete() {
        let baseline = failed_baseline(&[legacy()]);
        let (provenance, record) = provenance_event(&baseline, vec![legacy()], None);
        assert_eq!(record.provenance, FailureProvenance::PreExisting);
        let report = gate(&[
            baseline_event(baseline),
            due(TASK),
            started(TASK, &[root()]),
            ran(TASK, root(), Some(1), &[], 0),
            provenance,
            finished(TASK, VerificationOutcome::Failed),
        ]);

        assert!(
            report.eligible,
            "an unchanged pre-existing failure must not block completion: {report:?}"
        );
        assert!(report.reasons.is_empty(), "{report:?}");
        assert!(
            report.waivers.is_empty(),
            "accepting a pre-existing failure is not a user waiver: {report:?}"
        );
        assert_eq!(report.accepted_failures.len(), 1, "{report:?}");
        assert_eq!(report.accepted_failures[0].check, root());
        assert_eq!(
            report.accepted_failures[0].provenance,
            FailureProvenance::PreExisting
        );
        assert!(
            !report.accepted_failures[0].explanation.contains("passed"),
            "an accepted failure is never described as passed: {report:?}"
        );
    }

    #[test]
    fn a_report_without_accepted_failures_keeps_its_old_wire_shape() {
        let report = gate(&[]);
        let json = serde_json::to_value(&report).expect("the report serializes");
        assert!(
            json.get("acceptedFailures").is_none(),
            "an empty list is not written: {json}"
        );
        let old = serde_json::json!({
            "taskId": TASK,
            "eligible": true,
            "reasons": [],
            "waivers": [],
        });
        let loaded: CompletionGateReport =
            serde_json::from_value(old).expect("a pre-spec-106 report loads");
        assert!(loaded.accepted_failures.is_empty());
    }

    #[test]
    fn an_unchanged_failure_after_an_earlier_classification_is_accepted_too() {
        let baseline = failed_baseline(&[legacy()]);
        let (first, first_record) = provenance_event(&baseline, vec![legacy()], None);
        let (second, second_record) =
            provenance_event(&baseline, vec![legacy()], Some(&first_record));
        assert_eq!(
            second_record.provenance,
            FailureProvenance::UnchangedFailure
        );
        let report = gate(&[
            baseline_event(baseline),
            due(TASK),
            started(TASK, &[root()]),
            ran(TASK, root(), Some(1), &[], 0),
            first,
            ran(TASK, root(), Some(1), &[], 0),
            second,
            finished(TASK, VerificationOutcome::Failed),
        ]);

        assert!(report.eligible, "{report:?}");
    }

    #[test]
    fn an_introduced_finding_on_top_of_a_pre_existing_failure_still_blocks() {
        let baseline = failed_baseline(&[legacy()]);
        let (provenance, record) = provenance_event(&baseline, vec![legacy_and_sibling()], None);
        assert!(
            !record.introduced.is_empty(),
            "the sibling failure is introduced: {record:?}"
        );
        let report = gate(&[
            baseline_event(baseline),
            due(TASK),
            started(TASK, &[root()]),
            ran(TASK, root(), Some(1), &[], 0),
            provenance,
            finished(TASK, VerificationOutcome::Failed),
        ]);

        assert!(!report.eligible, "{report:?}");
        assert_eq!(codes(&report), vec!["required_check_failed"]);
    }

    #[test]
    fn an_uncertain_failure_is_never_accepted() {
        let (provenance, record) =
            provenance_event(&CheckBaseline::Uncertain, vec![legacy()], None);
        assert_eq!(record.provenance, FailureProvenance::Uncertain);
        let report = gate(&[
            baseline_event(CheckBaseline::Uncertain),
            due(TASK),
            started(TASK, &[root()]),
            ran(TASK, root(), Some(1), &[], 0),
            provenance,
            finished(TASK, VerificationOutcome::Failed),
        ]);

        assert!(!report.eligible, "{report:?}");
        assert_eq!(codes(&report), vec!["required_check_failed"]);
    }

    #[test]
    fn a_failure_after_a_declined_baseline_is_never_accepted() {
        let (provenance, record) = provenance_event(&CheckBaseline::Declined, vec![legacy()], None);
        assert_eq!(record.provenance, FailureProvenance::UnknownProvenance);
        let report = gate(&[
            baseline_event(CheckBaseline::Declined),
            due(TASK),
            started(TASK, &[root()]),
            ran(TASK, root(), Some(1), &[], 0),
            provenance,
            finished(TASK, VerificationOutcome::Failed),
        ]);

        assert!(!report.eligible, "{report:?}");
        assert_eq!(codes(&report), vec!["required_check_failed"]);
    }

    #[test]
    fn a_classification_older_than_the_latest_run_is_not_accepted() {
        let baseline = failed_baseline(&[legacy()]);
        let (provenance, _) = provenance_event(&baseline, vec![legacy()], None);
        // The check ran again after its classification and nothing classified
        // the newer run (an interrupted verification). The old "pre-existing"
        // verdict must not vouch for it.
        let report = gate(&[
            baseline_event(baseline),
            due(TASK),
            started(TASK, &[root()]),
            ran(TASK, root(), Some(1), &[], 0),
            provenance,
            ran(TASK, root(), Some(1), &[], 0),
        ]);

        assert!(!report.eligible, "{report:?}");
        assert!(
            codes(&report).contains(&"required_check_failed"),
            "{report:?}"
        );
    }
}
