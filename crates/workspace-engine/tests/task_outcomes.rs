//! Typed terminal outcomes for bounded worker tasks, per
//! `docs/specs/103_bounded_worker_continuation.md` Task 6.
//!
//! These tests exercise the durable model, each live stop, the reload path, the
//! spec 102 precedence rule and the worker return transport. They deliberately
//! assert the engine-owned kind/dimension/figures rather than a status string.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use workspace_engine::verification::{
    CheckId, CheckTermination, FailureFingerprint, RepairStateEntry, VerificationEvent,
};
use workspace_engine::{
    CancelToken, ChatTurnOptions, ChatTurnResult, Config, MockModelAdapter, ModelProviderConfig,
    RepairStall, RepairStallReason, Session, SessionStore, TaskBudgetDimension,
    TaskExecutionBudget, TaskOutcomeFigures, TaskStatus, TaskTerminalOutcome,
    TaskTerminalOutcomeKind, ToolCall, TurnProgress, TurnSink, WorkerRecord, WorkspaceEngine,
    repository_id_for_root,
};

static COUNTER: AtomicU64 = AtomicU64::new(1);

fn temp_repo(name: &str) -> PathBuf {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock should work")
        .as_nanos();
    let repo = std::env::temp_dir().join(format!(
        "damaian-task-outcomes-{name}-{now}-{}",
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

fn read_sequence(repo: &Path, count: usize) -> MockModelAdapter {
    let mut responses = Vec::new();
    let mut calls = Vec::new();
    for index in 1..=count {
        responses.push(String::new());
        let path = format!("src/f{index}.rs");
        fs::write(repo.join(&path), format!("fn f{index}() {{}}\n"))
            .expect("a readable fixture file");
        calls.push(vec![read_call(&format!("call_read_{index}"), &path)]);
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

fn segment_events(engine: &WorkspaceEngine, session_id: &str) -> Vec<serde_json::Value> {
    let path = engine
        .config
        .data_dir
        .join("sessions")
        .join(format!("{session_id}.jsonl"));
    fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
        .filter(|event| {
            matches!(
                event.get("eventType").and_then(|value| value.as_str()),
                Some("segment_started" | "segment_finished")
            )
        })
        .collect()
}

fn outcome_of(result: &ChatTurnResult) -> &TaskTerminalOutcome {
    result
        .task_control
        .terminal_outcome
        .as_ref()
        .expect("the bounded stop carries a typed outcome")
}

fn assert_non_success(result: &ChatTurnResult) {
    assert!(
        !result.task.status.is_successful(),
        "{:?}",
        result.task.status
    );
    assert!(!result.task.status.is_active(), "{:?}", result.task.status);
    assert!(result.task.status.is_terminal(), "{:?}", result.task.status);
    assert!(result.round_budget_proposal.is_none());
}

fn assert_survives_reload(
    engine: &WorkspaceEngine,
    session_id: &str,
    task_id: &str,
    kind: TaskTerminalOutcomeKind,
) {
    let fresh = WorkspaceEngine::new(Config {
        data_dir: engine.config.data_dir.clone(),
        enable_index_watcher: false,
        ..engine.config.clone()
    });
    let reloaded = fresh
        .session_store
        .read_task_outcome(session_id, task_id)
        .expect("the outcome reads")
        .expect("the outcome survives reload");
    assert_eq!(reloaded.kind, kind);
    assert!(!reloaded.task_status().is_successful());
    let control = workspace_engine::TaskControlService::new(&fresh.session_store, None)
        .session_control(session_id)
        .expect("the control reads")
        .expect("a task");
    assert_eq!(
        control
            .terminal_outcome
            .as_ref()
            .map(|outcome| outcome.kind),
        Some(kind)
    );
    assert!(control.primary_action.is_none());
}

fn durable_usage(
    engine: &WorkspaceEngine,
    session_id: &str,
    task_id: &str,
) -> workspace_engine::TaskUsage {
    engine
        .session_store
        .read_task_usage(session_id)
        .expect("the durable usage reads")
        .remove(task_id)
        .expect("the task recorded its usage")
}

/// The durable `segment_started` record the declined boundary left open.
///
/// It is exactly the stale shape the outcome must *not* copy: zero per-segment
/// counters, the prior task totals and no progress digest.
fn open_segment(
    engine: &WorkspaceEngine,
    session_id: &str,
    task_id: &str,
) -> workspace_engine::TaskSegmentStarted {
    let segments = engine
        .session_store
        .read_task_segments(session_id, task_id)
        .expect("the segments read");
    match segments.last().map(|stored| &stored.event) {
        Some(workspace_engine::TaskSegmentEvent::Started(started)) => started.clone(),
        other => panic!("a declined boundary leaves its segment open: {other:?}"),
    }
}

/// The live snapshot must describe the segment the turn actually ran.
fn assert_live_open_segment(
    engine: &WorkspaceEngine,
    session_id: &str,
    task_id: &str,
    snapshot: &workspace_engine::TaskSegmentSnapshot,
) -> workspace_engine::TaskSegmentStarted {
    let started = open_segment(engine, session_id, task_id);
    assert_eq!(
        snapshot.segment, started.segment,
        "the snapshot names the open segment"
    );
    assert_eq!(
        snapshot.start_digest, started.start_digest,
        "the snapshot keeps the segment's durable start digest"
    );
    assert_eq!(
        started.segment_action_rounds, 0,
        "the durable started record is the stale zero shape"
    );
    assert_eq!(started.segment_exploration_rounds, 0);
    assert_eq!(
        snapshot.total_action_rounds,
        started
            .total_action_rounds
            .saturating_add(snapshot.segment_action_rounds),
        "the task total is the segment baseline plus the live counters"
    );
    assert_eq!(
        snapshot.total_exploration_rounds,
        started
            .total_exploration_rounds
            .saturating_add(snapshot.segment_exploration_rounds),
        "the task total is the segment baseline plus the live counters"
    );
    assert!(
        snapshot.segment_action_rounds + snapshot.segment_exploration_rounds > 0,
        "copying the zeroed segment_started record would pass a stale snapshot"
    );
    assert!(
        snapshot.progress_digest.is_some(),
        "the live stop records the engine's current progress digest"
    );
    let usage = durable_usage(engine, session_id, task_id);
    assert_eq!(snapshot.tokens.input_tokens, usage.input_tokens);
    assert_eq!(snapshot.tokens.output_tokens, usage.output_tokens);
    assert_eq!(snapshot.tokens.source, Some(usage.source));
    started
}

fn reload_outcome(
    engine: &WorkspaceEngine,
    session_id: &str,
    task_id: &str,
) -> TaskTerminalOutcome {
    let fresh = WorkspaceEngine::new(Config {
        data_dir: engine.config.data_dir.clone(),
        enable_index_watcher: false,
        ..engine.config.clone()
    });
    fresh
        .session_store
        .read_task_outcome(session_id, task_id)
        .expect("the outcome reads")
        .expect("the outcome survives a fresh engine")
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

fn audit_text<'a>(event: &'a serde_json::Value, key: &str) -> &'a str {
    event
        .get(key)
        .and_then(|value| value.as_str())
        .unwrap_or_default()
}

fn append_raw_event(data_dir: &Path, session_id: &str, event_type: &str, payload: &str) {
    let path = data_dir
        .join("sessions")
        .join(format!("{session_id}.jsonl"));
    let content = fs::read_to_string(&path).unwrap_or_default();
    let seq = content
        .lines()
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
        .filter_map(|event| event.get("seq").and_then(|value| value.as_u64()))
        .max()
        .unwrap_or(0)
        + 1;
    let line = format!(
        "{{\"eventId\":\"evt_raw_{seq}\",\"seq\":{seq},\"timestampMs\":1,\"eventType\":\"{event_type}\",\"payload\":{payload}}}\n"
    );
    let mut file = fs::OpenOptions::new().append(true).open(path).unwrap();
    use std::io::Write;
    file.write_all(line.as_bytes()).unwrap();
}

fn segment_started(task_id: &str, session_id: &str) -> workspace_engine::TaskSegmentStarted {
    workspace_engine::TaskSegmentStarted {
        format_version: workspace_engine::TASK_SEGMENT_FORMAT_VERSION,
        session_id: session_id.to_string(),
        task_id: task_id.to_string(),
        segment: 1,
        segment_action_rounds: 0,
        segment_exploration_rounds: 0,
        total_action_rounds: 0,
        total_exploration_rounds: 0,
        tokens: workspace_engine::SegmentTokenTotals {
            input_tokens: 100,
            output_tokens: 20,
            source: Some(workspace_engine::model::UsageSource::Measured),
        },
        start_digest: "a".repeat(64),
    }
}

#[test]
fn each_outcome_round_trips_through_a_fresh_store_without_content() {
    let repo = temp_repo("durable-round-trip");
    let engine = engine_for(&repo, |_| {});
    let session = engine
        .session_store
        .create_session(&repository_id_for_root(&repo), "Worker")
        .expect("a session");
    let store = engine.session_store.clone();

    let cases = [
        TaskTerminalOutcomeKind::ApprovedBudgetExhausted,
        TaskTerminalOutcomeKind::CompactionCannotFit,
        TaskTerminalOutcomeKind::NoProgress,
        TaskTerminalOutcomeKind::UsageUnavailable,
    ];
    for kind in cases {
        let task = store
            .create_task(&session.id, "private prompt text", "mock", "m")
            .expect("a task");
        store
            .append_task_segment_started(&session.id, &segment_started(&task.id, &session.id))
            .expect("a segment");
        let last_segment = store
            .read_task_segments(&session.id, &task.id)
            .expect("segments read")
            .last()
            .map(workspace_engine::TaskSegmentSnapshot::from_stored);
        let mut outcome = TaskTerminalOutcome::new(&session.id, &task.id, kind)
            .with_last_segment(last_segment)
            .with_dimension(TaskBudgetDimension::ActionRounds)
            .with_figures(TaskOutcomeFigures {
                approved: 8,
                used: 8,
                required: None,
                remaining: Some(0),
            })
            .with_usage(Some(workspace_engine::SegmentTokenTotals {
                input_tokens: 100,
                output_tokens: 20,
                source: Some(workspace_engine::model::UsageSource::Estimated),
            }));
        if kind == TaskTerminalOutcomeKind::CompactionCannotFit {
            outcome = outcome.with_compaction(workspace_engine::TaskCompactionFacts {
                context_window_tokens: 10_000,
                window_share_bps: 8_000,
                threshold_tokens: 8_000,
                request_tokens_before: 9_000,
                request_tokens_after: None,
                request_tokens_minimum: Some(9_000),
                fixed_overhead_tokens: 500,
                completed_rounds: 4,
                pinned_results: 2,
                eligible_results: 1,
                ineligible_results: 1,
                compacted_results: 0,
            });
        }
        let settled = store
            .settle_task_terminal_outcome(&task, kind.task_status(), &outcome)
            .expect("the outcome settles");
        assert_eq!(settled.status, kind.task_status());

        // A fresh handle sees the same typed facts, including the last segment
        // and the durable usage source.
        let fresh = SessionStore::new(&engine.config.data_dir);
        let read = fresh
            .read_task_outcome(&session.id, &task.id)
            .expect("the outcome reads")
            .expect("the outcome is present");
        assert_eq!(read.kind, kind);
        assert_eq!(read.dimension, Some(TaskBudgetDimension::ActionRounds));
        assert_eq!(read.task_status(), kind.task_status());
        assert_eq!(
            read.last_segment.as_ref().map(|segment| segment.segment),
            Some(1)
        );
        assert_eq!(
            read.usage.as_ref().and_then(|usage| usage.source),
            Some(workspace_engine::model::UsageSource::Estimated)
        );
        let all = fresh
            .read_task_outcomes(&session.id)
            .expect("all outcomes read");
        assert_eq!(all.get(&task.id).map(|read| read.kind), Some(kind));

        let json = serde_json::to_string(&read).unwrap();
        for forbidden in [
            "private prompt text",
            "content",
            "command",
            "\"path\"",
            "finding",
            "tool_output",
        ] {
            assert!(
                !json.contains(forbidden),
                "outcome leaked {forbidden}: {json}"
            );
        }
    }
}

#[test]
fn corrupt_newer_and_wrong_task_records_are_ignored_and_history_reads_as_none() {
    let repo = temp_repo("reader-compat");
    let engine = engine_for(&repo, |_| {});
    let store = engine.session_store.clone();
    let session = store
        .create_session(&repository_id_for_root(&repo), "Worker")
        .expect("a session");
    // A historical status event with no terminal outcome.
    let historical = store
        .create_task(&session.id, "old", "mock", "m")
        .expect("a task");
    store
        .update_task_status(&historical, TaskStatus::Failed, None)
        .expect("old status");
    assert!(
        store
            .read_task_outcome(&session.id, &historical.id)
            .expect("the old log reads")
            .is_none()
    );

    // A valid record survives later corrupt, newer and wrong-task payloads.
    let task = store
        .create_task(&session.id, "new", "mock", "m")
        .expect("a task");
    let outcome =
        TaskTerminalOutcome::new(&session.id, &task.id, TaskTerminalOutcomeKind::NoProgress);
    store
        .settle_task_terminal_outcome(&task, TaskStatus::TaskNoProgress, &outcome)
        .expect("the valid outcome settles");

    let wrong = TaskTerminalOutcome::new(
        &session.id,
        "task_someone_else",
        TaskTerminalOutcomeKind::ApprovedBudgetExhausted,
    );
    append_raw_event(
        &engine.config.data_dir,
        &session.id,
        "task_status_updated",
        &format!(
            "{{\"task\":{{\"id\":\"{}\"}},\"terminalOutcome\":{}}}",
            task.id,
            serde_json::to_string(&wrong).unwrap()
        ),
    );
    let mut newer = TaskTerminalOutcome::new(
        &session.id,
        &task.id,
        TaskTerminalOutcomeKind::CompactionCannotFit,
    );
    newer.format_version = workspace_engine::TASK_TERMINAL_OUTCOME_FORMAT_VERSION + 1;
    append_raw_event(
        &engine.config.data_dir,
        &session.id,
        "task_status_updated",
        &format!(
            "{{\"task\":{{\"id\":\"{}\"}},\"terminalOutcome\":{}}}",
            task.id,
            serde_json::to_string(&newer).unwrap()
        ),
    );
    append_raw_event(
        &engine.config.data_dir,
        &session.id,
        "task_status_updated",
        &format!(
            "{{\"task\":{{\"id\":\"{}\"}},\"terminalOutcome\":{{\"formatVersion\":1}}}}",
            task.id
        ),
    );

    let fresh = SessionStore::new(&engine.config.data_dir);
    let read = fresh
        .read_task_outcome(&session.id, &task.id)
        .expect("the corrupt tail is skipped")
        .expect("the earlier valid outcome survives");
    assert_eq!(read.kind, TaskTerminalOutcomeKind::NoProgress);
}

#[test]
fn action_round_exhaustion_settles_the_approved_budget_outcome() {
    let repo = temp_repo("action-exhausted");
    let engine = engine_for(&repo, |config| {
        config.agent_max_tool_rounds = 2;
        config.agent_max_exploration_rounds = 4;
        config.worker_task_budget_action_rounds = 3;
    });
    let (proposal_id, session, approved) = plan_review(&engine, &repo, ChatTurnOptions::default());
    let mut adapter = action_round_sequence(&repo, 10);
    let result = decide_with(&engine, &proposal_id, &approved, &mut adapter)
        .expect("the action-round bound settles a terminal outcome");

    assert_eq!(result.task.status, TaskStatus::TaskBudgetExhausted);
    assert_non_success(&result);
    let outcome = outcome_of(&result);
    assert_eq!(
        outcome.kind,
        TaskTerminalOutcomeKind::ApprovedBudgetExhausted
    );
    assert_eq!(outcome.dimension, Some(TaskBudgetDimension::ActionRounds));
    let figures = outcome.figures.expect("exact figures");
    assert!(figures.used >= figures.approved);
    assert_eq!(
        figures.approved,
        u64::from(
            approved
                .action_rounds
                .min(workspace_engine::ABSOLUTE_TASK_ACTION_ROUND_CEILING)
        )
    );

    let calls_at_stop = adapter.requests.len();
    // No extra model call after the bound: the turn is already settled.
    assert!(calls_at_stop <= 4, "unexpected calls: {calls_at_stop}");
    let events = segment_events(&engine, &session.id);
    assert_eq!(
        events
            .iter()
            .filter(
                |event| event.get("eventType").and_then(|value| value.as_str())
                    == Some("segment_finished")
            )
            .count(),
        0,
        "a declined boundary stays open"
    );

    // A fresh engine reads the same outcome and control.
    assert_survives_reload(
        &engine,
        &session.id,
        &result.task.id,
        TaskTerminalOutcomeKind::ApprovedBudgetExhausted,
    );
    let control = workspace_engine::TaskControlService::new(&engine.session_store, None)
        .session_control(&session.id)
        .expect("the control reads")
        .expect("a task");
    assert!(control.actions.is_empty());
}

#[test]
fn a_plan_reminder_keeps_a_task_terminal_stop_as_the_turn_response() {
    let repo = temp_repo("reminder-terminal");
    let engine = engine_for(&repo, |config| {
        config.agent_max_tool_rounds = 2;
        config.agent_max_exploration_rounds = 4;
        // Two planning rounds are already in the task total when the plan is
        // approved, so four leaves exactly two post-approval rounds: the answer
        // that triggers the reminder and the completion that exhausts the total.
        config.worker_task_budget_action_rounds = 4;
    });
    let (proposal_id, session, approved) = plan_review(&engine, &repo, ChatTurnOptions::default());

    // The first post-approval answer leaves the plan open, so it triggers the
    // one reminder (and is recorded by `send_plan_reminder`). The completion
    // round that follows brings the task total to the approved four rounds, so
    // the next forced-final checkpoint settles a typed Task 6 stop after the
    // reminder.
    let earlier = "Earlier model prose that must not replace the stop.";
    let mut adapter = MockModelAdapter::new_sequence_with_tool_calls(
        vec![earlier.to_string(), String::new()],
        vec![
            Vec::new(),
            vec![call("call_complete", "complete_step", "{}")],
        ],
    );
    let result = decide_with(&engine, &proposal_id, &approved, &mut adapter)
        .expect("the bounded stop still settles");

    assert_eq!(result.task.status, TaskStatus::TaskBudgetExhausted);
    assert_non_success(&result);
    let outcome = outcome_of(&result).clone();
    assert_eq!(
        outcome.kind,
        TaskTerminalOutcomeKind::ApprovedBudgetExhausted
    );
    assert_eq!(outcome.dimension, Some(TaskBudgetDimension::ActionRounds));

    let rendering = outcome.render();
    assert_eq!(
        result.response, rendering,
        "the typed stop owns the response, not the earlier answer"
    );
    assert_eq!(
        result.model_run.content, rendering,
        "the model run carries the same engine rendering as the response"
    );
    assert_ne!(result.response, earlier);

    // Exactly one reminder was sent, and the earlier answer is still in the
    // transcript where `send_plan_reminder` wrote it.
    let reminders = audit_events(&engine)
        .into_iter()
        .filter(|event| event_type(event) == "plan_reminder_sent")
        .count();
    assert_eq!(reminders, 1);
    let messages = engine
        .session_store
        .read_messages(&session.id)
        .expect("the transcript reads");
    assert!(
        messages
            .iter()
            .any(|message| message.role == "assistant" && message.content == earlier),
        "the earlier answer recorded by send_plan_reminder is preserved"
    );
    assert!(
        messages
            .iter()
            .any(|message| message.kind.as_deref() == Some("plan_reminder")),
        "the reminder stays tagged in the session log"
    );
    let last_assistant = messages
        .iter()
        .rev()
        .find(|message| message.role == "assistant")
        .expect("the turn wrote an assistant message");
    assert_eq!(
        last_assistant.content, rendering,
        "the transcript's last assistant message is the authoritative stop"
    );
    assert_eq!(
        messages
            .iter()
            .filter(|message| message.role == "assistant" && message.content == rendering)
            .count(),
        1,
        "the stop rendering is written exactly once"
    );

    // A fresh engine carries the same outcome and control.
    assert_survives_reload(
        &engine,
        &session.id,
        &result.task.id,
        TaskTerminalOutcomeKind::ApprovedBudgetExhausted,
    );
}

#[test]
fn a_budget_stop_persists_the_live_open_segment_it_ran() {
    let repo = temp_repo("live-budget-segment");
    let engine = engine_for(&repo, |config| {
        config.agent_max_tool_rounds = 2;
        config.agent_max_exploration_rounds = 4;
        config.worker_task_budget_action_rounds = 3;
    });
    let (proposal_id, session, approved) = plan_review(&engine, &repo, ChatTurnOptions::default());
    let mut adapter = action_round_sequence(&repo, 10);
    let result = decide_with(&engine, &proposal_id, &approved, &mut adapter)
        .expect("the action-round bound settles a terminal outcome");

    let outcome = outcome_of(&result).clone();
    let snapshot = outcome
        .last_segment
        .as_ref()
        .expect("the bounded stop describes the segment it ran");
    assert_live_open_segment(&engine, &session.id, &result.task.id, snapshot);
    // Exact live values: the plan review's two action rounds are segment 1's
    // baseline, so the one round the approved total still allowed is the whole
    // live segment and reaches exactly the approved three.
    assert_eq!(snapshot.segment, 1);
    assert_eq!(snapshot.segment_action_rounds, 1);
    assert_eq!(snapshot.segment_exploration_rounds, 0);
    assert_eq!(snapshot.total_action_rounds, 3);
    assert_eq!(snapshot.total_exploration_rounds, 0);

    let usage = durable_usage(&engine, &session.id, &result.task.id);
    let recorded = outcome.usage.expect("the outcome carries durable usage");
    assert_eq!(recorded.input_tokens, usage.input_tokens);
    assert_eq!(recorded.output_tokens, usage.output_tokens);
    assert_eq!(recorded.source, Some(usage.source));

    // A fresh engine reads the same numbers, not the durable zeroed record.
    assert_eq!(
        reload_outcome(&engine, &session.id, &result.task.id),
        outcome
    );
}

#[test]
fn a_no_progress_stop_persists_the_repeated_live_segment() {
    let repo = temp_repo("live-no-progress-segment");
    let engine = engine_for(&repo, |config| {
        config.agent_max_tool_rounds = 2;
        config.agent_max_exploration_rounds = 1;
    });
    let (proposal_id, session, approved) = plan_review(&engine, &repo, ChatTurnOptions::default());
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
        .expect("the repeated segment settles no-progress");

    let outcome = outcome_of(&result).clone();
    let snapshot = outcome
        .last_segment
        .as_ref()
        .expect("the no-progress stop describes the segment it ran");
    let started = assert_live_open_segment(&engine, &session.id, &result.task.id, snapshot);
    assert!(
        snapshot.segment >= 2,
        "the first boundary advanced, so the repeat is a later segment"
    );
    // The no-progress statement is exact: the current digest is the one the
    // segment started with.
    assert_eq!(
        snapshot.progress_digest.as_deref(),
        Some(started.start_digest.as_str()),
        "a repeated digest is what no progress means"
    );
    let usage = durable_usage(&engine, &session.id, &result.task.id);
    let recorded = outcome.usage.expect("the outcome carries durable usage");
    assert_eq!(recorded.input_tokens, usage.input_tokens);
    assert_eq!(recorded.source, Some(usage.source));

    assert_eq!(
        reload_outcome(&engine, &session.id, &result.task.id),
        outcome
    );
}

#[test]
fn a_cannot_fit_stop_persists_the_live_segment_it_could_not_extend() {
    let repo = temp_repo("live-cannot-fit-segment");
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
            && phase.kind == workspace_engine::PhaseKind::Tool
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
    .expect("a cannot-fit request is a typed outcome, not a client error");

    let outcome = outcome_of(&result).clone();
    let snapshot = outcome
        .last_segment
        .as_ref()
        .expect("the cannot-fit stop describes the segment it could not extend");
    let started = assert_live_open_segment(&engine, &session.id, &result.task.id, snapshot);
    assert_eq!(snapshot.segment, 1);
    assert_ne!(
        snapshot.progress_digest.as_deref(),
        Some(started.start_digest.as_str()),
        "the segment advanced before its next request could not fit"
    );
    let usage = durable_usage(&engine, &session.id, &result.task.id);
    let recorded = outcome.usage.expect("the outcome carries durable usage");
    assert_eq!(recorded.input_tokens, usage.input_tokens);
    assert_eq!(recorded.source, Some(usage.source));

    assert_eq!(
        reload_outcome(&engine, &session.id, &result.task.id),
        outcome
    );
}

#[test]
fn exploration_round_exhaustion_names_exploration() {
    let repo = temp_repo("exploration-exhausted");
    let engine = engine_for(&repo, |config| {
        config.agent_max_tool_rounds = 6;
        config.agent_max_exploration_rounds = 2;
        config.worker_task_budget_exploration_rounds = 2;
        config.worker_task_budget_action_rounds = 20;
    });
    let (proposal_id, _session, approved) = plan_review(&engine, &repo, ChatTurnOptions::default());
    let mut adapter = read_sequence(&repo, 12);
    let result = decide_with(&engine, &proposal_id, &approved, &mut adapter)
        .expect("the exploration bound settles a terminal outcome");

    assert_eq!(result.task.status, TaskStatus::TaskBudgetExhausted);
    assert_non_success(&result);
    let outcome = outcome_of(&result);
    assert_eq!(
        outcome.dimension,
        Some(TaskBudgetDimension::ExplorationRounds)
    );
    let figures = outcome.figures.expect("exact figures");
    assert!(figures.used >= figures.approved);
}

#[test]
fn input_token_exhaustion_names_input_tokens() {
    let repo = temp_repo("input-exhausted");
    let engine = engine_for(&repo, |config| {
        config.worker_task_budget_input_tokens = 1;
    });
    let (proposal_id, _session, approved) = plan_review(&engine, &repo, ChatTurnOptions::default());
    let mut adapter = MockModelAdapter::new("Done.");
    let result = decide_with(&engine, &proposal_id, &approved, &mut adapter)
        .expect("the input bound settles before the call");

    assert_eq!(result.task.status, TaskStatus::TaskBudgetExhausted);
    assert_non_success(&result);
    let outcome = outcome_of(&result);
    assert_eq!(outcome.dimension, Some(TaskBudgetDimension::InputTokens));
    assert!(adapter.requests.is_empty(), "no call may follow the bound");
}

#[test]
fn output_token_exhaustion_names_output_tokens() {
    let repo = temp_repo("output-exhausted");
    let engine = engine_for(&repo, |config| {
        config.worker_task_budget_output_tokens = 1;
    });
    let (proposal_id, _session, approved) = plan_review(&engine, &repo, ChatTurnOptions::default());
    let mut adapter = MockModelAdapter::new("Done.");
    let result = decide_with(&engine, &proposal_id, &approved, &mut adapter)
        .expect("the output bound settles before the call");

    assert_eq!(result.task.status, TaskStatus::TaskBudgetExhausted);
    assert_non_success(&result);
    let outcome = outcome_of(&result);
    assert_eq!(outcome.dimension, Some(TaskBudgetDimension::OutputTokens));
    assert_eq!(
        adapter.requests.len(),
        1,
        "the stop lands before the next call; no call follows the exhausted total"
    );
    assert!(
        result.task_control.terminal_outcome.is_some(),
        "the bound is a typed outcome, not a completion"
    );
}

#[test]
fn repeated_progress_settles_the_no_progress_outcome() {
    let repo = temp_repo("no-progress");
    let engine = engine_for(&repo, |config| {
        config.agent_max_tool_rounds = 2;
        config.agent_max_exploration_rounds = 1;
    });
    let (proposal_id, session, approved) = plan_review(&engine, &repo, ChatTurnOptions::default());
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
        .expect("the repeated segment settles no-progress");

    assert_eq!(result.task.status, TaskStatus::TaskNoProgress);
    assert_non_success(&result);
    assert_eq!(
        outcome_of(&result).kind,
        TaskTerminalOutcomeKind::NoProgress
    );
    assert!(outcome_of(&result).dimension.is_none());
    assert!(result.round_budget_proposal.is_none());
    assert_survives_reload(
        &engine,
        &session.id,
        &result.task.id,
        TaskTerminalOutcomeKind::NoProgress,
    );
}

#[test]
fn a_pinned_minimum_that_cannot_fit_settles_cannot_fit_without_a_call_or_boundary() {
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
            && phase.kind == workspace_engine::PhaseKind::Tool
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
    .expect("a cannot-fit request is a typed outcome, not a client error");

    assert_eq!(result.task.status, TaskStatus::TaskCompactionCannotFit);
    assert_non_success(&result);
    let outcome = outcome_of(&result);
    assert_eq!(outcome.kind, TaskTerminalOutcomeKind::CompactionCannotFit);
    assert!(outcome.compaction.is_some());
    assert_eq!(
        adapter.requests.len(),
        3,
        "no model call may follow a request that did not fit"
    );
    let events = segment_events(&engine, &session.id);
    assert_eq!(
        events
            .iter()
            .filter(
                |event| event.get("eventType").and_then(|value| value.as_str())
                    == Some("segment_started")
            )
            .count(),
        1
    );
    assert_eq!(
        events
            .iter()
            .filter(
                |event| event.get("eventType").and_then(|value| value.as_str())
                    == Some("segment_finished")
            )
            .count(),
        0
    );

    // Invalid compaction metadata is a fail-closed preparation error and is not
    // relabelled as a cannot-fit terminal outcome.
    use workspace_engine::request_preparation::{
        RequestPreparationInput, RequestPreparationOutcome, WithinTurnCompactionAttemptOutcome,
        prepare_request,
    };
    use workspace_engine::within_turn_compaction::{
        CompactedToolKind, CompletedToolCall, CompletedToolRound, ToolIdentity, ToolResultOutcome,
    };
    let request = workspace_engine::model::ModelRequest {
        provider: "openai".to_string(),
        model: "m".to_string(),
        messages: vec![
            workspace_engine::model::ModelMessage::user("x".repeat(2_000)),
            workspace_engine::model::ModelMessage::assistant_with_tool_calls(
                "",
                vec![call("dup", "read_file", r#"{"path":"src/a.rs"}"#)],
                None,
            ),
            workspace_engine::model::ModelMessage::tool("dup", "one"),
        ],
        temperature: None,
        reasoning_level: None,
        stream: true,
        tools: None,
        max_tokens: None,
        request_usage: false,
        emit_cache_breakpoints: false,
    };
    let duplicate = CompletedToolRound {
        calls: vec![
            CompletedToolCall {
                call_id: "dup".to_string(),
                tool_kind: CompactedToolKind::ReadFile,
                identity: ToolIdentity::path("src/a.rs"),
                range: None,
                outcome: ToolResultOutcome::Succeeded,
                include_excerpt: false,
            },
            CompletedToolCall {
                call_id: "dup".to_string(),
                tool_kind: CompactedToolKind::ReadFile,
                identity: ToolIdentity::path("src/a.rs"),
                range: None,
                outcome: ToolResultOutcome::Succeeded,
                include_excerpt: false,
            },
        ],
    };
    let scanner = workspace_engine::SecretScanner::default();
    let pinned = std::collections::BTreeSet::new();
    let invalid = prepare_request(&RequestPreparationInput {
        request: &request,
        completed_rounds: &[duplicate],
        pinned_call_ids: &pinned,
        context_window: Some(300),
        window_share_bps: 10_000,
        scanner: &scanner,
        session_id: &session.id,
        task_id: &result.task.id,
    });
    match invalid {
        RequestPreparationOutcome::FailedClosed { attempt } => assert_eq!(
            attempt.outcome,
            WithinTurnCompactionAttemptOutcome::InvalidInput,
            "invalid metadata must not be a cannot-fit refusal"
        ),
        other => panic!("expected invalid input, got {other:?}"),
    }

    assert_survives_reload(
        &engine,
        &session.id,
        &result.task.id,
        TaskTerminalOutcomeKind::CompactionCannotFit,
    );
}

#[test]
fn usage_unavailable_fails_closed_without_offering_a_continuation() {
    let repo = temp_repo("usage-unavailable");
    let engine = engine_for(&repo, |_| {});
    let store = engine.session_store.clone();
    let session = store
        .create_session(&repository_id_for_root(&repo), "Worker")
        .expect("a session");
    let task = store
        .create_task(&session.id, "bounded", "mock", "m")
        .expect("a task");
    let outcome = TaskTerminalOutcome::new(
        &session.id,
        &task.id,
        TaskTerminalOutcomeKind::UsageUnavailable,
    );
    let settled = store
        .settle_task_terminal_outcome(&task, TaskStatus::Failed, &outcome)
        .expect("the abnormal path settles as a named failure");
    assert_eq!(settled.status, TaskStatus::Failed);
    let control = workspace_engine::TaskControlService::new(&store, None)
        .session_control(&session.id)
        .expect("the control reads")
        .expect("a task");
    assert_eq!(
        control
            .terminal_outcome
            .as_ref()
            .map(|outcome| outcome.kind),
        Some(TaskTerminalOutcomeKind::UsageUnavailable)
    );
    assert!(control.detail.contains("could not be proven"));
    assert!(control.primary_action.is_none());
    assert!(control.actions.is_empty());
}

/// Spec 102's stall wins the atomic settlement race, so every surface of the
/// returned turn describes the stall — not the newer Task 6 outcome.
///
/// The stall is written by a concurrent writer while the bounded turn is
/// running, exactly the window the store's locked settlement check closes. A
/// plan reminder precedes the stop, so the authoritative stall report also
/// proves it cannot be replaced by the earlier pre-reminder prose.
#[test]
fn a_stall_that_wins_the_settlement_race_keeps_every_surface_authoritative() {
    let repo = temp_repo("stall-race");
    let engine = engine_for(&repo, |config| {
        config.agent_max_tool_rounds = 2;
        config.agent_max_exploration_rounds = 4;
        // Two planning rounds are already in the task total, so four leaves
        // the answer that triggers the reminder and the round that exhausts it.
        config.worker_task_budget_action_rounds = 4;
    });
    let (proposal_id, session, approved) = plan_review(&engine, &repo, ChatTurnOptions::default());
    let task_id = engine
        .session_store
        .read_tasks(&session.id)
        .expect("the tasks read")
        .last()
        .expect("the plan review's task")
        .id
        .clone();
    let stall = RepairStall::new(RepairStallReason::RepeatedFailure);
    let entry = RepairStateEntry {
        failure_fingerprint: FailureFingerprint::compute(
            &CheckId::new(".", "cargo test"),
            &[],
            &CheckTermination::Exited { exit_code: 1 },
        ),
        patch_digest: "patch".to_string(),
        task_diff_digest: "diff".to_string(),
    };
    let store = engine.session_store.clone();
    let stall_session = session.id.clone();
    let stall_task = task_id.clone();
    let mut wrote = false;
    let mut on_progress = |event: TurnProgress| {
        if wrote {
            return;
        }
        if let TurnProgress::Phase(_) = event {
            wrote = true;
            store
                .append_verification_event(
                    &stall_session,
                    &VerificationEvent::RepairStalled {
                        task_id: stall_task.clone(),
                        entry: entry.clone(),
                        stall: stall.clone(),
                    },
                )
                .expect("the concurrent writer records the stall");
        }
    };
    let steering = workspace_engine::TurnSteering::new();
    let earlier = "Earlier model prose that the stall must replace.";
    let mut adapter = MockModelAdapter::new_sequence_with_tool_calls(
        vec![earlier.to_string(), String::new()],
        vec![
            Vec::new(),
            vec![call("call_complete", "complete_step", "{}")],
        ],
    );
    let result = decide_with_steering(
        &engine,
        &proposal_id,
        &approved,
        &mut adapter,
        &steering,
        &mut on_progress,
    )
    .expect("the bounded stop still settles");
    assert!(wrote, "the stall must land while the turn is running");

    // The store kept the spec 102 result and did not write the Task 6 outcome.
    assert_eq!(result.task.status, TaskStatus::Failed);
    assert!(result.task_control.terminal_outcome.is_none());
    assert!(result.turn_presentation.terminal_outcome.is_none());
    assert!(
        result.turn_presentation.stop_notice.is_none(),
        "a failed task has no bounded-stop notice: {:?}",
        result.turn_presentation.stop_notice
    );
    assert!(
        engine
            .session_store
            .read_task_outcome(&session.id, &result.task.id)
            .expect("the outcome reads")
            .is_none(),
        "no newer outcome is durable"
    );

    // The returned response is the stall's engine-owned report, once. The
    // earlier pre-reminder answer is in the transcript but must not replace it.
    assert_ne!(
        result.response, earlier,
        "the earlier answer must not replace the authoritative stall"
    );
    assert!(
        !result.response.contains("Approved task budget exhausted"),
        "the losing outcome must not reach the response: {}",
        result.response
    );
    assert!(
        result.response.contains(&stall.label),
        "the authoritative stall is presented: {}",
        result.response
    );
    assert_eq!(result.model_run.content, result.response);
    let report = result
        .verification_report
        .as_ref()
        .expect("the stall's report is the presentation");
    assert_eq!(
        report.repair_stall.as_ref().map(|stall| stall.reason),
        Some(RepairStallReason::RepeatedFailure)
    );
    let messages = engine
        .session_store
        .read_messages(&session.id)
        .expect("the transcript reads");
    let last_assistant = messages
        .iter()
        .rev()
        .find(|message| message.role == "assistant")
        .expect("the turn wrote an assistant message");
    assert!(
        !last_assistant
            .content
            .contains("Approved task budget exhausted"),
        "the transcript keeps the authoritative result: {}",
        last_assistant.content
    );

    // The audit names the durable status and writes no losing outcome fact.
    let audit = audit_events(&engine);
    let completed = audit
        .iter()
        .rev()
        .find(|event| event_type(event) == "model_response_completed")
        .expect("the turn records its completion");
    assert_eq!(
        audit_text(completed, "status"),
        TaskStatus::Failed.as_str(),
        "the audit status is the result the task actually has"
    );
    assert!(
        !audit
            .iter()
            .any(|event| event_type(event) == "task_terminal_outcome"),
        "a losing outcome writes no audit fact"
    );

    // A fresh engine sees the same authoritative result and no outcome.
    let fresh = WorkspaceEngine::new(Config {
        data_dir: engine.config.data_dir.clone(),
        enable_index_watcher: false,
        ..engine.config.clone()
    });
    assert!(
        fresh
            .session_store
            .read_task_outcome(&session.id, &result.task.id)
            .expect("the outcome reads")
            .is_none()
    );
    let control = workspace_engine::TaskControlService::new(&fresh.session_store, None)
        .session_control(&session.id)
        .expect("the control reads")
        .expect("a task");
    assert!(control.terminal_outcome.is_none());
    assert_eq!(control.status.as_deref(), Some(TaskStatus::Failed.as_str()));
    assert!(!control.headline.to_lowercase().contains("budget"));
}

#[test]
fn a_recorded_verification_repair_stall_is_preserved_over_a_new_outcome() {
    let repo = temp_repo("repair-stall-precedence");
    let engine = engine_for(&repo, |_| {});
    let store = engine.session_store.clone();
    let session = store
        .create_session(&repository_id_for_root(&repo), "Worker")
        .expect("a session");
    let task = store
        .create_task(&session.id, "repair", "mock", "m")
        .expect("a task");
    let task = store
        .update_task_status(&task, TaskStatus::Failed, None)
        .expect("the stall settles failed first");
    let fingerprint = FailureFingerprint::compute(
        &CheckId::new(".", "cargo test"),
        &[],
        &CheckTermination::Exited { exit_code: 1 },
    );
    store
        .append_verification_event(
            &session.id,
            &VerificationEvent::RepairStalled {
                task_id: task.id.clone(),
                entry: RepairStateEntry {
                    failure_fingerprint: fingerprint.clone(),
                    patch_digest: "patch".to_string(),
                    task_diff_digest: "diff".to_string(),
                },
                stall: RepairStall::new(RepairStallReason::RepeatedFailure),
            },
        )
        .expect("the stall is durable");

    let attempted = TaskTerminalOutcome::new(
        &session.id,
        &task.id,
        TaskTerminalOutcomeKind::ApprovedBudgetExhausted,
    )
    .with_dimension(TaskBudgetDimension::ActionRounds)
    .with_figures(TaskOutcomeFigures {
        approved: 8,
        used: 8,
        required: None,
        remaining: Some(0),
    });
    let settled = store
        .settle_task_terminal_outcome(&task, TaskStatus::TaskBudgetExhausted, &attempted)
        .expect("the write path preserves the stall without erroring");
    assert_eq!(
        settled.status,
        TaskStatus::Failed,
        "the spec 102 stall remains the authoritative terminal result"
    );
    assert!(
        store
            .read_task_outcome(&session.id, &task.id)
            .expect("the outcome reads")
            .is_none(),
        "no new typed outcome was written"
    );
}

#[test]
fn a_historical_worker_return_without_an_outcome_still_decodes() {
    let repo = temp_repo("return-history");
    let engine = engine_for(&repo, |_| {});
    let store = engine.session_store.clone();
    let parent = store
        .create_session(&repository_id_for_root(&repo), "Parent")
        .expect("a parent");
    let historical = workspace_engine::WorkerReturn::new(
        &parent.id,
        "ses_worker",
        "task_worker",
        "branch",
        "/worktree",
        "complete",
        "done",
    );
    let json = serde_json::to_string(&historical).unwrap();
    assert!(!json.contains("terminalOutcome"));
    let read: workspace_engine::WorkerReturn = serde_json::from_str(&json).unwrap();
    assert_eq!(read, historical);
    assert!(read.terminal_outcome.is_none());
    store
        .record_worker_return(&historical, &workspace_engine::SecretScanner::default())
        .expect("the old return records");
    assert_eq!(
        store.pending_worker_returns(&parent.id).unwrap(),
        vec![historical]
    );
}

#[test]
fn the_parent_return_carries_the_same_typed_outcome_and_control() {
    let repo = temp_repo("return-outcome");
    let engine = engine_for(&repo, |_| {});
    let store = engine.session_store.clone();
    let parent = store
        .create_session(&repository_id_for_root(&repo), "Parent")
        .expect("a parent");
    let worker = worker_session(&engine, &repo);
    let task = store
        .create_task(&worker.id, "bounded", "mock", "m")
        .expect("a task");
    let outcome =
        TaskTerminalOutcome::new(&worker.id, &task.id, TaskTerminalOutcomeKind::NoProgress);
    let task = store
        .settle_task_terminal_outcome(&task, TaskStatus::TaskNoProgress, &outcome)
        .expect("the task settles");
    let live = workspace_engine::TaskControlService::new(&store, None)
        .worker_control(
            &worker.id,
            workspace_engine::WorkerTaskFacts {
                task: None,
                reported_task_status: None,
                report: None,
                alive: true,
                state: workspace_engine::WorkerProcessState::Idle,
                pending_question: false,
                has_run: true,
                turn_error: None,
                last_progress_at_ms: None,
                now_ms: workspace_engine::hash::now_millis(),
            },
        )
        .expect("the live control reads");

    let returned = workspace_engine::WorkerReturn::new(
        &parent.id,
        &worker.id,
        &task.id,
        "branch",
        "/worktree",
        task.status.as_str(),
        "stopped",
    )
    .with_terminal_outcome(
        store
            .read_task_outcome(&worker.id, &task.id)
            .expect("the outcome reads"),
    );
    store
        .record_worker_return(&returned, &workspace_engine::SecretScanner::default())
        .expect("the return records");
    let reloaded = SessionStore::new(&engine.config.data_dir)
        .pending_worker_returns(&parent.id)
        .expect("the return reads")
        .pop()
        .expect("one return");
    assert_eq!(
        reloaded
            .terminal_outcome
            .as_ref()
            .map(|outcome| outcome.kind),
        Some(TaskTerminalOutcomeKind::NoProgress)
    );
    let returned_control = workspace_engine::returned_task_control_with_outcome(
        &reloaded.task_status,
        None,
        reloaded.terminal_outcome.as_ref(),
    );
    assert_eq!(returned_control.headline, live.headline);
    assert_eq!(returned_control.detail, live.detail);
    assert_eq!(
        returned_control.outcome,
        workspace_engine::TaskOutcomeKind::NoProgress
    );
    assert_eq!(
        returned_control
            .terminal_outcome
            .as_ref()
            .map(|outcome| outcome.kind),
        Some(TaskTerminalOutcomeKind::NoProgress)
    );
    assert!(returned_control.primary_action.is_none());
}
