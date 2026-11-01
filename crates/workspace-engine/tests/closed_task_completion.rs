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

use workspace_engine::hash::file_hash;
use workspace_engine::plan::StepStatus;
use workspace_engine::{
    CheckId, CheckOutcome, ClientError, CompletionFacts, CompletionGateReport,
    CompletionGateService, CompletionReasonCode, PendingApprovalRef, PendingDecision,
    PendingQuestionRef, PlanStep, Session, SessionStore, SkipReason, Task, TaskPlan, TaskStatus,
    VerificationEvent, VerificationOutcome, VerificationState, fold_verification,
    repository_id_for_root,
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

fn due(task_id: &str) -> VerificationEvent {
    VerificationEvent::Due {
        task_id: task_id.to_string(),
        patch_id: "patch".to_string(),
        paths: Vec::new(),
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
) -> VerificationEvent {
    VerificationEvent::Check {
        task_id: task_id.to_string(),
        check,
        outcome: CheckOutcome::Ran {
            exit_code,
            marker_id: "marker".to_string(),
        },
        file_hashes: hashes
            .iter()
            .map(|(path, hash)| ((*path).to_string(), (*hash).to_string()))
            .collect(),
    }
}

fn skipped(task_id: &str, check: CheckId, reason: SkipReason) -> VerificationEvent {
    VerificationEvent::Check {
        task_id: task_id.to_string(),
        check,
        outcome: CheckOutcome::Skipped(reason),
        file_hashes: BTreeMap::new(),
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

    let plan = plan(
        task_id,
        vec![
            step("implement", StepStatus::Pending),
            step("verify_1", StepStatus::InProgress),
            step("document", StepStatus::Skipped),
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
        ran(task_id, passing, Some(0), &[]),
        ran(task_id, failing, Some(1), &[]),
        skipped(
            task_id,
            blocked,
            SkipReason::Blocked {
                reason: "not allowed".to_string(),
            },
        ),
        skipped(task_id, declined.clone(), SkipReason::DeclinedByUser),
        ran(
            task_id,
            stale.clone(),
            Some(0),
            &[("src/lib.rs", "sha256:old")],
        ),
        // Finished first, then a fresh due: the checks stay selected and the
        // run is due without also being started-but-unfinished.
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
    let plan = plan(task_id, vec![step("implement", StepStatus::Completed)]);
    let report = evaluate(
        task_id,
        Some(&plan),
        &state(&[
            started(task_id, std::slice::from_ref(&check)),
            ran(task_id, check, Some(0), &[]),
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
    let plan = plan(task_id, vec![step("implement", StepStatus::Completed)]);
    let report = evaluate(
        task_id,
        Some(&plan),
        &state(&[
            started(task_id, std::slice::from_ref(&declined)),
            skipped(task_id, declined.clone(), SkipReason::DeclinedByUser),
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
    let plan = plan(&task.id, vec![step("implement", StepStatus::Completed)]);
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
    let first_plan = plan(&first.id, vec![step("implement", StepStatus::Completed)]);
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
            &ran(&verification.id, cargo_test, Some(1), &[]),
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
