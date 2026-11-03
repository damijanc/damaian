//! Automatic bounded worker segments, per
//! `docs/specs/103_bounded_worker_continuation.md` Task 5.
//!
//! A worker whose plan carries an approved `TaskExecutionBudget` crosses the
//! ordinary round checkpoint internally while every approved total allows
//! progress, the engine-owned digest advanced, and the next request fits. A
//! worker without a budget, an inline task and a historical pending turn keep
//! the explicit round-budget pause byte-for-byte.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use workspace_engine::model::model_request_json;
use workspace_engine::{
    CancelToken, ChatTurnOptions, ChatTurnResult, ClientError, Config, MockModelAdapter,
    ModelProviderConfig, PhaseKind, Session, TaskExecutionBudget, TaskStatus, ToolCall,
    TurnProgress, TurnSink, WithinTurnCompactionAttemptOutcome, WorkerRecord, WorkspaceEngine,
    repository_id_for_root,
};

static COUNTER: AtomicU64 = AtomicU64::new(1);

fn temp_repo(name: &str) -> PathBuf {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock should work")
        .as_nanos();
    let repo = std::env::temp_dir().join(format!(
        "damaian-task-segments-{name}-{now}-{}",
        COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir_all(repo.join("src")).expect("repository should be created");
    fs::write(repo.join("src/a.rs"), "fn main() {}\n").expect("a file to read");
    repo
}

fn native_tool_provider(context_window: Option<u32>) -> ModelProviderConfig {
    ModelProviderConfig {
        id: "openai".to_string(),
        label: "OpenAI".to_string(),
        base_url: String::new(),
        api_key_env: String::new(),
        models: Vec::new(),
        supports_native_tools: true,
        max_output_tokens: None,
        context_token_budget: None,
        context_window,
        provider_reports_usage: true,
        price_per_million_input_tokens: None,
        price_per_million_output_tokens: None,
        price_per_million_cached_input_tokens: None,
        supports_explicit_cache_breakpoints: false,
    }
}

fn engine_for(repo: &Path, adjust: impl FnOnce(&mut Config)) -> WorkspaceEngine {
    let mut config = Config {
        data_dir: repo.join(".damaian"),
        agent_max_tool_rounds: 2,
        agent_max_exploration_rounds: 2,
        enable_index_watcher: false,
        ..Config::default()
    };
    adjust(&mut config);
    WorkspaceEngine::new(config)
}

fn worker_session(engine: &WorkspaceEngine, repo: &Path) -> Session {
    let session = engine
        .session_store
        .create_session(&repository_id_for_root(repo), "Worker")
        .expect("a worker session should be created");
    engine
        .session_store
        .record_worker_spawned(
            &session.id,
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
    session
}

fn call(id: &str, name: &str, arguments_json: &str) -> ToolCall {
    ToolCall {
        id: id.to_string(),
        name: name.to_string(),
        arguments_json: arguments_json.to_string(),
    }
}

fn read_call(id: &str, path: &str) -> ToolCall {
    call(id, "read_file", &format!(r#"{{"path":"{path}"}}"#))
}

fn propose_plan_and_patch() -> MockModelAdapter {
    MockModelAdapter::new_sequence_with_tool_calls(
        vec![String::new(), String::new()],
        vec![
            vec![call(
                "call_plan",
                "propose_plan",
                r#"{"steps":[{"title":"Read the retry helper"},{"title":"Add a bounded backoff"}]}"#,
            )],
            vec![call(
                "call_patch",
                "propose_patch",
                r#"{"summary":"Add src/retry.rs","files":[{"path":"src/retry.rs","content":"pub fn added() {}\n"}]}"#,
            )],
        ],
    )
}

fn read_sequence(repo: &Path, count: usize) -> MockModelAdapter {
    read_sequence_then(repo, count, 0)
}

fn read_sequence_then(repo: &Path, count: usize, complete_steps: usize) -> MockModelAdapter {
    let mut responses = Vec::new();
    let mut calls = Vec::new();
    for index in 1..=count {
        responses.push(String::new());
        let path = format!("src/f{index}.rs");
        fs::write(repo.join(&path), format!("fn f{index}() {{}}\n"))
            .expect("a readable fixture file");
        calls.push(vec![read_call(&format!("call_read_{index}"), &path)]);
    }
    for index in 0..complete_steps {
        responses.push(String::new());
        calls.push(vec![call(
            &format!("call_complete_{index}"),
            "complete_step",
            "{}",
        )]);
    }
    responses.push("Done.".to_string());
    calls.push(Vec::new());
    MockModelAdapter::new_sequence_with_tool_calls(responses, calls)
}

/// A round of one distinct read plus `complete_step`. `complete_step` is a
/// turn-shaping call rather than a read, so each round is charged as an action
/// round, while the distinct read keeps the progress digest advancing after the
/// plan's own steps are exhausted.
fn action_round_sequence(repo: &Path, count: usize) -> MockModelAdapter {
    let mut responses = Vec::new();
    let mut calls = Vec::new();
    for index in 1..=count {
        responses.push(String::new());
        let path = format!("src/a{index}.rs");
        fs::write(repo.join(&path), format!("fn a{index}() {{}}\n"))
            .expect("a readable fixture file");
        calls.push(vec![
            read_call(&format!("call_read_{index}"), &path),
            call(&format!("call_complete_{index}"), "complete_step", "{}"),
        ]);
    }
    responses.push("Done.".to_string());
    calls.push(Vec::new());
    MockModelAdapter::new_sequence_with_tool_calls(responses, calls)
}

fn ask_with_options(
    engine: &WorkspaceEngine,
    repo: &Path,
    session_id: Option<&str>,
    adapter: &mut MockModelAdapter,
    options: ChatTurnOptions,
) -> workspace_engine::Result<ChatTurnResult> {
    let cancel = CancelToken::new();
    let mut on_token = |_token: &str| {};
    let mut on_progress = |_event: TurnProgress| {};
    let mut sink = TurnSink {
        on_token: &mut on_token,
        on_progress: &mut on_progress,
        cancel: &cancel,
        steering: None,
    };
    engine.chat_orchestrator.ask_with_session_with_options(
        repo,
        "Add retry handling",
        &[],
        session_id,
        adapter,
        &mut sink,
        options,
    )
}

fn decide_with(
    engine: &WorkspaceEngine,
    proposal_id: &str,
    budget: &TaskExecutionBudget,
    adapter: &mut MockModelAdapter,
) -> workspace_engine::Result<ChatTurnResult> {
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
        .resume_after_plan_decision_with_budget(
            proposal_id,
            true,
            None,
            Some(budget),
            "tester",
            adapter,
            &mut sink,
        )
}

fn decide_with_steering(
    engine: &WorkspaceEngine,
    proposal_id: &str,
    budget: &TaskExecutionBudget,
    adapter: &mut MockModelAdapter,
    steering: &workspace_engine::TurnSteering,
    on_progress: &mut dyn FnMut(TurnProgress),
) -> workspace_engine::Result<ChatTurnResult> {
    let cancel = CancelToken::new();
    let mut on_token = |_token: &str| {};
    let mut sink = TurnSink {
        on_token: &mut on_token,
        on_progress,
        cancel: &cancel,
        steering: Some(steering),
    };
    engine
        .chat_orchestrator
        .resume_after_plan_decision_with_budget(
            proposal_id,
            true,
            None,
            Some(budget),
            "tester",
            adapter,
            &mut sink,
        )
}

fn resume_command(
    engine: &WorkspaceEngine,
    proposal_id: &str,
) -> workspace_engine::Result<ChatTurnResult> {
    let cancel = CancelToken::new();
    let mut on_token = |_token: &str| {};
    let mut on_progress = |_event: TurnProgress| {};
    let mut sink = TurnSink {
        on_token: &mut on_token,
        on_progress: &mut on_progress,
        cancel: &cancel,
        steering: None,
    };
    engine.chat_orchestrator.resume_after_command_decision(
        proposal_id,
        true,
        "tester",
        &mut MockModelAdapter::new("Done."),
        &mut sink,
    )
}

fn answer_question(
    engine: &WorkspaceEngine,
    question_id: &str,
    answer: &str,
) -> workspace_engine::Result<ChatTurnResult> {
    let cancel = CancelToken::new();
    let mut on_token = |_token: &str| {};
    let mut on_progress = |_event: TurnProgress| {};
    let mut sink = TurnSink {
        on_token: &mut on_token,
        on_progress: &mut on_progress,
        cancel: &cancel,
        steering: None,
    };
    engine.chat_orchestrator.resume_after_answer(
        question_id,
        answer,
        Some(0),
        &mut MockModelAdapter::new("Done."),
        &mut sink,
    )
}

fn log_events(engine: &WorkspaceEngine, session_id: &str) -> Vec<serde_json::Value> {
    let path = engine
        .config
        .data_dir
        .join("sessions")
        .join(format!("{session_id}.jsonl"));
    fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .filter_map(|line| serde_json::from_str(line).ok())
        .collect()
}

fn audit_events(engine: &WorkspaceEngine) -> Vec<serde_json::Value> {
    let path = engine.config.data_dir.join("audit").join("events.jsonl");
    fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .filter_map(|line| serde_json::from_str(line).ok())
        .collect()
}

fn event_type(event: &serde_json::Value) -> &str {
    event
        .get("eventType")
        .and_then(|value| value.as_str())
        .unwrap_or_default()
}

fn segment_events(engine: &WorkspaceEngine, session_id: &str) -> Vec<serde_json::Value> {
    log_events(engine, session_id)
        .into_iter()
        .filter(|event| matches!(event_type(event), "segment_started" | "segment_finished"))
        .collect()
}

fn payload(event: &serde_json::Value) -> &serde_json::Value {
    event.get("payload").expect("a payload")
}

fn payload_u64(event: &serde_json::Value, key: &str) -> u64 {
    payload(event)
        .get(key)
        .and_then(|value| value.as_u64())
        .unwrap_or_else(|| panic!("{key} should be a number in {event}"))
}

fn payload_text<'a>(event: &'a serde_json::Value, key: &str) -> &'a str {
    payload(event)
        .get(key)
        .and_then(|value| value.as_str())
        .unwrap_or_else(|| panic!("{key} should be a string in {event}"))
}

fn audit_text<'a>(event: &'a serde_json::Value, key: &str) -> &'a str {
    event
        .get(key)
        .and_then(|value| value.as_str())
        .unwrap_or_else(|| panic!("{key} should be a string in {event}"))
}

fn pending_json(engine: &WorkspaceEngine, proposal_id: &str) -> serde_json::Value {
    let path = engine
        .config
        .data_dir
        .join("chat")
        .join("pending")
        .join(format!("{proposal_id}.json"));
    serde_json::from_str(&fs::read_to_string(path).expect("pending turn should exist"))
        .expect("pending turn should be valid JSON")
}

/// Creates the plan-review pause and returns the proposal id, session and the
/// exact budget the engine offered (the only value an approval may echo).
fn plan_review(
    engine: &WorkspaceEngine,
    repo: &Path,
    options: ChatTurnOptions,
) -> (String, Session, TaskExecutionBudget) {
    let session = worker_session(engine, repo);
    let result = ask_with_options(
        engine,
        repo,
        Some(&session.id),
        &mut propose_plan_and_patch(),
        options,
    )
    .expect("the turn should pause on its plan review");
    let proposal = result
        .plan_proposal
        .expect("the worker plan is reviewed before any patch");
    let budget = proposal
        .budget_review
        .expect("a genuine worker is offered a budget")
        .budget;
    (proposal.id, session, budget)
}

#[test]
fn a_budgeted_worker_crosses_three_segments_and_completes() {
    let repo = temp_repo("three-segments");
    // The ordinary default segment limits, not a fixture-sized 2. The script
    // must genuinely need more than sixteen action rounds and several internal
    // segments after the one budget approval.
    let engine = engine_for(&repo, |config| {
        config.agent_max_tool_rounds = 16;
        config.agent_max_exploration_rounds = 24;
    });
    let (proposal_id, session, approved) = plan_review(&engine, &repo, ChatTurnOptions::default());
    assert!(
        approved.action_rounds > 16,
        "the approved task total must exceed one segment"
    );
    let mut adapter = action_round_sequence(&repo, 70);
    let result = decide_with(&engine, &proposal_id, &approved, &mut adapter)
        .expect("the budgeted turn should complete across its segments");

    assert_eq!(
        result.task.status,
        TaskStatus::Complete,
        "response={:?} proposal={:?} events={:#?}",
        result.response,
        result.round_budget_proposal,
        segment_events(&engine, &session.id)
    );
    assert!(
        result.round_budget_proposal.is_none(),
        "an approved budget must not surface a generic Continue"
    );
    for request in &adapter.requests {
        for message in &request.messages {
            assert!(
                !message.content.contains("tool-round segment ended"),
                "a budgeted continuation must not add the synthetic Continue message"
            );
        }
    }

    let events = segment_events(&engine, &session.id);
    let started: Vec<_> = events
        .iter()
        .filter(|event| event_type(event) == "segment_started")
        .collect();
    let finished: Vec<_> = events
        .iter()
        .filter(|event| event_type(event) == "segment_finished")
        .collect();
    assert!(
        started.len() >= 4,
        "the worker should cross at least three internal segments, got {}: {events:#?}",
        started.len()
    );
    assert_eq!(
        finished.len(),
        started.len() - 1,
        "one finished event per committed boundary, and one open segment"
    );
    let first_finished = *finished.first().expect("at least one boundary");
    assert_eq!(
        payload_u64(first_finished, "segmentActionRounds"),
        16,
        "the fixture must use the ordinary segment limit, not a shrunken one"
    );
    let last_finished = *finished.last().expect("at least one boundary");
    assert!(
        payload_u64(last_finished, "totalActionRounds") > 64,
        "the approved task total must carry action rounds past the legacy turn cap"
    );

    // Exact start/finish pairing, monotonic totals, exact per-segment counts,
    // deterministic digests and token provenance.
    let mut last_action: Option<u64> = None;
    let mut last_exploration: Option<u64> = None;
    let mut last_input = 0;
    let mut last_output = 0;
    let mut open_started: Option<&serde_json::Value> = None;
    for event in &events {
        match event_type(event) {
            "segment_started" => {
                assert!(
                    open_started.replace(event).is_none(),
                    "a started event was written while a segment was already open"
                );
                assert_eq!(payload_u64(event, "segmentActionRounds"), 0);
                assert_eq!(payload_u64(event, "segmentExplorationRounds"), 0);
                assert_eq!(payload_text(event, "startDigest").len(), 64);
                let total_action = payload_u64(event, "totalActionRounds");
                let total_exploration = payload_u64(event, "totalExplorationRounds");
                if let Some(previous) = last_action {
                    assert_eq!(
                        total_action, previous,
                        "a new segment starts at the previous total"
                    );
                }
                if let Some(previous) = last_exploration {
                    assert_eq!(total_exploration, previous);
                }
                last_action = Some(total_action);
                last_exploration = Some(total_exploration);
            }
            "segment_finished" => {
                let opened = open_started
                    .take()
                    .expect("a finished event must close an open segment");
                assert_eq!(
                    payload(event).get("segment"),
                    payload(opened).get("segment"),
                    "the finished event names the segment it closes"
                );
                assert_eq!(
                    payload_text(event, "startDigest"),
                    payload_text(opened, "startDigest")
                );
                assert_ne!(
                    payload_text(event, "progressDigest"),
                    payload_text(event, "startDigest"),
                    "a committed boundary advanced the digest"
                );
                let action = payload_u64(event, "segmentActionRounds");
                let exploration = payload_u64(event, "segmentExplorationRounds");
                let total_action = payload_u64(event, "totalActionRounds");
                let total_exploration = payload_u64(event, "totalExplorationRounds");
                assert_eq!(total_action, last_action.unwrap() + action);
                assert_eq!(total_exploration, last_exploration.unwrap() + exploration);
                let tokens = payload(event).get("tokens").expect("tokens record");
                let input = tokens
                    .get("inputTokens")
                    .and_then(|value| value.as_u64())
                    .unwrap();
                let output = tokens
                    .get("outputTokens")
                    .and_then(|value| value.as_u64())
                    .unwrap();
                assert!(input > last_input, "token totals never reset");
                assert!(output >= last_output, "token totals never reset");
                assert_eq!(
                    tokens.get("source").and_then(|value| value.as_str()),
                    Some("estimated"),
                    "the mock reports no measured usage"
                );
                last_action = Some(total_action);
                last_exploration = Some(total_exploration);
                last_input = input;
                last_output = output;
            }
            _ => unreachable!(),
        }
    }
    assert!(
        open_started.is_some(),
        "the completed turn leaves its last segment open"
    );

    // Content safety: the durable segment payloads carry numbers, ids, enums
    // and digests only.
    let raw = serde_json::to_string(&events).expect("events serialize");
    for forbidden in [
        "src/f1.rs",
        "src/f12.rs",
        "Add retry handling",
        "propose_patch",
        "Read the retry helper",
    ] {
        assert!(
            !raw.contains(forbidden),
            "segment events leaked {forbidden}"
        );
    }
}

#[test]
fn repeating_the_same_typed_calls_is_not_progress() {
    let repo = temp_repo("no-progress");
    let engine = engine_for(&repo, |config| {
        config.agent_max_tool_rounds = 2;
        config.agent_max_exploration_rounds = 1;
    });
    let (proposal_id, session, approved) = plan_review(&engine, &repo, ChatTurnOptions::default());
    // Four distinct reads, then the same four repeated. The first boundary
    // advances; a later boundary repeats the identical typed calls and must not
    // start another segment.
    let mut responses = Vec::new();
    let mut calls = Vec::new();
    for cycle in 0..3 {
        for index in 1..=4 {
            responses.push(String::new());
            calls.push(vec![read_call(
                &format!("call_{cycle}_{index}"),
                &format!("src/f{index}.rs"),
            )]);
        }
    }
    responses.push("Done.".to_string());
    calls.push(Vec::new());
    let mut adapter = MockModelAdapter::new_sequence_with_tool_calls(responses, calls);
    let result = decide_with(&engine, &proposal_id, &approved, &mut adapter)
        .expect("the turn should settle the no-progress outcome");

    assert_eq!(result.task.status, TaskStatus::TaskNoProgress);
    assert!(
        result.round_budget_proposal.is_none(),
        "a bounded stop must not become a generic round pause"
    );
    let outcome = result
        .task_control
        .terminal_outcome
        .as_ref()
        .expect("the no-progress outcome is typed");
    assert_eq!(
        outcome.kind,
        workspace_engine::TaskTerminalOutcomeKind::NoProgress
    );
    assert_eq!(
        result
            .turn_presentation
            .terminal_outcome
            .as_ref()
            .map(|outcome| outcome.kind),
        Some(workspace_engine::TaskTerminalOutcomeKind::NoProgress)
    );
    let events = segment_events(&engine, &session.id);
    let declined = audit_events(&engine)
        .into_iter()
        .filter(|event| event_type(event) == "segment_continuation_declined")
        .collect::<Vec<_>>();
    assert!(
        declined
            .iter()
            .any(|event| audit_text(event, "reason") == "no_progress"),
        "the engine should record a no-progress decline"
    );
    let started = events
        .iter()
        .filter(|event| event_type(event) == "segment_started")
        .count();
    let finished = events
        .iter()
        .filter(|event| event_type(event) == "segment_finished")
        .count();
    assert_eq!(started, finished + 1, "the declined segment stays open");
}

#[test]
fn a_smaller_budget_caps_the_allowance_before_the_ordinary_segment() {
    let repo = temp_repo("small-budget");
    let engine = engine_for(&repo, |config| {
        config.agent_max_tool_rounds = 8;
        config.agent_max_exploration_rounds = 1;
        config.worker_task_budget_action_rounds = 3;
        config.worker_task_budget_exploration_rounds = 3;
    });
    let (proposal_id, session, approved) = plan_review(&engine, &repo, ChatTurnOptions::default());
    let mut adapter = read_sequence(&repo, 6);
    let result = decide_with(&engine, &proposal_id, &approved, &mut adapter)
        .expect("the turn should settle at the approved total, not the ordinary segment");

    assert_eq!(result.task.status, TaskStatus::TaskBudgetExhausted);
    assert!(result.round_budget_proposal.is_none());
    let outcome = result
        .task_control
        .terminal_outcome
        .as_ref()
        .expect("the exhausted budget is a typed outcome");
    assert_eq!(
        outcome.kind,
        workspace_engine::TaskTerminalOutcomeKind::ApprovedBudgetExhausted
    );
    assert!(
        outcome.dimension.is_some(),
        "the exact limiting dimension must be named"
    );
    let events = segment_events(&engine, &session.id);
    assert_eq!(
        events
            .iter()
            .filter(|event| event_type(event) == "segment_started")
            .count(),
        1
    );
    assert_eq!(
        events
            .iter()
            .filter(|event| event_type(event) == "segment_finished")
            .count(),
        0
    );
}

#[test]
fn an_unbudgeted_worker_and_an_inline_task_keep_the_round_budget_pause() {
    for (label, worker) in [("worker", true), ("inline", false)] {
        let repo = temp_repo(label);
        let engine = engine_for(&repo, |config| {
            config.agent_max_tool_rounds = 1;
            config.agent_max_exploration_rounds = 5;
        });
        let session = worker.then(|| worker_session(&engine, &repo));
        let mut adapter = MockModelAdapter::new_sequence(vec![
            "DAMAIAN_COMMAND_V1\nCOMMAND: ls src\nREASON: Look.\nEND_COMMAND\n".to_string(),
            "DAMAIAN_COMMAND_V1\nCOMMAND: ls src\nREASON: Look.\nEND_COMMAND\n".to_string(),
        ]);
        let result = ask_with_options(
            &engine,
            &repo,
            session.as_ref().map(|session| session.id.as_str()),
            &mut adapter,
            ChatTurnOptions::default(),
        )
        .expect("the turn should pause");
        assert_eq!(result.task.status, TaskStatus::WaitingForApproval);
        assert!(
            result.round_budget_proposal.is_some(),
            "{label}: the explicit round budget stays"
        );
        let events = segment_events(
            &engine,
            session.as_ref().map_or(&result.session.id, |s| &s.id),
        );
        assert!(
            events.is_empty(),
            "{label}: no segment events without a budget"
        );
        let proposal = result.round_budget_proposal.unwrap();
        let pending = pending_json(&engine, &proposal.id);
        assert!(
            pending.get("segment").is_none(),
            "{label}: a budgetless pending turn keeps its old shape"
        );
    }
}

#[test]
fn a_command_pause_and_resume_keeps_the_same_segment() {
    let repo = temp_repo("command");
    let engine = engine_for(&repo, |_| {});
    let (proposal_id, session, approved) = plan_review(&engine, &repo, ChatTurnOptions::default());
    let mut adapter = MockModelAdapter::new_sequence_with_tool_calls(
        vec![String::new()],
        vec![vec![call(
            "call_command",
            "run_command",
            r#"{"command":"sed -i '' 's/1/2/' src/a.rs","reason":"Bump the constant."}"#,
        )]],
    );
    let paused = decide_with(&engine, &proposal_id, &approved, &mut adapter)
        .expect("the command should pause inside the budget");
    assert_eq!(paused.task.status, TaskStatus::WaitingForApproval);
    let proposal = paused.command_proposal.expect("the command needs approval");
    let pending = pending_json(&engine, &proposal.id);
    let segment = pending
        .get("segment")
        .expect("a pause inside a segment records that segment");
    assert_eq!(
        segment.get("segment").and_then(|value| value.as_u64()),
        Some(1)
    );
    assert_eq!(
        segment
            .get("totalActionRounds")
            .and_then(|value| value.as_u64()),
        Some(2),
        "the planning rounds are the baseline, and the paused round has not been charged"
    );

    let finished = resume_command(&engine, &proposal.id).expect("the approved command resumes");
    let events = segment_events(&engine, &session.id);
    assert_eq!(
        events
            .iter()
            .filter(|event| event_type(event) == "segment_started")
            .count(),
        1,
        "a resume must not mint a second segment"
    );
    assert_eq!(
        events
            .iter()
            .filter(|event| event_type(event) == "segment_finished")
            .count(),
        0
    );
    assert_ne!(finished.task.status, TaskStatus::WaitingForApproval);
}

#[test]
fn a_question_pause_and_resume_keeps_the_same_segment() {
    let repo = temp_repo("question");
    let engine = engine_for(&repo, |_| {});
    let (proposal_id, session, approved) =
        plan_review(&engine, &repo, ChatTurnOptions::interactive());
    let mut adapter = MockModelAdapter::new_sequence_with_tool_calls(
        vec![String::new()],
        vec![vec![call(
            "call_question",
            "ask_user",
            r#"{"question":"Which helper should I change?","options":["The retry helper","The scheduler"]}"#,
        )]],
    );
    let paused = decide_with(&engine, &proposal_id, &approved, &mut adapter)
        .expect("the question should pause inside the budget");
    assert_eq!(paused.task.status, TaskStatus::WaitingForApproval);
    let question = paused.question_proposal.expect("the question waits");
    let pending = pending_json(&engine, &question.question_id);
    assert!(pending.get("segment").is_some());

    let finished = answer_question(&engine, &question.question_id, "The retry helper")
        .expect("the answer resumes");
    let events = segment_events(&engine, &session.id);
    assert_eq!(
        events
            .iter()
            .filter(|event| event_type(event) == "segment_started")
            .count(),
        1
    );
    assert_eq!(
        events
            .iter()
            .filter(|event| event_type(event) == "segment_finished")
            .count(),
        0
    );
    assert_ne!(finished.task.status, TaskStatus::WaitingForApproval);
}

#[test]
fn a_boundary_records_one_compaction_attempt_and_reuses_the_prepared_request() {
    let repo = temp_repo("compaction");
    let engine = engine_for(&repo, |config| {
        config.conversation_window_share = 10_000;
        config.agent_max_tool_rounds = 4;
        config.agent_max_exploration_rounds = 2;
        config.max_search_matches = 5_000;
        config
            .model_providers
            .push(native_tool_provider(Some(60_000)));
    });
    // Search results are compactable and are not file evidence, so only the
    // latest result for the open step is pinned. The earlier ones are the
    // history Task 4 replaces when the next request crosses the window.
    for index in 1..=4 {
        let body = (0..3_000)
            .map(|line| format!("needle{index} line {line} with some context text\n"))
            .collect::<String>();
        fs::write(repo.join(format!("src/s{index}.rs")), body).expect("a searchable file");
    }
    let (proposal_id, session, approved) = plan_review(&engine, &repo, ChatTurnOptions::default());
    let mut responses = Vec::new();
    let mut calls = Vec::new();
    for index in 1..=4 {
        responses.push(String::new());
        calls.push(vec![call(
            &format!("call_search_{index}"),
            "search_content",
            &format!(r#"{{"pattern":"needle{index}"}}"#),
        )]);
    }
    responses.push("Done.".to_string());
    calls.push(Vec::new());
    let mut adapter = MockModelAdapter::new_sequence_with_tool_calls(responses, calls);
    let _result = decide_with(&engine, &proposal_id, &approved, &mut adapter)
        .expect("the boundary should compact and continue");

    let attempts = engine
        .session_store
        .read_within_turn_compaction_attempts(&session.id)
        .expect("attempts should read");
    assert!(
        !attempts.is_empty(),
        "the boundary crossed the window share"
    );
    let mut seen = std::collections::BTreeSet::new();
    for attempt in &attempts {
        let key = (
            attempt.request_tokens_before,
            attempt.message_tokens_before,
            attempt.completed_rounds,
            attempt.compacted_results,
        );
        assert!(
            seen.insert(key),
            "the same boundary request was prepared twice: {attempt:#?}"
        );
    }
}

#[test]
fn a_cannot_fit_boundary_sends_no_request_and_starts_no_segment() {
    let repo = temp_repo("cannot-fit");
    let engine = engine_for(&repo, |config| {
        config.conversation_window_share = 10_000;
        config.agent_max_tool_rounds = 2;
        config.agent_max_exploration_rounds = 1;
        config
            .model_providers
            .push(native_tool_provider(Some(12_000)));
    });
    fs::write(repo.join("src/c.rs"), "c\n").expect("a small file");
    let (proposal_id, session, approved) = plan_review(&engine, &repo, ChatTurnOptions::default());
    let mut adapter = MockModelAdapter::new_sequence_with_tool_calls(
        vec![
            String::new(),
            String::new(),
            String::new(),
            "Done.".to_string(),
        ],
        vec![
            vec![read_call("call_a", "src/a.rs")],
            vec![read_call("call_b", "src/b.rs")],
            vec![read_call("call_c", "src/c.rs")],
            Vec::new(),
        ],
    );
    let steering = workspace_engine::TurnSteering::new();
    let mut queued = false;
    let mut on_progress = |event: TurnProgress| {
        if let TurnProgress::Phase(phase) = event
            && phase.kind == PhaseKind::Tool
            && phase.exploration_round >= 1
            && phase.round >= 2
            && !queued
        {
            queued = true;
            steering.queue_note("x".repeat(200_000));
        }
    };
    let result = decide_with_steering(
        &engine,
        &proposal_id,
        &approved,
        &mut adapter,
        &steering,
        &mut on_progress,
    )
    .expect("a cannot-fit boundary is a typed terminal outcome");
    assert_eq!(result.task.status, TaskStatus::TaskCompactionCannotFit);
    assert!(result.round_budget_proposal.is_none());
    let outcome = result
        .task_control
        .terminal_outcome
        .as_ref()
        .expect("the cannot-fit outcome is typed");
    assert_eq!(
        outcome.kind,
        workspace_engine::TaskTerminalOutcomeKind::CompactionCannotFit
    );
    let compaction = outcome
        .compaction
        .expect("the refusal carries the content-safe request facts");
    assert!(compaction.threshold_tokens > 0);

    let events = segment_events(&engine, &session.id);
    assert_eq!(
        events
            .iter()
            .filter(|event| event_type(event) == "segment_started")
            .count(),
        1,
        "the declined boundary must not start a new segment"
    );
    assert_eq!(
        events
            .iter()
            .filter(|event| event_type(event) == "segment_finished")
            .count(),
        0
    );
    assert_eq!(
        adapter.requests.len(),
        3,
        "no model call may follow a request that did not fit"
    );
}

#[test]
fn exploration_crosses_the_legacy_32_round_cap_up_to_its_approved_total() {
    let repo = temp_repo("exploration-total");
    let engine = engine_for(&repo, |config| {
        config.agent_max_tool_rounds = 16;
        config.agent_max_exploration_rounds = 24;
        config.worker_task_budget_exploration_rounds = 50;
    });
    let (proposal_id, session, approved) = plan_review(&engine, &repo, ChatTurnOptions::default());
    assert_eq!(approved.exploration_rounds, 50);
    let mut adapter = read_sequence(&repo, 80);
    let result = decide_with(&engine, &proposal_id, &approved, &mut adapter)
        .expect("the exploration total should carry the task past 32 reads");

    assert_ne!(
        result.task.status,
        TaskStatus::WaitingForApproval,
        "the approved exploration total must not pause at the legacy cap: {:?}",
        result.response
    );
    assert!(result.round_budget_proposal.is_none());
    let events = segment_events(&engine, &session.id);
    let started: Vec<_> = events
        .iter()
        .filter(|event| event_type(event) == "segment_started")
        .collect();
    assert!(
        started.len() >= 3,
        "the approved exploration total must cross more than the legacy 32-round cap"
    );
    let max_total = started
        .iter()
        .map(|event| payload_u64(event, "totalExplorationRounds"))
        .max()
        .unwrap_or(0);
    assert!(
        max_total > 32,
        "exploration totals continue past 32 within the approved 50: {events:#?}"
    );
}

#[test]
fn a_boundary_request_that_would_overspend_input_tokens_sends_nothing_and_starts_no_segment() {
    let repo = temp_repo("input-authority");
    let engine = engine_for(&repo, |config| {
        config.agent_max_tool_rounds = 2;
        config.agent_max_exploration_rounds = 1;
        config.worker_task_budget_input_tokens = 200_000;
    });
    let (proposal_id, session, approved) = plan_review(&engine, &repo, ChatTurnOptions::default());
    assert_eq!(approved.input_tokens, 200_000);
    let mut adapter = action_round_sequence(&repo, 6);
    let steering = workspace_engine::TurnSteering::new();
    let mut queued = false;
    let mut on_progress = |event: TurnProgress| {
        if let TurnProgress::Phase(phase) = event
            && phase.kind == PhaseKind::Tool
            && phase.round >= 2
            && !queued
        {
            queued = true;
            // Safety-pinned user text, so Task 4's compaction cannot remove it:
            // the exact next request is much larger than the remaining input
            // allowance even though its message count still fits.
            steering.queue_note("x".repeat(2_000_000));
        }
    };
    let result = decide_with_steering(
        &engine,
        &proposal_id,
        &approved,
        &mut adapter,
        &steering,
        &mut on_progress,
    )
    .expect("the budget stop is a turn outcome, not a preparation error");

    assert_eq!(result.task.status, TaskStatus::TaskBudgetExhausted);
    let outcome = result
        .task_control
        .terminal_outcome
        .as_ref()
        .expect("the overspent input total is a typed outcome");
    assert_eq!(
        outcome.kind,
        workspace_engine::TaskTerminalOutcomeKind::ApprovedBudgetExhausted
    );
    assert_eq!(
        outcome.dimension,
        Some(workspace_engine::TaskBudgetDimension::InputTokens)
    );
    assert_eq!(
        adapter.requests.len(),
        2,
        "no model call may be made for the boundary request that would overspend the input total"
    );
    let usage = engine
        .session_store
        .read_task_usage(&session.id)
        .expect("usage reads")
        .remove(&result.task.id)
        .expect("the planning and earlier calls recorded usage");
    assert!(
        usage.input_tokens < approved.input_tokens,
        "the refused request was never charged: {} spent of {}",
        usage.input_tokens,
        approved.input_tokens
    );
    let events = segment_events(&engine, &session.id);
    assert_eq!(
        events
            .iter()
            .filter(|event| event_type(event) == "segment_started")
            .count(),
        1,
        "the refused boundary must not start a new segment"
    );
    assert_eq!(
        events
            .iter()
            .filter(|event| event_type(event) == "segment_finished")
            .count(),
        0
    );
}

#[test]
fn the_first_post_approval_request_is_stopped_when_planning_already_spent_the_input_total() {
    let repo = temp_repo("initial-authority");
    let engine = engine_for(&repo, |config| {
        config.worker_task_budget_input_tokens = 1;
    });
    let (proposal_id, session, approved) = plan_review(&engine, &repo, ChatTurnOptions::default());
    assert_eq!(approved.input_tokens, 1);
    let mut adapter = MockModelAdapter::new("Done.");
    let result = decide_with(&engine, &proposal_id, &approved, &mut adapter)
        .expect("the exhausted budget stops before the call");

    assert_eq!(result.task.status, TaskStatus::TaskBudgetExhausted);
    let outcome = result
        .task_control
        .terminal_outcome
        .as_ref()
        .expect("the spent input total is a typed outcome");
    assert_eq!(
        outcome.dimension,
        Some(workspace_engine::TaskBudgetDimension::InputTokens)
    );
    assert!(
        adapter.requests.is_empty(),
        "planning spend cannot authorise another model call"
    );
    let events = segment_events(&engine, &session.id);
    assert_eq!(
        events
            .iter()
            .filter(|event| event_type(event) == "segment_started")
            .count(),
        1,
        "the open segment is recorded, but no boundary is committed"
    );
    assert_eq!(
        events
            .iter()
            .filter(|event| event_type(event) == "segment_finished")
            .count(),
        0
    );
}

#[test]
fn the_outgoing_request_is_capped_to_the_approved_output_remaining() {
    let repo = temp_repo("output-authority");
    let engine = engine_for(&repo, |config| {
        config.agent_max_tool_rounds = 2;
        config.agent_max_exploration_rounds = 1;
        config.worker_task_budget_output_tokens = 100;
        let mut provider = native_tool_provider(None);
        provider.max_output_tokens = Some(8_192);
        config.model_providers.push(provider);
    });
    let (proposal_id, _session, approved) = plan_review(&engine, &repo, ChatTurnOptions::default());
    assert_eq!(approved.output_tokens, 100);
    let mut adapter = action_round_sequence(&repo, 3);
    let result = decide_with(&engine, &proposal_id, &approved, &mut adapter)
        .expect("the capped request should continue the task");

    assert_eq!(
        result.task.status,
        TaskStatus::Complete,
        "{:?}",
        result.response
    );
    assert!(
        adapter.requests.len() >= 4,
        "the task should cross its first boundary: {} requests",
        adapter.requests.len()
    );
    // Request 0 is the first post-approval call; request 3 is the call sent
    // after the first committed boundary.
    for (index, request) in adapter.requests.iter().enumerate() {
        assert_eq!(
            request.max_tokens,
            Some(100),
            "request {index} must be capped to the remaining approved output total"
        );
    }
}

#[test]
fn an_unconfigured_output_ceiling_still_applies_the_approved_remainder_first() {
    let repo = temp_repo("no-configured-output");
    let engine = engine_for(&repo, |config| {
        config.conversation_window_share = 10_000;
        config.agent_max_tool_rounds = 2;
        config.agent_max_exploration_rounds = 1;
        config.worker_task_budget_output_tokens = 100;
        // `max_output_tokens` stays `None` on both the provider and the
        // built-in model default. The approved remainder is the only output
        // ceiling, and it must be in the request Task 4 measures.
        config
            .model_providers
            .push(native_tool_provider(Some(60_000)));
    });
    let (proposal_id, _session, approved) = plan_review(&engine, &repo, ChatTurnOptions::default());
    assert_eq!(approved.output_tokens, 100);
    let mut adapter = action_round_sequence(&repo, 3);
    let result = decide_with(&engine, &proposal_id, &approved, &mut adapter)
        .expect("the capped request should continue the task");

    assert_eq!(
        result.task.status,
        TaskStatus::Complete,
        "{:?}",
        result.response
    );
    assert!(
        adapter.requests.len() >= 3,
        "the task should cross a boundary: {} requests",
        adapter.requests.len()
    );
    for (index, request) in adapter.requests.iter().enumerate() {
        assert_eq!(
            request.max_tokens,
            Some(100),
            "request {index} must carry the approved remainder even with no configured output cap"
        );
    }
}

#[test]
fn task4_measures_the_same_capped_request_the_adapter_receives() {
    let repo = temp_repo("exact-request");
    let engine = engine_for(&repo, |config| {
        config.conversation_window_share = 10_000;
        config.agent_max_tool_rounds = 4;
        config.agent_max_exploration_rounds = 2;
        config.max_search_matches = 5_000;
        config.worker_task_budget_output_tokens = 100;
        // No configured output cap, so a post-measurement `max_tokens` would
        // change the request bytes relative to Task 4's recorded estimate.
        config
            .model_providers
            .push(native_tool_provider(Some(60_000)));
    });
    for index in 1..=4 {
        let body = (0..3_000)
            .map(|line| format!("needle{index} line {line} with some context text\n"))
            .collect::<String>();
        fs::write(repo.join(format!("src/s{index}.rs")), body).expect("a searchable file");
    }
    let (proposal_id, session, approved) = plan_review(&engine, &repo, ChatTurnOptions::default());
    let mut responses = Vec::new();
    let mut calls = Vec::new();
    for index in 1..=4 {
        responses.push(String::new());
        calls.push(vec![call(
            &format!("call_search_{index}"),
            "search_content",
            &format!(r#"{{"pattern":"needle{index}"}}"#),
        )]);
    }
    responses.push("Done.".to_string());
    calls.push(Vec::new());
    let mut adapter = MockModelAdapter::new_sequence_with_tool_calls(responses, calls);
    let _result = decide_with(&engine, &proposal_id, &approved, &mut adapter)
        .expect("the boundary should compact and continue");

    let attempts = engine
        .session_store
        .read_within_turn_compaction_attempts(&session.id)
        .expect("attempts should read");
    let compacted: Vec<_> = attempts
        .iter()
        .filter(|attempt| attempt.outcome == WithinTurnCompactionAttemptOutcome::Compacted)
        .collect();
    assert!(
        !compacted.is_empty(),
        "the boundary crossed the window share and compacted"
    );
    let sent: Vec<usize> = adapter
        .requests
        .iter()
        .map(|request| model_request_json(request).len().div_ceil(4))
        .collect();
    for attempt in compacted {
        let recorded = attempt
            .request_tokens_after
            .expect("a compacted attempt records the post-compaction size");
        assert!(
            sent.contains(&recorded),
            "Task 4 recorded {recorded} tokens but no adapter request measured that size: {sent:?}"
        );
    }
}

#[test]
fn a_corrupt_pending_segment_resume_state_fails_closed() {
    let repo = temp_repo("corrupt-resume");
    let engine = engine_for(&repo, |_| {});
    let (proposal_id, session, approved) =
        plan_review(&engine, &repo, ChatTurnOptions::interactive());
    let mut adapter = MockModelAdapter::new_sequence_with_tool_calls(
        vec![String::new()],
        vec![vec![call(
            "call_question",
            "ask_user",
            r#"{"question":"Which helper?","options":["Retry","Scheduler"]}"#,
        )]],
    );
    let paused = decide_with(&engine, &proposal_id, &approved, &mut adapter)
        .expect("the question should pause inside the budget");
    let question = paused.question_proposal.expect("the question waits");
    let before = segment_events(&engine, &session.id);

    let path = engine
        .config
        .data_dir
        .join("chat")
        .join("pending")
        .join(format!("{}.json", question.question_id));
    let mut pending: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&path).expect("pending turn"))
            .expect("pending JSON");
    pending["segment"]["startDigest"] = serde_json::Value::String("0".repeat(64));
    fs::write(&path, serde_json::to_string(&pending).unwrap()).expect("corrupt pending turn");

    let error = answer_question(&engine, &question.question_id, "Retry")
        .expect_err("mismatched resume state must fail closed");
    assert!(matches!(error, ClientError::InvalidInput(_)), "{error}");
    assert!(
        error.to_string().contains("does not match durable segment"),
        "{error}"
    );
    assert_eq!(
        segment_events(&engine, &session.id),
        before,
        "a refused resume must not append a segment event"
    );
}

#[test]
fn a_reloaded_engine_makes_the_same_segment_decision_without_duplicate_events() {
    let repo = temp_repo("reload");
    let engine = engine_for(&repo, |_| {});
    let (proposal_id, session, approved) = plan_review(&engine, &repo, ChatTurnOptions::default());
    let mut adapter = MockModelAdapter::new_sequence_with_tool_calls(
        vec![String::new()],
        vec![vec![call(
            "call_command",
            "run_command",
            r#"{"command":"sed -i '' 's/1/2/' src/a.rs","reason":"Bump the constant."}"#,
        )]],
    );
    let paused = decide_with(&engine, &proposal_id, &approved, &mut adapter)
        .expect("the command should pause");
    let command = paused.command_proposal.expect("the command waits");
    let before = segment_events(&engine, &session.id);

    // A fresh engine over the same store resumes the same pending turn.
    let reloaded = engine_for(&repo, |_| {});
    let finished = resume_command(&reloaded, &command.id).expect("the resume should run");
    assert_ne!(finished.task.status, TaskStatus::WaitingForApproval);
    let after = segment_events(&reloaded, &session.id);
    assert_eq!(
        after.len(),
        before.len(),
        "the reload must not duplicate segment events"
    );
    assert_eq!(
        after
            .iter()
            .filter(|event| event_type(event) == "segment_started")
            .count(),
        1
    );
}
