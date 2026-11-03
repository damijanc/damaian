//! Engine-owned task outcomes, actions and explicit continuation, per
//! `docs/specs/101_closed_task_completion.md` Task 3.
//!
//! The continuation is authorized here, not by a shell or a browser: these
//! tests drive the engine directly and prove the target is re-read under the
//! session log's lock, the plan and reasons travel, and a stale or
//! wrong-repository target creates nothing.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use workspace_engine::plan::StepStatus;
use workspace_engine::{
    CancelToken, ChatTurnResult, CompletionGateReport, CompletionReason, CompletionReasonCode,
    Config, MockModelAdapter, ModelAdapter, PlanStep, TaskPlan, TaskStatus, ToolCall, TurnProgress,
    TurnSink, WorkspaceEngine, hash::repository_id_for_root,
};

static COUNTER: AtomicU64 = AtomicU64::new(1);

fn temp_repo(name: &str) -> PathBuf {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock should work")
        .as_nanos();
    let repo = std::env::temp_dir().join(format!(
        "damaian-task-control-{name}-{now}-{}",
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

fn report(task_id: &str) -> CompletionGateReport {
    CompletionGateReport {
        task_id: task_id.to_string(),
        eligible: false,
        reasons: vec![CompletionReason {
            code: CompletionReasonCode::OpenPlanSteps,
            explanation: "1 planned step is still open: \"Finish the parser\".".to_string(),
        }],
        waivers: Vec::new(),
    }
}

fn call(name: &str, arguments_json: &str) -> ToolCall {
    ToolCall {
        id: format!("call_{name}"),
        name: name.to_string(),
        arguments_json: arguments_json.to_string(),
    }
}

/// A repository, a session, and one incomplete task carrying an open plan and
/// its gate report.
fn seed_incomplete(engine: &WorkspaceEngine, repo: &Path, name: &str) -> (String, String) {
    let store = &engine.session_store;
    let session = store
        .create_session(&repository_id_for_root(repo), name)
        .unwrap();
    let task = store
        .create_task(&session.id, "Write the parser", "mock", "mock")
        .unwrap();
    let mut plan = TaskPlan::new(&task.id, 0);
    plan.steps = vec![
        step("read", StepStatus::Completed),
        step("write", StepStatus::InProgress),
    ];
    store.create_plan(&task, &plan).unwrap();
    store
        .update_task_status_with_completion(&task, TaskStatus::Incomplete, &report(&task.id))
        .unwrap();
    (session.id, task.id)
}

fn sink<'a>(
    cancel: &'a CancelToken,
    on_token: &'a mut dyn FnMut(&str),
    on_progress: &'a mut dyn FnMut(TurnProgress),
) -> TurnSink<'a> {
    TurnSink {
        on_token,
        on_progress,
        cancel,
        steering: None,
    }
}

fn continue_task(
    engine: &WorkspaceEngine,
    repo: &Path,
    session_id: &str,
    task_id: &str,
    note: Option<&str>,
    adapter: &mut dyn ModelAdapter,
) -> workspace_engine::Result<ChatTurnResult> {
    let cancel = CancelToken::new();
    let mut on_token = |_token: &str| {};
    let mut on_progress = |_event: TurnProgress| {};
    let mut sink = sink(&cancel, &mut on_token, &mut on_progress);
    engine
        .chat_orchestrator
        .continue_incomplete_task(repo, session_id, task_id, note, adapter, &mut sink)
}

fn audit_events(repo: &Path) -> Vec<serde_json::Value> {
    fs::read_to_string(repo.join(".damaian/audit/events.jsonl"))
        .unwrap_or_default()
        .lines()
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
        .collect()
}

fn log_events(repo: &Path, session_id: &str) -> Vec<serde_json::Value> {
    fs::read_to_string(
        repo.join(".damaian/sessions")
            .join(format!("{session_id}.jsonl")),
    )
    .unwrap_or_default()
    .lines()
    .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
    .collect()
}

#[test]
fn continuing_an_incomplete_task_carries_its_plan_and_reasons_without_a_typed_word() {
    let repo = temp_repo("carry");
    let engine = engine_for(&repo);
    let (session_id, source_task_id) = seed_incomplete(&engine, &repo, "Closed task");
    let mut adapter = MockModelAdapter::new("I will finish the parser.");

    let result = continue_task(
        &engine,
        &repo,
        &session_id,
        &source_task_id,
        Some("Use the existing tokenizer."),
        &mut adapter,
    )
    .expect("the engine authorizes the continuation");

    assert_ne!(result.task.id, source_task_id);
    assert_eq!(result.task.status, TaskStatus::Incomplete);
    assert!(
        result
            .task
            .user_prompt
            .contains("1 planned step is still open"),
        "the engine prompt carries the durable reason: {}",
        result.task.user_prompt
    );
    assert!(
        result
            .task
            .user_prompt
            .contains("Use the existing tokenizer."),
        "the optional note is data in the engine prompt"
    );
    assert!(
        result.task.user_prompt.contains("not an approval"),
        "continuing grants nothing"
    );
    // The live result carries the same report-derived reasons a reload shows.
    assert!(
        result
            .task_control
            .reason_sentences()
            .iter()
            .any(|reason| reason.contains("1 planned step is still open")),
        "{:?}",
        result.task_control.reason_sentences()
    );
    let action = result
        .task_control
        .primary_action
        .as_ref()
        .expect("the new run that is still incomplete offers its next continuation");
    assert_eq!(action.command, workspace_engine::TaskCommand::ContinueTask);
    assert_eq!(action.task_id.as_deref(), Some(result.task.id.as_str()));
    assert_eq!(
        result.turn_presentation.indicator_state,
        workspace_engine::TurnIndicatorState::Incomplete,
        "the live turn's indicator is engine presentation"
    );
    assert!(result.turn_presentation.show_task_control);

    // The plan edge is the same durable one `resume_plan` writes, and it points
    // at the source task so protected paths and the gate can follow it.
    let events = log_events(&repo, &session_id);
    let resumed = events
        .iter()
        .find(|event| {
            event["eventType"] == "plan_resumed"
                && event["payload"]["taskId"] == result.task.id.as_str()
        })
        .expect("the carried plan has a durable edge");
    assert_eq!(resumed["payload"]["resumedFrom"], source_task_id);
    let plan = engine
        .session_store
        .read_task_plan(&session_id, &result.task.id)
        .unwrap()
        .expect("the new task carries the plan");
    assert_eq!(plan.steps[1].status, StepStatus::InProgress);
    assert_eq!(
        engine
            .session_store
            .read_completion_report(&session_id, &result.task.id)
            .unwrap()
            .expect("the new run settles through the gate again")
            .reason_codes(),
        vec![CompletionReasonCode::OpenPlanSteps]
    );

    // The source task is untouched and one continuation was recorded.
    let source = engine
        .session_store
        .task(&session_id, &source_task_id)
        .unwrap();
    assert_eq!(source.status, TaskStatus::Incomplete);
    assert_eq!(
        audit_events(&repo)
            .iter()
            .filter(|event| event["eventType"] == "task_continued")
            .count(),
        1
    );
}

#[test]
fn completing_the_carried_plan_on_the_next_run_settles_complete() {
    let repo = temp_repo("carry-complete");
    let engine = engine_for(&repo);
    let (session_id, source_task_id) = seed_incomplete(&engine, &repo, "Closed task");
    let mut adapter = MockModelAdapter::new_sequence_with_tool_calls(
        vec![String::new(), "The parser is finished.".to_string()],
        vec![vec![call("complete_step", "{}")], Vec::new()],
    );

    let result = continue_task(
        &engine,
        &repo,
        &session_id,
        &source_task_id,
        None,
        &mut adapter,
    )
    .expect("the engine authorizes the continuation");

    assert_eq!(result.task.status, TaskStatus::Complete);
    let report = engine
        .session_store
        .read_completion_report(&session_id, &result.task.id)
        .unwrap()
        .expect("the completed run carries its gate report");
    assert!(report.eligible, "{report:?}");
    let plan = engine
        .session_store
        .read_task_plan(&session_id, &result.task.id)
        .unwrap()
        .expect("the carried plan stays attached");
    assert!(
        plan.steps.iter().all(|step| step.status.is_terminal()),
        "the next run closed the remaining step instead of re-planning: {plan:?}"
    );
}

#[test]
fn a_stale_continuation_cannot_continue_a_later_incomplete_task() {
    let repo = temp_repo("stale-later");
    let engine = engine_for(&repo);
    let (session_id, older_task_id) = seed_incomplete(&engine, &repo, "Closed task");
    let newer = engine
        .session_store
        .create_task(&session_id, "Write the next parser", "mock", "mock")
        .unwrap();
    engine
        .session_store
        .update_task_status_with_completion(&newer, TaskStatus::Incomplete, &report(&newer.id))
        .unwrap();
    let before = engine.session_store.read_tasks(&session_id).unwrap().len();
    let mut adapter = MockModelAdapter::new("Unused.");

    let refused = continue_task(
        &engine,
        &repo,
        &session_id,
        &older_task_id,
        None,
        &mut adapter,
    )
    .unwrap_err();
    assert!(
        refused
            .to_string()
            .contains("no longer the newest incomplete task"),
        "{refused}"
    );
    assert_eq!(
        engine.session_store.read_tasks(&session_id).unwrap().len(),
        before,
        "the later incomplete task is not continued by a stale control"
    );

    let continued = continue_task(&engine, &repo, &session_id, &newer.id, None, &mut adapter)
        .expect("the id the control named is still the newest incomplete task");
    assert_ne!(continued.task.id, newer.id);
    assert_ne!(continued.task.id, older_task_id);
}

#[test]
fn the_continuation_transaction_requires_the_report_the_prompt_used() {
    let repo = temp_repo("report-mismatch");
    let engine = engine_for(&repo);
    let (session_id, source_task_id) = seed_incomplete(&engine, &repo, "Closed task");
    let mut stale = report(&source_task_id);
    stale.reasons[0].explanation = "An older reason the prompt did not carry.".to_string();

    let wrote = engine
        .session_store
        .continue_incomplete_task(
            &session_id,
            &source_task_id,
            &stale,
            "A prompt built from a different report.",
            "mock",
            "mock",
        )
        .unwrap();
    assert!(
        wrote.is_none(),
        "a report mismatch under the lock creates nothing"
    );
    assert_eq!(
        engine.session_store.read_tasks(&session_id).unwrap().len(),
        1
    );

    let current = report(&source_task_id);
    let wrote = engine
        .session_store
        .continue_incomplete_task(
            &session_id,
            &source_task_id,
            &current,
            "A prompt built from the current report.",
            "mock",
            "mock",
        )
        .unwrap()
        .expect("the matching report authorizes the transaction");
    assert_ne!(wrote.task.id, source_task_id);
}

#[test]
fn a_second_click_with_the_old_task_id_creates_nothing() {
    let repo = temp_repo("second");
    let engine = engine_for(&repo);
    let (session_id, source_task_id) = seed_incomplete(&engine, &repo, "Closed task");
    let mut adapter = MockModelAdapter::new_sequence(vec![
        "I will finish the parser.".to_string(),
        "Unused.".to_string(),
    ]);
    continue_task(
        &engine,
        &repo,
        &session_id,
        &source_task_id,
        None,
        &mut adapter,
    )
    .expect("the first click is authorized");
    let tasks_after_first = engine.session_store.read_tasks(&session_id).unwrap().len();

    let refused = continue_task(
        &engine,
        &repo,
        &session_id,
        &source_task_id,
        None,
        &mut adapter,
    )
    .unwrap_err();
    assert!(
        refused
            .to_string()
            .contains("no longer the newest incomplete task"),
        "{refused}"
    );
    assert_eq!(
        engine.session_store.read_tasks(&session_id).unwrap().len(),
        tasks_after_first,
        "a stale control creates no second task"
    );
}

#[test]
fn a_non_incomplete_or_unknown_target_is_refused() {
    let repo = temp_repo("refuse");
    let engine = engine_for(&repo);
    let (session_id, source_task_id) = seed_incomplete(&engine, &repo, "Closed task");
    engine
        .session_store
        .update_task_status(
            &engine
                .session_store
                .task(&session_id, &source_task_id)
                .unwrap(),
            TaskStatus::Complete,
            None,
        )
        .unwrap();
    let mut adapter = MockModelAdapter::new("Unused.");

    let refused = continue_task(
        &engine,
        &repo,
        &session_id,
        &source_task_id,
        None,
        &mut adapter,
    )
    .unwrap_err();
    assert!(
        refused.to_string().contains("newest incomplete task"),
        "{refused}"
    );

    let refused = continue_task(
        &engine,
        &repo,
        &session_id,
        "task_unknown",
        None,
        &mut adapter,
    )
    .unwrap_err();
    assert!(
        refused.to_string().contains("newest incomplete task"),
        "{refused}"
    );
}

#[test]
fn a_wrong_repository_target_is_refused_before_anything_is_created() {
    let repo = temp_repo("right");
    let other = temp_repo("other");
    let engine = engine_for(&repo);
    let (session_id, source_task_id) = seed_incomplete(&engine, &repo, "Closed task");
    let before = engine.session_store.read_tasks(&session_id).unwrap().len();
    let mut adapter = MockModelAdapter::new("Unused.");

    let refused = continue_task(
        &engine,
        &other,
        &session_id,
        &source_task_id,
        None,
        &mut adapter,
    )
    .unwrap_err();
    assert!(
        refused.to_string().contains("different repository"),
        "{refused}"
    );
    assert_eq!(
        engine.session_store.read_tasks(&session_id).unwrap().len(),
        before
    );
}

#[test]
fn an_incomplete_worker_progress_status_stays_literal() {
    // The worker host writes `taskStatus` from `TaskStatus::as_str()`. The
    // engine's control must preserve that literal, not translate it.
    let report = report("task_1");
    let control = workspace_engine::returned_task_control("incomplete", Some(&report));
    assert_eq!(control.status.as_deref(), Some("incomplete"));
    assert_eq!(
        control.outcome,
        workspace_engine::TaskOutcomeKind::Incomplete
    );
    assert_ne!(control.outcome, workspace_engine::TaskOutcomeKind::Complete);
    assert!(
        control.detail.contains("1 planned step is still open"),
        "the returned card names the gate's reason: {}",
        control.detail
    );
}
