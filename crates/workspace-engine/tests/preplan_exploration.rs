//! The bounded pre-plan exploration phase for workers (spec 103 R17).
//!
//! A worker with no plan yet must never need the user to type the generic
//! **Continue**. It explores in the same durable segments as an approved task
//! budget; when the pre-plan budget is spent it must propose a plan, and a
//! worker that still does not is stopped with a typed outcome. An interactive
//! session keeps the ordinary round-budget pause byte-for-byte.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use workspace_engine::{
    CancelToken, ChatTurnOptions, ChatTurnResult, Config, MockModelAdapter, Session, TaskStatus,
    ToolCall, TurnProgress, TurnSink, WorkerRecord, WorkspaceEngine, repository_id_for_root,
};

static COUNTER: AtomicU64 = AtomicU64::new(1);

fn temp_repo(name: &str) -> PathBuf {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock should work")
        .as_nanos();
    let repo = std::env::temp_dir().join(format!(
        "damaian-preplan-{name}-{now}-{}",
        COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir_all(repo.join("src")).expect("repository should be created");
    fs::write(repo.join("src/a.rs"), "fn main() {}\n").expect("a file to read");
    repo
}

fn engine_for(repo: &Path, adjust: impl FnOnce(&mut Config)) -> WorkspaceEngine {
    let mut config = Config {
        data_dir: repo.join(".damaian"),
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

fn read_sequence(repo: &Path, count: usize, tail: &[(String, Vec<ToolCall>)]) -> MockModelAdapter {
    let mut responses = Vec::new();
    let mut calls = Vec::new();
    for index in 1..=count {
        responses.push(String::new());
        let path = format!("src/f{index}.rs");
        fs::write(repo.join(&path), format!("fn f{index}() {{}}\n"))
            .expect("a readable fixture file");
        calls.push(vec![read_call(&format!("call_read_{index}"), &path)]);
    }
    for (content, tail_calls) in tail {
        responses.push(content.clone());
        calls.push(tail_calls.clone());
    }
    MockModelAdapter::new_sequence_with_tool_calls(responses, calls)
}

fn plan_tail(repo: &Path) -> Vec<(String, Vec<ToolCall>)> {
    let _ = repo;
    vec![
        (
            String::new(),
            vec![call(
                "call_plan",
                "propose_plan",
                r#"{"steps":[{"title":"Read the retry helper"},{"title":"Add the retry helper"}]}"#,
            )],
        ),
        ("Plan proposed.".to_string(), Vec::new()),
    ]
}

fn ask_with(
    engine: &WorkspaceEngine,
    repo: &Path,
    session_id: Option<&str>,
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
    engine.chat_orchestrator.ask_with_session_with_options(
        repo,
        "Implement retry handling",
        &[],
        session_id,
        adapter,
        &mut sink,
        ChatTurnOptions::default(),
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

fn event_type(event: &serde_json::Value) -> &str {
    event
        .get("eventType")
        .and_then(|value| value.as_str())
        .unwrap_or_default()
}

fn segment_count(engine: &WorkspaceEngine, session_id: &str, kind: &str) -> usize {
    log_events(engine, session_id)
        .iter()
        .filter(|event| event_type(event) == kind)
        .count()
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

fn adapter_sent(adapter: &MockModelAdapter, needle: &str) -> bool {
    adapter
        .requests
        .iter()
        .flat_map(|request| request.messages.iter())
        .any(|message| message.content.contains(needle))
}

#[test]
fn a_worker_exploring_past_the_old_limits_continues_in_segments_without_continue() {
    let repo = temp_repo("segments");
    let engine = engine_for(&repo, |config| {
        config.agent_max_tool_rounds = 2;
        config.agent_max_exploration_rounds = 1;
    });
    let session = worker_session(&engine, &repo);
    let mut adapter = read_sequence(&repo, 12, &[("Done.".to_string(), Vec::new())]);

    let result = ask_with(&engine, &repo, Some(&session.id), &mut adapter)
        .expect("the worker should continue its pre-plan exploration");

    assert!(
        result.round_budget_proposal.is_none(),
        "a worker must not be offered generic Continue before a plan: {:?}",
        result.round_budget_proposal
    );
    assert!(
        !adapter_sent(&adapter, "tool-round segment ended"),
        "the pre-plan phase must not inject the legacy Continue message"
    );
    let started = segment_count(&engine, &session.id, "segment_started");
    assert!(
        started >= 3,
        "twelve reads at a two-action/one-exploration segment should cross several boundaries, got {started}: {:#?}",
        log_events(&engine, &session.id)
    );
}

#[test]
fn spending_the_preplan_budget_asks_for_a_plan_and_the_turn_continues_to_one() {
    let repo = temp_repo("plan-required");
    let engine = engine_for(&repo, |config| {
        config.agent_max_tool_rounds = 2;
        config.agent_max_exploration_rounds = 1;
        config.worker_preplan_exploration_rounds = 6;
    });
    let session = worker_session(&engine, &repo);
    let tail = plan_tail(&repo);
    let mut adapter = read_sequence(&repo, 9, &tail);

    let result = ask_with(&engine, &repo, Some(&session.id), &mut adapter)
        .expect("the turn should continue to the plan proposal");

    assert!(
        result.round_budget_proposal.is_none(),
        "spent pre-plan exploration asks for a plan, not Continue"
    );
    assert!(
        adapter_sent(&adapter, "Call propose_plan now"),
        "the model must be told to propose a plan after the pre-plan budget is spent; requests={:#?}",
        adapter
            .requests
            .iter()
            .map(|request| request
                .messages
                .iter()
                .map(|message| message.content.as_str())
                .collect::<Vec<_>>())
            .collect::<Vec<_>>()
    );
    let plan = engine
        .session_store
        .read_task_plan(&result.session.id, &result.task.id)
        .expect("the plan store should read")
        .unwrap_or_else(|| {
            panic!(
                "the worker's plan proposal should be recorded; status={:?} requests={:#?} events={:#?}",
                result.task.status,
                adapter
                    .requests
                    .iter()
                    .map(|request| request
                        .messages
                        .iter()
                        .map(|message| message.content.as_str())
                        .collect::<Vec<_>>())
                    .collect::<Vec<_>>(),
                log_events(&engine, &result.session.id)
            )
        });
    assert_eq!(plan.steps.len(), 2);
    assert_eq!(plan.steps[1].title, "Add the retry helper");
}

#[test]
fn a_worker_that_still_does_not_propose_a_plan_stops_with_a_typed_outcome() {
    let repo = temp_repo("typed-stop");
    let engine = engine_for(&repo, |config| {
        config.agent_max_tool_rounds = 1;
        config.agent_max_exploration_rounds = 1;
        config.worker_preplan_exploration_rounds = 2;
    });
    let session = worker_session(&engine, &repo);
    let mut adapter = read_sequence(
        &repo,
        4,
        &[("I could not decide on a plan.".to_string(), Vec::new())],
    );

    let result = ask_with(&engine, &repo, Some(&session.id), &mut adapter)
        .expect("the worker should settle its typed pre-plan outcome");

    assert_eq!(
        result.task.status,
        TaskStatus::TaskPrePlanExplorationExhausted
    );
    assert!(
        result.round_budget_proposal.is_none(),
        "a typed stop must never become generic Continue"
    );
    let outcome = result
        .task_control
        .terminal_outcome
        .as_ref()
        .expect("the pre-plan stop is typed and visible in task control");
    assert_eq!(
        outcome.kind,
        workspace_engine::TaskTerminalOutcomeKind::PrePlanExplorationExhausted
    );
    assert_eq!(
        result
            .turn_presentation
            .terminal_outcome
            .as_ref()
            .map(|outcome| outcome.kind),
        Some(workspace_engine::TaskTerminalOutcomeKind::PrePlanExplorationExhausted)
    );
}

#[test]
fn a_command_that_needs_approval_still_pauses_inside_the_preplan_segment() {
    let repo = temp_repo("approval");
    let engine = engine_for(&repo, |config| {
        config.agent_max_tool_rounds = 1;
        config.agent_max_exploration_rounds = 1;
    });
    let session = worker_session(&engine, &repo);
    let mut adapter = read_sequence(
        &repo,
        2,
        &[
            (
                String::new(),
                vec![call(
                    "call_command",
                    "run_command",
                    r#"{"command":"sed -i '' 's/1/2/' src/f1.rs","reason":"Bump the constant."}"#,
                )],
            ),
            ("Done.".to_string(), Vec::new()),
        ],
    );

    let paused = ask_with(&engine, &repo, Some(&session.id), &mut adapter)
        .expect("the command should pause even inside the pre-plan segment");
    assert_eq!(paused.task.status, TaskStatus::WaitingForApproval);
    assert!(
        paused.round_budget_proposal.is_none(),
        "an approval pause is not a round-budget pause"
    );
    let proposal = paused
        .command_proposal
        .as_ref()
        .expect("the command needs its own approval");
    let pending = pending_json(&engine, &proposal.id);
    let segment = pending
        .get("segment")
        .expect("a pause inside a pre-plan segment records that segment");
    assert_eq!(
        segment.get("segment").and_then(|value| value.as_u64()),
        Some(1)
    );
    let started_before = segment_count(&engine, &session.id, "segment_started");

    // A fresh engine reads the same durable paused turn, as a worker restart
    // does. It must resume the same pre-plan phase, not resurrect generic
    // Continue.
    let restarted = engine_for(&repo, |config| {
        config.agent_max_tool_rounds = 1;
        config.agent_max_exploration_rounds = 1;
    });
    let finished = resume_command(&restarted, &proposal.id)
        .expect("the approved command should resume in the same segment");
    assert_ne!(finished.task.status, TaskStatus::WaitingForApproval);
    assert!(
        finished.round_budget_proposal.is_none(),
        "a resumed pre-plan segment must not become a round-budget pause"
    );
    assert!(
        segment_count(&restarted, &session.id, "segment_started") >= started_before,
        "the resumed pre-plan phase keeps its durable segments"
    );
}

#[test]
fn an_interactive_session_keeps_todays_round_limits_and_continue() {
    let repo = temp_repo("interactive");
    let engine = engine_for(&repo, |config| {
        config.agent_max_tool_rounds = 1;
        config.agent_max_exploration_rounds = 1;
        config.worker_preplan_exploration_rounds = 20;
    });
    let mut adapter = read_sequence(&repo, 4, &[("Done.".to_string(), Vec::new())]);

    let result = ask_with(&engine, &repo, None, &mut adapter)
        .expect("an inline turn should pause at its ordinary round budget");

    assert_eq!(result.task.status, TaskStatus::WaitingForApproval);
    assert!(
        result.round_budget_proposal.is_some(),
        "an interactive session keeps today's explicit Continue"
    );
    assert_eq!(
        segment_count(&engine, &result.session.id, "segment_started"),
        0,
        "an interactive session has no pre-plan segment"
    );
}
