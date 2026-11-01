//! Settlement through the completion gate, per
//! `docs/specs/101_closed_task_completion.md` Task 2.
//!
//! The gate itself is tested in `closed_task_completion.rs`. These tests cover
//! the two engine paths that may newly write `complete` — an answered turn and
//! a successful verification — plus the durably marked crash window recovery
//! has to finish.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use workspace_engine::plan::StepStatus;
use workspace_engine::{
    AuditLog, COMPLETION_SETTLEMENT_ACTION, CancelToken, ChatTurnResult, CheckId, CheckOutcome,
    CompletionGateReport, CompletionReason, CompletionReasonCode, CompletionSettlement,
    CompletionSettlementMarker, Config, MockModelAdapter, ModelAdapter, PlanStep, SecretScanner,
    SessionStore, TaskPlan, TaskStatus, ToolCall, TurnProgress, TurnSink, VerificationEvent,
    WorkspaceEngine, classify_session, repository_id_for_root,
};

static COUNTER: AtomicU64 = AtomicU64::new(1);

fn temp_repo(name: &str) -> PathBuf {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock should work")
        .as_nanos();
    let repo = std::env::temp_dir().join(format!(
        "damaian-completion-settlement-{name}-{now}-{}",
        COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir_all(repo.join("src")).expect("repository should be created");
    fs::write(repo.join("src/a.rs"), "fn main() {}\n").expect("a file to read");
    repo
}

fn engine_for(repo: &Path) -> WorkspaceEngine {
    WorkspaceEngine::new(Config {
        data_dir: repo.join(".damaian"),
        enable_index_watcher: false,
        ..Config::default()
    })
}

fn ask(
    engine: &WorkspaceEngine,
    repo: &Path,
    prompt: &str,
    adapter: &mut dyn ModelAdapter,
) -> ChatTurnResult {
    let cancel = CancelToken::new();
    let mut on_token = |_token: &str| {};
    let mut on_progress = |_event: TurnProgress| {};
    let mut sink = TurnSink {
        on_token: &mut on_token,
        on_progress: &mut on_progress,
        cancel: &cancel,
        steering: None,
    };
    engine
        .chat_orchestrator
        .ask_with_session(repo, prompt, &[], None, adapter, &mut sink)
        .expect("the turn should run")
}

fn call(name: &str, arguments_json: &str) -> ToolCall {
    ToolCall {
        id: format!("call_{name}"),
        name: name.to_string(),
        arguments_json: arguments_json.to_string(),
    }
}

fn plan_call(titles: &[&str]) -> ToolCall {
    let steps = titles
        .iter()
        .map(|title| format!(r#"{{"title":"{title}"}}"#))
        .collect::<Vec<_>>()
        .join(",");
    call("propose_plan", &format!(r#"{{"steps":[{steps}]}}"#))
}

fn audit_events(repo: &Path, event_type: &str) -> Vec<serde_json::Value> {
    fs::read_to_string(repo.join(".damaian/audit/events.jsonl"))
        .unwrap_or_default()
        .lines()
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
        .filter(|event| event.get("eventType").and_then(|value| value.as_str()) == Some(event_type))
        .collect()
}

fn statuses_in_log(repo: &Path, session_id: &str, task_id: &str) -> Vec<String> {
    let path = repo
        .join(".damaian/sessions")
        .join(format!("{session_id}.jsonl"));
    fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
        .filter(|event| event["eventType"] == "task_status_updated")
        .filter(|event| event["payload"]["task"]["id"] == task_id)
        .filter_map(|event| {
            event["payload"]["task"]["status"]
                .as_str()
                .map(ToString::to_string)
        })
        .collect()
}

fn settlement_event_order(repo: &Path, session_id: &str) -> Vec<String> {
    let path = repo
        .join(".damaian/sessions")
        .join(format!("{session_id}.jsonl"));
    fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
        .filter_map(|event| {
            let event_type = event["eventType"].as_str()?;
            if event_type == "action_started" || event_type == "action_finished" {
                return (event["payload"]["action"] == COMPLETION_SETTLEMENT_ACTION)
                    .then(|| event_type.to_string());
            }
            (event_type == "task_status_updated" && !event["payload"]["completionReport"].is_null())
                .then(|| event_type.to_string())
        })
        .collect()
}

#[test]
fn two_answers_with_an_open_plan_settle_incomplete_and_keep_the_plan() {
    let repo = temp_repo("answered-open");
    let engine = engine_for(&repo);
    let mut adapter = MockModelAdapter::new_sequence_with_tool_calls(
        vec![
            String::new(),
            String::new(),
            "The report is written.".to_string(),
            "Still working.".to_string(),
        ],
        vec![
            vec![plan_call(&["Collect statuses", "Write the report"])],
            vec![call("read_file", r#"{"path":"src/a.rs"}"#)],
            Vec::new(),
            Vec::new(),
        ],
    );

    let result = ask(&engine, &repo, "Write the report", &mut adapter);

    assert_eq!(audit_events(&repo, "plan_reminder_sent").len(), 1);
    assert_eq!(result.task.status, TaskStatus::Incomplete);
    let plan = engine
        .session_store
        .read_task_plan(&result.session.id, &result.task.id)
        .unwrap()
        .expect("the plan stays attached");
    assert_eq!(plan.steps[0].status, StepStatus::InProgress);
    assert_eq!(plan.steps[1].status, StepStatus::Pending);

    let report = engine
        .session_store
        .read_completion_report(&result.session.id, &result.task.id)
        .unwrap()
        .expect("the settlement carries the gate report");
    assert!(!report.eligible);
    assert_eq!(
        report.reason_codes(),
        vec![CompletionReasonCode::OpenPlanSteps]
    );
    assert_eq!(
        statuses_in_log(&repo, &result.session.id, &result.task.id),
        vec!["incomplete"]
    );

    // #66's transcript behaviour is unchanged: the answer that triggered the
    // reminder and the answer after it are both useful assistant messages.
    let messages = engine
        .session_store
        .read_messages(&result.session.id)
        .unwrap();
    assert!(
        messages
            .iter()
            .any(|message| message.role == "assistant"
                && message.content == "The report is written."),
        "{messages:?}"
    );
    assert!(
        messages
            .iter()
            .any(|message| message.role == "assistant" && message.content == "Still working."),
        "{messages:?}"
    );
    let completed = audit_events(&repo, "model_response_completed");
    assert_eq!(
        completed.last().and_then(|event| event["status"].as_str()),
        Some("incomplete"),
        "the audit records the persisted verdict"
    );
}

#[test]
fn a_closed_plan_settles_complete_with_an_eligible_report() {
    let repo = temp_repo("answered-closed");
    let engine = engine_for(&repo);
    let mut adapter = MockModelAdapter::new_sequence_with_tool_calls(
        vec![
            String::new(),
            String::new(),
            String::new(),
            String::new(),
            "Done.".to_string(),
        ],
        vec![
            vec![plan_call(&["Read the source", "Report"])],
            vec![call("read_file", r#"{"path":"src/a.rs"}"#)],
            vec![call("complete_step", "{}")],
            vec![call("complete_step", "{}")],
            Vec::new(),
        ],
    );

    let result = ask(&engine, &repo, "Read the source", &mut adapter);

    assert_eq!(result.task.status, TaskStatus::Complete);
    let report = engine
        .session_store
        .read_completion_report(&result.session.id, &result.task.id)
        .unwrap()
        .expect("the settlement carries the gate report");
    assert!(report.eligible, "{report:?}");
    assert!(report.reasons.is_empty());
    assert_eq!(
        statuses_in_log(&repo, &result.session.id, &result.task.id),
        vec!["complete"]
    );
    assert_eq!(
        settlement_event_order(&repo, &result.session.id),
        vec!["action_started", "task_status_updated", "action_finished"],
        "the marker must be durable before the report/status and finished after"
    );
}

struct RecoveryFixture {
    root: PathBuf,
    store: SessionStore,
    audit: AuditLog,
    session_id: String,
}

fn recovery_fixture(name: &str) -> RecoveryFixture {
    let root = temp_repo(name);
    let data_dir = root.join(".damaian");
    let store = SessionStore::new(&data_dir);
    let audit = AuditLog::new(&data_dir, true, SecretScanner::default());
    let session = store
        .create_session(&repository_id_for_root(&root), "Recovery")
        .unwrap();
    RecoveryFixture {
        root,
        store,
        audit,
        session_id: session.id,
    }
}

fn plan_with(task_id: &str, status: StepStatus) -> TaskPlan {
    let mut plan = TaskPlan::new(task_id, 0);
    plan.steps = vec![PlanStep {
        id: "implement".to_string(),
        title: "implement the change".to_string(),
        detail: None,
        status,
        depends_on: Vec::new(),
        started_at_ms: None,
        completed_at_ms: None,
        evidence: Vec::new(),
    }];
    plan
}

/// Leaves the task exactly as a crash between the final answer and the status
/// write does: a live task, a settled plan, and a dangling settlement marker
/// naming the repository.
fn crashed_settlement(
    fixture: &RecoveryFixture,
    step_status: StepStatus,
    root_reference: &Path,
    settlement: CompletionSettlement,
) -> workspace_engine::Task {
    let task = fixture
        .store
        .create_task(&fixture.session_id, "do the work", "mock", "mock-model")
        .unwrap();
    let task = fixture
        .store
        .update_task_status(&task, TaskStatus::RunningTool, None)
        .unwrap();
    fixture
        .store
        .create_plan(&task, &plan_with(&task.id, step_status))
        .unwrap();
    // Never finished: stands in for the process dying before the status write.
    let reference = CompletionSettlementMarker::new(root_reference, settlement)
        .reference()
        .expect("the marker reference serializes");
    let _marker = fixture
        .store
        .start_action(&task, COMPLETION_SETTLEMENT_ACTION, &reference, false)
        .unwrap();
    task
}

fn settlement_action_events(fixture: &RecoveryFixture, event_type: &str) -> Vec<serde_json::Value> {
    fs::read_to_string(
        fixture
            .root
            .join(".damaian/sessions")
            .join(format!("{}.jsonl", fixture.session_id)),
    )
    .unwrap_or_default()
    .lines()
    .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
    .filter(|event| event["eventType"] == event_type)
    .filter(|event| event["payload"]["action"] == COMPLETION_SETTLEMENT_ACTION)
    .collect()
}

#[test]
fn a_crash_window_settles_an_open_task_incomplete() {
    let fixture = recovery_fixture("crash-open");
    let task = crashed_settlement(
        &fixture,
        StepStatus::Pending,
        &fixture.root,
        CompletionSettlement::Gate,
    );

    let recovered = classify_session(&fixture.store, &fixture.audit, &fixture.session_id).unwrap();

    assert!(recovered.is_empty(), "{recovered:?}");
    let statuses = fixture
        .store
        .read_task_statuses(&fixture.session_id)
        .unwrap();
    assert_eq!(
        statuses.get(&task.id).map(String::as_str),
        Some("incomplete")
    );
    let report = fixture
        .store
        .read_completion_report(&fixture.session_id, &task.id)
        .unwrap()
        .expect("the recomputed report is durable");
    assert_eq!(
        report.reason_codes(),
        vec![CompletionReasonCode::OpenPlanSteps]
    );
    let audit = fs::read_to_string(fixture.root.join(".damaian/audit/events.jsonl")).unwrap();
    assert!(
        audit.contains("task_completion_settled_after_crash"),
        "{audit}"
    );
    assert_eq!(
        settlement_action_events(&fixture, "action_started").len(),
        1,
        "recovery must close the original marker, not create another"
    );
    assert_eq!(
        settlement_action_events(&fixture, "action_finished").len(),
        1
    );
    assert!(
        fixture
            .store
            .dangling_actions(&fixture.session_id)
            .unwrap()
            .iter()
            .all(|action| action.action != COMPLETION_SETTLEMENT_ACTION),
        "the recovered marker is finished"
    );
}

#[test]
fn a_crash_window_settles_a_closed_task_complete() {
    let fixture = recovery_fixture("crash-closed");
    let task = crashed_settlement(
        &fixture,
        StepStatus::Completed,
        &fixture.root,
        CompletionSettlement::Gate,
    );

    let recovered = classify_session(&fixture.store, &fixture.audit, &fixture.session_id).unwrap();

    assert!(recovered.is_empty(), "{recovered:?}");
    let statuses = fixture
        .store
        .read_task_statuses(&fixture.session_id)
        .unwrap();
    assert_eq!(statuses.get(&task.id).map(String::as_str), Some("complete"));
    let report = fixture
        .store
        .read_completion_report(&fixture.session_id, &task.id)
        .unwrap()
        .expect("the recomputed report is durable");
    assert!(report.eligible, "{report:?}");
}

#[test]
fn a_crash_after_the_settlement_status_only_closes_the_marker() {
    let fixture = recovery_fixture("crash-after-status");
    let task = crashed_settlement(
        &fixture,
        StepStatus::Pending,
        &fixture.root,
        CompletionSettlement::Gate,
    );
    let report = CompletionGateReport {
        task_id: task.id.clone(),
        eligible: false,
        reasons: vec![CompletionReason {
            code: CompletionReasonCode::OpenPlanSteps,
            explanation: "1 planned step is still open.".to_string(),
        }],
        waivers: Vec::new(),
    };
    fixture
        .store
        .update_task_status_with_completion(&task, TaskStatus::Incomplete, &report)
        .unwrap();

    let recovered = classify_session(&fixture.store, &fixture.audit, &fixture.session_id).unwrap();

    assert!(recovered.is_empty(), "{recovered:?}");
    assert_eq!(
        fixture
            .store
            .read_task_statuses(&fixture.session_id)
            .unwrap()
            .get(&task.id)
            .map(String::as_str),
        Some("incomplete"),
        "a landed verdict is not rewritten"
    );
    assert_eq!(
        settlement_action_events(&fixture, "action_started").len(),
        1,
        "the original marker is reused, not duplicated"
    );
    assert_eq!(
        settlement_action_events(&fixture, "action_finished").len(),
        1
    );
    assert!(
        fixture
            .store
            .dangling_actions(&fixture.session_id)
            .unwrap()
            .iter()
            .all(|action| action.action != COMPLETION_SETTLEMENT_ACTION),
        "a settlement that landed before the crash is no longer a crash window"
    );
}

#[test]
fn a_crashed_failed_verification_settles_failed_with_its_report() {
    let fixture = recovery_fixture("crash-failed");
    let task = crashed_settlement(
        &fixture,
        StepStatus::Completed,
        &fixture.root,
        CompletionSettlement::VerificationFailed,
    );
    let check = CheckId::new("", "cargo test");
    for event in [
        VerificationEvent::Started {
            task_id: task.id.clone(),
            attempt: 0,
            checks: vec![check.clone()],
        },
        VerificationEvent::Check {
            task_id: task.id.clone(),
            check,
            outcome: CheckOutcome::Ran {
                exit_code: Some(1),
                marker_id: "marker".to_string(),
                termination: None,
            },
            file_hashes: BTreeMap::new(),
        },
        VerificationEvent::Finished {
            task_id: task.id.clone(),
            outcome: workspace_engine::VerificationOutcome::Failed,
        },
    ] {
        fixture
            .store
            .append_verification_event(&fixture.session_id, &event)
            .unwrap();
    }

    let recovered = classify_session(&fixture.store, &fixture.audit, &fixture.session_id).unwrap();

    assert!(recovered.is_empty(), "{recovered:?}");
    let statuses = fixture
        .store
        .read_task_statuses(&fixture.session_id)
        .unwrap();
    assert_eq!(statuses.get(&task.id).map(String::as_str), Some("failed"));
    let report = fixture
        .store
        .read_completion_report(&fixture.session_id, &task.id)
        .unwrap()
        .expect("the recomputed report is durable");
    assert!(!report.eligible);
    assert!(
        report
            .reason_codes()
            .contains(&CompletionReasonCode::RequiredCheckFailed)
    );
    assert_eq!(
        settlement_action_events(&fixture, "action_started").len(),
        1
    );
    assert_eq!(
        settlement_action_events(&fixture, "action_finished").len(),
        1
    );
}

#[test]
fn an_unknown_future_status_is_not_settled_from_a_marker() {
    let fixture = recovery_fixture("crash-future-status");
    let task = crashed_settlement(
        &fixture,
        StepStatus::Pending,
        &fixture.root,
        CompletionSettlement::Gate,
    );
    let log = fixture
        .root
        .join(".damaian/sessions")
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

    assert_eq!(recovered.len(), 1, "{recovered:?}");
    assert_eq!(
        recovered[0].previous_status,
        "negotiating_with_the_compiler"
    );
    assert_eq!(recovered[0].classification, TaskStatus::Interrupted);
    let statuses = fixture
        .store
        .read_task_statuses(&fixture.session_id)
        .unwrap();
    assert_eq!(
        statuses.get(&task.id).map(String::as_str),
        Some("negotiating_with_the_compiler"),
        "a status this build does not understand must not be overwritten"
    );
    assert!(
        fixture
            .store
            .read_completion_report(&fixture.session_id, &task.id)
            .unwrap()
            .is_none()
    );
}

#[test]
fn a_dangling_side_effect_keeps_the_unknown_outcome_over_a_completion_marker() {
    let fixture = recovery_fixture("crash-side-effect");
    let task = crashed_settlement(
        &fixture,
        StepStatus::Pending,
        &fixture.root,
        CompletionSettlement::Gate,
    );
    let _side_effect = fixture
        .store
        .start_action(&task, "run_command", "make the change", true)
        .unwrap();

    let recovered = classify_session(&fixture.store, &fixture.audit, &fixture.session_id).unwrap();

    assert_eq!(recovered.len(), 1, "{recovered:?}");
    assert_eq!(recovered[0].task_id, task.id);
    assert_eq!(
        recovered[0].classification,
        TaskStatus::UnknownExternalOutcome,
        "an unfinished side effect outranks a completion marker"
    );
    let statuses = fixture
        .store
        .read_task_statuses(&fixture.session_id)
        .unwrap();
    assert_eq!(
        statuses.get(&task.id).map(String::as_str),
        Some("running_tool")
    );
    assert!(
        fixture
            .store
            .read_completion_report(&fixture.session_id, &task.id)
            .unwrap()
            .is_none()
    );
}

#[test]
fn an_unusable_root_fails_safe_into_the_interrupted_path() {
    let fixture = recovery_fixture("crash-bad-root");
    let other = temp_repo("crash-other-root");
    let task = crashed_settlement(
        &fixture,
        StepStatus::Pending,
        &other,
        CompletionSettlement::Gate,
    );

    let recovered = classify_session(&fixture.store, &fixture.audit, &fixture.session_id).unwrap();

    assert_eq!(recovered.len(), 1, "{recovered:?}");
    assert_eq!(recovered[0].task_id, task.id);
    assert_eq!(recovered[0].classification, TaskStatus::Interrupted);
    let statuses = fixture
        .store
        .read_task_statuses(&fixture.session_id)
        .unwrap();
    assert_eq!(
        statuses.get(&task.id).map(String::as_str),
        Some("running_tool"),
        "nothing may be written from an unvalidated repository"
    );
    assert!(
        fixture
            .store
            .read_completion_report(&fixture.session_id, &task.id)
            .unwrap()
            .is_none()
    );
    let audit = fs::read_to_string(fixture.root.join(".damaian/audit/events.jsonl")).unwrap();
    assert!(
        audit.contains("task_completion_settlement_unavailable"),
        "{audit}"
    );
}
