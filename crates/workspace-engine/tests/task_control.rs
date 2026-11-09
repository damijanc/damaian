//! Engine-owned task outcomes, actions and explicit continuation, per
//! `docs/specs/101_closed_task_completion.md` Task 3.
//!
//! The continuation is authorized here, not by a shell or a browser: these
//! tests drive the engine directly and prove the target is re-read under the
//! session log's lock, the plan and reasons travel, and a stale or
//! wrong-repository target creates nothing.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use workspace_engine::plan::StepStatus;
use workspace_engine::{
    CancelToken, ChatTurnResult, CompletionGateReport, CompletionGateService, CompletionReason,
    CompletionReasonCode, Config, MockModelAdapter, ModelAdapter, PLANNED_CHECK_VERSION,
    PatchFileDecision, PlanStep, PlannedCheck, TaskPlan, TaskStatus, ToolCall, TurnProgress,
    TurnSink, WorkerRecord, WorkspaceEngine, hash::repository_id_for_root,
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

fn checked_step(id: &str, command: &str, status: StepStatus) -> PlanStep {
    let mut step = step(id, status);
    step.check = Some(PlannedCheck {
        version: PLANNED_CHECK_VERSION,
        command: command.to_string(),
        working_directory: String::new(),
        purpose: "validates the final tree".to_string(),
        required: true,
    });
    step
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

fn ask(
    engine: &WorkspaceEngine,
    repo: &Path,
    prompt: &str,
    adapter: &mut dyn ModelAdapter,
) -> ChatTurnResult {
    let cancel = CancelToken::new();
    let mut on_token = |_token: &str| {};
    let mut on_progress = |_event: TurnProgress| {};
    let mut sink = sink(&cancel, &mut on_token, &mut on_progress);
    engine
        .chat_orchestrator
        .ask_with_session(repo, prompt, &[], None, adapter, &mut sink)
        .expect("the turn should run")
}

/// Drives a turn through tool-call rounds, ending with a plain answer so the
/// loop terminates rather than running to the round limit.
fn scripted(rounds: Vec<Vec<ToolCall>>) -> MockModelAdapter {
    let mut responses: Vec<String> = rounds.iter().map(|_| String::new()).collect();
    let mut calls = rounds;
    responses.push("Done.".to_string());
    calls.push(Vec::new());
    MockModelAdapter::new_sequence_with_tool_calls(responses, calls)
}

fn plan_call(titles: &[&str]) -> ToolCall {
    let steps = titles
        .iter()
        .map(|title| format!(r#"{{"title":"{title}"}}"#))
        .collect::<Vec<_>>()
        .join(",");
    call("propose_plan", &format!(r#"{{"steps":[{steps}]}}"#))
}

fn check_call() -> ToolCall {
    call(
        "run_command",
        r#"{"command":"./check","reason":"Run the repository check"}"#,
    )
}

fn complete_step_call() -> ToolCall {
    call("complete_step", "{}")
}

fn complete_step_call_with_id(id: &str) -> ToolCall {
    ToolCall {
        id: id.to_string(),
        name: "complete_step".to_string(),
        arguments_json: "{}".to_string(),
    }
}

fn approve_plan(
    engine: &WorkspaceEngine,
    proposal_id: &str,
    adapter: &mut dyn ModelAdapter,
) -> ChatTurnResult {
    let cancel = CancelToken::new();
    let mut on_token = |_token: &str| {};
    let mut on_progress = |_event: TurnProgress| {};
    let mut sink = sink(&cancel, &mut on_token, &mut on_progress);
    engine
        .chat_orchestrator
        .resume_after_plan_decision(proposal_id, true, None, "tester", adapter, &mut sink)
        .expect("the plan decision should resume the turn")
}

fn approve_command(
    engine: &WorkspaceEngine,
    proposal_id: &str,
    adapter: &mut dyn ModelAdapter,
) -> ChatTurnResult {
    let cancel = CancelToken::new();
    let mut on_token = |_token: &str| {};
    let mut on_progress = |_event: TurnProgress| {};
    let mut sink = sink(&cancel, &mut on_token, &mut on_progress);
    engine
        .chat_orchestrator
        .resume_after_command_decision(proposal_id, true, "tester", adapter, &mut sink)
        .expect("the command decision should resume the turn")
}

/// A repository whose `./check` fails once and then passes, by touching
/// `.fixed` on the failing run. Both runs need the normal command decision.
fn write_recovering_check(repo: &Path) {
    fs::write(
        repo.join("check"),
        "#!/bin/sh\nif [ -f .fixed ]; then exit 0; fi\ntouch .fixed\nexit 1\n",
    )
    .expect("the check script should be written");
    fs::set_permissions(repo.join("check"), fs::Permissions::from_mode(0o755))
        .expect("the check script should be executable");
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

/// R2 regression from the 2026-11-04 review of specs 101–105: a source task
/// can settle `incomplete` with only an unmet required check while every plan
/// step is already terminal. Continue used to write `plan_resumed` only when a
/// step was non-terminal, so that continuation carried no plan, no required
/// check and no verification chain; the model's next answer settled `complete`
/// without running anything.
#[test]
fn continuing_a_task_with_only_a_missing_required_check_keeps_the_check() {
    let repo = temp_repo("continue-required-check");
    let engine = engine_for(&repo);
    let store = &engine.session_store;
    let session = store
        .create_session(&repository_id_for_root(&repo), "Required check")
        .unwrap();
    let task = store
        .create_task(&session.id, "Run the check", "mock", "mock")
        .unwrap();
    let mut plan = TaskPlan::new(&task.id, 0);
    plan.steps = vec![checked_step("test", "cargo test", StepStatus::Completed)];
    store.create_plan(&task, &plan).unwrap();
    let report = CompletionGateService::new(store, &repo)
        .report(&session.id, &task.id)
        .unwrap();
    assert_eq!(
        report.reason_codes(),
        vec![CompletionReasonCode::RequiredCheckMissing],
        "the source task has one unmet required check and no open step: {report:?}"
    );
    store
        .update_task_status_with_completion(&task, TaskStatus::Incomplete, &report)
        .unwrap();
    let mut adapter = MockModelAdapter::new("The task is done.");

    let result = continue_task(&engine, &repo, &session.id, &task.id, None, &mut adapter)
        .expect("the engine authorizes the continuation");

    assert_eq!(
        result.task.status,
        TaskStatus::Incomplete,
        "answering without the check must not close the continued task"
    );
    let carried = store
        .read_task_plan(&session.id, &result.task.id)
        .unwrap()
        .expect("the source plan is carried even though every step is terminal");
    assert_eq!(carried.steps[0].status, StepStatus::Completed);
    assert!(
        carried.steps[0].check.is_some(),
        "the carried plan still declares its required check: {carried:?}"
    );
    let continued_report = store
        .read_completion_report(&session.id, &result.task.id)
        .unwrap()
        .expect("the continued task settles through the gate");
    assert_eq!(
        continued_report.reason_codes(),
        vec![CompletionReasonCode::RequiredCheckMissing],
        "{continued_report:?}"
    );
}

/// R2 regression: a source task stopped by blocked plan steps must carry the
/// blocked plan into the continuation. Before R2 the plan was only carried when
/// a step was non-terminal, so `Blocked` (terminal but unfinished) lost the
/// plan and the gate returned eligible.
#[test]
fn continuing_a_task_stopped_by_a_blocked_step_carries_the_blocked_plan() {
    let repo = temp_repo("continue-blocked");
    let engine = engine_for(&repo);
    let store = &engine.session_store;
    let session = store
        .create_session(&repository_id_for_root(&repo), "Blocked step")
        .unwrap();
    let task = store
        .create_task(&session.id, "Run the tests", "mock", "mock")
        .unwrap();
    let mut plan = TaskPlan::new(&task.id, 0);
    plan.steps = vec![step("test", StepStatus::Blocked)];
    store.create_plan(&task, &plan).unwrap();
    let report = CompletionGateService::new(store, &repo)
        .report(&session.id, &task.id)
        .unwrap();
    assert_eq!(
        report.reason_codes(),
        vec![CompletionReasonCode::BlockedPlanSteps],
        "{report:?}"
    );
    store
        .update_task_status_with_completion(&task, TaskStatus::Incomplete, &report)
        .unwrap();
    let mut adapter = MockModelAdapter::new("The task is done.");

    let result = continue_task(&engine, &repo, &session.id, &task.id, None, &mut adapter)
        .expect("the engine authorizes the continuation");

    assert!(
        !result.task.user_prompt.contains("No plan step is open"),
        "the continuation prompt must not deny the carried plan: {}",
        result.task.user_prompt
    );
    let carried = store
        .read_task_plan(&session.id, &result.task.id)
        .unwrap()
        .expect("the blocked plan is carried");
    assert_eq!(carried.steps[0].status, StepStatus::Blocked);
    let continued_report = store
        .read_completion_report(&session.id, &result.task.id)
        .unwrap()
        .expect("the continued task settles through the gate");
    assert_eq!(
        continued_report.reason_codes(),
        vec![CompletionReasonCode::BlockedPlanSteps],
        "the continued task's gate still sees the blocked step: {continued_report:?}"
    );
}

/// Drives the source turn end to end: propose a plan, approve it, run the
/// failing check, approve the command, and close the step so its evidence
/// leaves it blocked. The source task settles `incomplete` with
/// `blocked_plan_steps`.
fn seed_incomplete_with_a_blocked_command(
    engine: &WorkspaceEngine,
    repo: &Path,
) -> (String, String) {
    let mut first = scripted(vec![
        vec![plan_call(&["Run the check", "Report"])],
        vec![check_call()],
    ]);
    let paused = ask(engine, repo, "Check the folder", &mut first);
    let review = paused
        .plan_proposal
        .expect("the mutating check puts the plan up for review");

    // The plan decision asks the model again, and the command needs its own
    // normal one-shot approval.
    let mut command_round = scripted(vec![vec![check_call()]]);
    let paused = approve_plan(engine, &review.id, &mut command_round);
    let command = paused
        .command_proposal
        .expect("the check needs command approval");

    let mut finish = scripted(vec![vec![complete_step_call()]]);
    let result = approve_command(engine, &command.id, &mut finish);

    assert_eq!(
        result.task.status,
        TaskStatus::Incomplete,
        "the failing command must settle the source task incomplete: {result:?}"
    );
    let plan = engine
        .session_store
        .read_task_plan(&result.session.id, &result.task.id)
        .unwrap()
        .expect("the source plan reads");
    assert_eq!(plan.steps[0].status, StepStatus::Blocked);
    assert_eq!(plan.steps[1].status, StepStatus::Pending);
    let report = engine
        .session_store
        .read_completion_report(&result.session.id, &result.task.id)
        .unwrap()
        .expect("the source task carries its gate report");
    assert!(
        report
            .reason_codes()
            .contains(&CompletionReasonCode::BlockedPlanSteps),
        "{report:?}"
    );
    (result.session.id, result.task.id)
}

/// R2 follow-up: a continuation must not only carry a blocked step; the model
/// must be able to clear it by rerunning the same command after the cause is
/// fixed, close the plan, and settle `complete`. Both command runs go through
/// the ordinary approval decision.
#[test]
fn a_continued_blocked_step_recovers_when_the_same_command_passes() {
    let repo = temp_repo("continue-blocked-recovers");
    write_recovering_check(&repo);
    let engine = engine_for(&repo);
    let (session_id, source_task_id) = seed_incomplete_with_a_blocked_command(&engine, &repo);

    // The first run touched `.fixed`, so this run exits 0. It still asks for
    // its own command approval; the carried plan is already approved.
    let mut request = scripted(vec![vec![check_call()]]);
    let paused = continue_task(
        &engine,
        &repo,
        &session_id,
        &source_task_id,
        None,
        &mut request,
    )
    .expect("the engine authorizes the continuation");
    assert!(
        paused.plan_proposal.is_none(),
        "the carried plan is already approved"
    );
    let command = paused
        .command_proposal
        .expect("the rerun needs its own command approval");

    let mut finish = scripted(vec![vec![complete_step_call()], vec![complete_step_call()]]);
    let result = approve_command(&engine, &command.id, &mut finish);

    assert_eq!(
        result.task.status,
        TaskStatus::Complete,
        "rerunning the same command and closing both steps must complete the carried task"
    );
    let plan = engine
        .session_store
        .read_task_plan(&session_id, &result.task.id)
        .unwrap()
        .expect("the carried plan stays attached");
    assert_eq!(plan.steps[0].status, StepStatus::Completed);
    assert_eq!(plan.steps[1].status, StepStatus::Completed);
    let report = engine
        .session_store
        .read_completion_report(&session_id, &result.task.id)
        .unwrap()
        .expect("the completed task carries its gate report");
    assert!(report.eligible, "{report:?}");
    assert!(report.reasons.is_empty(), "{report:?}");
}

/// The other side: a continuation that calls `complete_step` without rerunning
/// the failed command cannot clear the block, and the task stays incomplete
/// with `blocked_plan_steps`.
#[test]
fn a_continued_blocked_step_stays_blocked_without_rerunning_the_command() {
    let repo = temp_repo("continue-blocked-stays");
    write_recovering_check(&repo);
    let engine = engine_for(&repo);
    let (session_id, source_task_id) = seed_incomplete_with_a_blocked_command(&engine, &repo);

    let mut adapter = scripted(vec![vec![complete_step_call()]]);
    let result = continue_task(
        &engine,
        &repo,
        &session_id,
        &source_task_id,
        None,
        &mut adapter,
    )
    .expect("the engine authorizes the continuation");

    assert_eq!(result.task.status, TaskStatus::Incomplete);
    let plan = engine
        .session_store
        .read_task_plan(&session_id, &result.task.id)
        .unwrap()
        .expect("the carried plan stays attached");
    assert_eq!(
        plan.steps[0].status,
        StepStatus::Blocked,
        "complete_step without the same command passing leaves the step blocked"
    );
    assert_eq!(plan.steps[1].status, StepStatus::Pending);
    let report = engine
        .session_store
        .read_completion_report(&session_id, &result.task.id)
        .unwrap()
        .expect("the continued task carries its gate report");
    assert!(
        report
            .reason_codes()
            .contains(&CompletionReasonCode::BlockedPlanSteps),
        "{report:?}"
    );
}

fn patch_call(path: &str) -> ToolCall {
    call(
        "propose_patch",
        &format!(
            r#"{{"summary":"Add {path}","files":[{{"path":"{path}","content":"pub fn added() {{}}\n"}}]}}"#
        ),
    )
}

fn mark_as_worker(engine: &WorkspaceEngine, repo: &Path, session_id: &str) {
    engine
        .session_store
        .record_worker_spawned(
            session_id,
            &WorkerRecord {
                parent_session_id: None,
                parent_task_id: None,
                inherited_protected_paths: Vec::new(),
                source_repository: repo.to_path_buf(),
                worktree_path: repo.to_path_buf(),
                branch: "main".to_string(),
                base_commit: "0000000".to_string(),
                included_uncommitted: false,
                git_commands: Vec::new(),
            },
        )
        .expect("the worker record should be written");
}

fn tool_results(engine: &WorkspaceEngine, session_id: &str) -> Vec<String> {
    engine
        .session_store
        .read_messages(session_id)
        .expect("the transcript reads")
        .into_iter()
        .filter(|message| message.role == "tool")
        .map(|message| message.content)
        .collect()
}

/// R9 regression: a continuation carries an incomplete task's open plan. A
/// model `propose_plan` in that run used to replace it because
/// `carried_plan_untouched` was true, silently dropping the work the user
/// asked to continue. The replacement is now refused; the model is told to
/// work through the carried plan, and the user owns any change to its steps.
#[test]
fn a_continued_task_cannot_replace_its_carried_open_plan() {
    let repo = temp_repo("continue-replace-open");
    let engine = engine_for(&repo);
    let (session_id, source_task_id) = seed_incomplete(&engine, &repo, "Carried plan");

    let mut adapter = scripted(vec![
        vec![plan_call(&["Summarize", "Report"])],
        vec![complete_step_call_with_id("call_complete_first")],
        vec![complete_step_call_with_id("call_complete_second")],
    ]);
    let result = continue_task(
        &engine,
        &repo,
        &session_id,
        &source_task_id,
        None,
        &mut adapter,
    )
    .expect("the engine authorizes the continuation");

    let plan = engine
        .session_store
        .read_task_plan(&session_id, &result.task.id)
        .unwrap()
        .expect("the carried plan stays attached");
    assert_eq!(
        plan.steps
            .iter()
            .map(|step| step.id.as_str())
            .collect::<Vec<_>>(),
        vec!["read", "write"],
        "the model's smaller plan must not replace the carried one"
    );
    assert!(
        tool_results(&engine, &session_id)
            .iter()
            .any(|content| content.contains("carried") && content.contains("complete_step")),
        "the model is told to work from the carried plan: {:?}",
        tool_results(&engine, &session_id)
    );
    assert!(
        !engine
            .session_store
            .read_plan_approved(&session_id, &result.task.id)
            .unwrap(),
        "Continue grants no approval"
    );
}

/// R9 regression for the blocked case: the replacement used to lose the
/// blocked step, so `complete_step` closed the new one and the task settled
/// `complete`. The refusal keeps the blocked step, and the gate keeps the task
/// incomplete.
#[test]
fn a_continued_task_cannot_replace_its_carried_blocked_plan() {
    let repo = temp_repo("continue-replace-blocked");
    write_recovering_check(&repo);
    let engine = engine_for(&repo);
    let (session_id, source_task_id) = seed_incomplete_with_a_blocked_command(&engine, &repo);

    let mut adapter = scripted(vec![
        vec![plan_call(&["Summarize", "Report"])],
        vec![complete_step_call_with_id("call_complete_first")],
        vec![complete_step_call_with_id("call_complete_second")],
    ]);
    let result = continue_task(
        &engine,
        &repo,
        &session_id,
        &source_task_id,
        None,
        &mut adapter,
    )
    .expect("the engine authorizes the continuation");

    assert_eq!(
        result.task.status,
        TaskStatus::Incomplete,
        "the blocked step is still outstanding"
    );
    let plan = engine
        .session_store
        .read_task_plan(&session_id, &result.task.id)
        .unwrap()
        .expect("the carried plan stays attached");
    assert_eq!(plan.steps.len(), 2, "the carried plan keeps both steps");
    assert_eq!(plan.steps[0].title, "Run the check");
    assert_eq!(plan.steps[0].status, StepStatus::Blocked);
    assert_eq!(plan.steps[1].status, StepStatus::Pending);
    let report = engine
        .session_store
        .read_completion_report(&session_id, &result.task.id)
        .unwrap()
        .expect("the continued task settles through the gate");
    assert!(
        report
            .reason_codes()
            .contains(&CompletionReasonCode::BlockedPlanSteps),
        "{report:?}"
    );
}

/// The same regression in the mode a worker uses. A worker continuation gets
/// `ChatTurnOptions::worker()`, not an interactive turn, and must be protected
/// identically.
#[test]
fn a_worker_continued_task_cannot_replace_its_carried_blocked_plan() {
    let repo = temp_repo("continue-replace-blocked-worker");
    write_recovering_check(&repo);
    let engine = engine_for(&repo);
    let (session_id, source_task_id) = seed_incomplete_with_a_blocked_command(&engine, &repo);
    mark_as_worker(&engine, &repo, &session_id);

    let mut adapter = scripted(vec![
        vec![plan_call(&["Summarize", "Report"])],
        vec![complete_step_call_with_id("call_complete_first")],
        vec![complete_step_call_with_id("call_complete_second")],
    ]);
    let result = continue_task(
        &engine,
        &repo,
        &session_id,
        &source_task_id,
        None,
        &mut adapter,
    )
    .expect("the engine authorizes the continuation");

    assert_eq!(result.task.status, TaskStatus::Incomplete);
    let plan = engine
        .session_store
        .read_task_plan(&session_id, &result.task.id)
        .unwrap()
        .expect("the carried plan stays attached");
    assert_eq!(plan.steps.len(), 2);
    assert_eq!(plan.steps[0].title, "Run the check");
    assert_eq!(plan.steps[0].status, StepStatus::Blocked);
    assert!(
        tool_results(&engine, &session_id)
            .iter()
            .any(|content| content.contains("carried") && content.contains("complete_step")),
        "the worker is told to work from the carried plan"
    );
}

/// Guard: the refusal is about dropping carried work or an unmet check, not
/// about forbidding new work. A continued task whose carried plan has nothing
/// outstanding may still propose the next plan, and that plan goes through the
/// ordinary review before its first mutating action.
#[test]
fn a_continued_task_without_open_work_can_propose_a_plan_that_is_reviewed() {
    let repo = temp_repo("continue-add-steps");
    fs::write(repo.join("Cargo.toml"), "[workspace]\nmembers = []\n")
        .expect("the repository manifest should be written");
    let engine = engine_for(&repo);
    let store = &engine.session_store;
    let session = store
        .create_session(&repository_id_for_root(&repo), "Add steps")
        .unwrap();
    let task = store
        .create_task(&session.id, "Run the check", "mock", "mock")
        .unwrap();
    let mut plan = TaskPlan::new(&task.id, 0);
    plan.steps = vec![step("done", StepStatus::Completed)];
    store.create_plan(&task, &plan).unwrap();
    // The task changed a file, so the completion floor requires the detected
    // check. Nothing about that obligation lives in the plan, and the floor
    // follows the continuation chain, so a replacement plan may not hide it.
    store
        .append_patch_file_decision(
            &session.id,
            &PatchFileDecision {
                task_id: task.id.clone(),
                patch_id: "patch_applied".to_string(),
                paths: vec!["src/a.rs".to_string()],
                applied: true,
                applied_hashes: Default::default(),
                accepted_hunks: Default::default(),
            },
        )
        .unwrap();
    // The gate reads only the persisted map; it never builds one. Build it here
    // the way indexing does, so the fixture has a real floor.
    let index = engine
        .indexer
        .index_repository(&repo)
        .expect("the fixture indexes");
    workspace_engine::RepositoryMapStore::new(&engine.config.data_dir, engine.audit_log.clone())
        .load_or_build(&index, &engine.command_policy)
        .expect("the repository map builds");
    let report = CompletionGateService::new(store, &repo)
        .report(&session.id, &task.id)
        .unwrap();
    assert_eq!(
        report.reason_codes(),
        vec![CompletionReasonCode::RequiredCheckMissing],
        "the source task is incomplete on the floor check alone: {report:?}"
    );
    store
        .update_task_status_with_completion(&task, TaskStatus::Incomplete, &report)
        .unwrap();

    let mut adapter = scripted(vec![
        vec![plan_call(&["Add docs", "Check docs"])],
        vec![patch_call("src/docs.rs")],
    ]);
    let result = continue_task(&engine, &repo, &session.id, &task.id, None, &mut adapter)
        .expect("the engine authorizes the continuation");

    let replaced = store
        .read_task_plan(&session.id, &result.task.id)
        .unwrap()
        .expect("the new plan is recorded");
    assert_eq!(
        replaced
            .steps
            .iter()
            .map(|step| step.title.as_str())
            .collect::<Vec<_>>(),
        vec!["Add docs", "Check docs"],
        "nothing outstanding was dropped, so the model may add the next steps"
    );
    assert!(
        result.plan_proposal.is_some(),
        "the new plan still needs the user's review before its first change"
    );
    assert!(
        result.patch_proposal.is_none(),
        "the patch waits behind the plan review"
    );
}

/// Guard: the restriction is specific to a continuation's carried plan. A
/// fresh task's first `propose_plan` is unchanged, and proposing it is not an
/// approval.
#[test]
fn a_fresh_tasks_first_propose_plan_is_unchanged() {
    let repo = temp_repo("fresh-first-plan");
    let engine = engine_for(&repo);
    let mut adapter = scripted(vec![vec![plan_call(&["Read", "Write"])]]);

    let result = ask(&engine, &repo, "Plan the work", &mut adapter);

    let plan = engine
        .session_store
        .read_task_plan(&result.session.id, &result.task.id)
        .unwrap()
        .expect("the fresh plan is recorded");
    assert_eq!(plan.steps.len(), 2);
    assert_eq!(plan.steps[0].status, StepStatus::InProgress);
    assert_eq!(plan.steps[1].status, StepStatus::Pending);
    assert!(
        !engine
            .session_store
            .read_plan_approved(&result.session.id, &result.task.id)
            .unwrap(),
        "proposing a plan never approves it"
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
