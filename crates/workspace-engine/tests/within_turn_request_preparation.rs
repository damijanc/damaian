//! Safety-pinned within-turn request preparation, per
//! `docs/specs/103_bounded_worker_continuation.md` Task 4.
//!
//! These tests exercise both halves of the integration: the pure request
//! preparation component and the live agent loop. They hold the load-bearing
//! rules — pinned state is byte-identical, fixed request overhead counts, a
//! cannot-fit or malformed request sends nothing, a compacted read is
//! repeatable, and the attempt record is durable and content-safe.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use workspace_engine::model::{model_request_json, provider_message_json};
use workspace_engine::{
    CancelToken, ChatTurnOptions, ChatTurnResult, CompactedToolKind, CompletedToolCall,
    CompletedToolRound, Config, MockModelAdapter, ModelMessage, ModelProviderConfig, ModelRequest,
    PhaseKind, RequestPreparationInput, RequestPreparationOutcome, SecretScanner, Session,
    SessionStore, TokenEstimate, ToolCall, ToolDefinition, ToolIdentity, ToolResultOutcome,
    TurnProgress, TurnSink, WithinTurnCompactionAttemptOutcome, WorkerRecord, WorkspaceEngine,
    prepare_request, repository_id_for_root,
};

static COUNTER: AtomicU64 = AtomicU64::new(1);

fn temp_repo(name: &str) -> PathBuf {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock should work")
        .as_nanos();
    let repo = std::env::temp_dir().join(format!(
        "damaian-request-prep-{name}-{now}-{}",
        COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir_all(&repo).expect("repository should be created");
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

fn engine(repo: &Path, context_window: Option<u32>, max_messages: usize) -> WorkspaceEngine {
    engine_with_data_dir(&repo.join(".damaian"), context_window, max_messages)
}

/// A fresh engine over an explicit data directory, so a reload test can point
/// at a copy of the paused state while keeping the same repository root.
fn engine_with_data_dir(
    data_dir: &Path,
    context_window: Option<u32>,
    max_messages: usize,
) -> WorkspaceEngine {
    let mut config = Config {
        data_dir: data_dir.to_path_buf(),
        agent_max_turn_messages: max_messages,
        // The share is the configured basis-point threshold. At 100% the test
        // can state its window directly in tokens.
        conversation_window_share: 10_000,
        enable_index_watcher: false,
        ..Config::default()
    };
    config
        .model_providers
        .push(native_tool_provider(context_window));
    WorkspaceEngine::new(config)
}

fn run_turn(
    engine: &WorkspaceEngine,
    repo: &Path,
    prompt: &str,
    adapter: &mut MockModelAdapter,
) -> (workspace_engine::Result<ChatTurnResult>, Vec<TurnProgress>) {
    run_turn_for_session(engine, repo, None, prompt, adapter)
}

fn run_turn_for_session(
    engine: &WorkspaceEngine,
    repo: &Path,
    session_id: Option<&str>,
    prompt: &str,
    adapter: &mut MockModelAdapter,
) -> (workspace_engine::Result<ChatTurnResult>, Vec<TurnProgress>) {
    let cancel = CancelToken::new();
    let mut on_token = |_token: &str| {};
    let mut progress = Vec::new();
    let result;
    {
        let mut on_progress = |event: TurnProgress| progress.push(event);
        let mut sink = TurnSink {
            on_token: &mut on_token,
            on_progress: &mut on_progress,
            cancel: &cancel,
            steering: None,
        };
        result = engine.chat_orchestrator.ask_with_session_with_options(
            repo,
            prompt,
            &[],
            session_id,
            adapter,
            &mut sink,
            ChatTurnOptions {
                user_present: true,
                ..ChatTurnOptions::default()
            },
        );
    }
    (result, progress)
}

fn resume_answer(
    engine: &WorkspaceEngine,
    question_id: &str,
    answer: &str,
    selected_option: Option<usize>,
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
    engine.chat_orchestrator.resume_after_answer(
        question_id,
        answer,
        selected_option,
        adapter,
        &mut sink,
    )
}

/// Recursively copies a data directory for the reload branch. The paused state
/// is small (session log, pending turn, audit) and the copy must not share a
/// mutable file with the live branch.
fn copy_dir_all(source: &Path, target: &Path) {
    fs::create_dir_all(target).expect("the copy directory should be created");
    for entry in fs::read_dir(source).expect("the source directory should list") {
        let entry = entry.expect("a directory entry");
        let destination = target.join(entry.file_name());
        let file_type = entry.file_type().expect("the entry type");
        if file_type.is_dir() {
            copy_dir_all(&entry.path(), &destination);
        } else if file_type.is_file() {
            fs::copy(entry.path(), destination).expect("the file should copy");
        }
    }
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

fn tool_call(id: &str, name: &str, arguments_json: &str) -> ToolCall {
    ToolCall {
        id: id.to_string(),
        name: name.to_string(),
        arguments_json: arguments_json.to_string(),
    }
}

fn read_call(id: &str, path: &str) -> ToolCall {
    tool_call(
        id,
        "read_file",
        &format!("{{\"path\":\"{}\"}}", path.replace('"', "\\\"")),
    )
}

fn request_with(messages: Vec<ModelMessage>, tools: Option<Vec<ToolDefinition>>) -> ModelRequest {
    ModelRequest {
        provider: "openai".to_string(),
        model: "test-model".to_string(),
        messages,
        temperature: Some("0".to_string()),
        reasoning_level: Some("none".to_string()),
        stream: true,
        tools,
        max_tokens: Some(512),
        request_usage: true,
        emit_cache_breakpoints: false,
    }
}

fn assistant_calls(ids: &[&str]) -> ModelMessage {
    ModelMessage::assistant_with_tool_calls(
        "",
        ids.iter()
            .map(|id| ToolCall {
                id: (*id).to_string(),
                name: "read_file".to_string(),
                arguments_json: "{}".to_string(),
            })
            .collect(),
        None,
    )
}

fn read_call_meta(id: &str, path: &str, outcome: ToolResultOutcome) -> CompletedToolCall {
    CompletedToolCall {
        call_id: id.to_string(),
        tool_kind: CompactedToolKind::ReadFile,
        identity: ToolIdentity::path(path),
        range: None,
        outcome,
        include_excerpt: false,
    }
}

fn prepare(
    request: &ModelRequest,
    rounds: &[CompletedToolRound],
    pinned: &BTreeSet<String>,
    context_window: usize,
) -> RequestPreparationOutcome {
    let scanner = SecretScanner::default();
    prepare_request(&RequestPreparationInput {
        request,
        completed_rounds: rounds,
        pinned_call_ids: pinned,
        context_window: Some(context_window),
        window_share_bps: 10_000,
        scanner: &scanner,
        session_id: "session",
        task_id: "task",
    })
}

fn find_tool_message<'a>(request: &'a ModelRequest, call_id: &str) -> &'a ModelMessage {
    request
        .messages
        .iter()
        .find(|message| message.role == "tool" && message.tool_call_id.as_deref() == Some(call_id))
        .unwrap_or_else(|| panic!("no tool result for {call_id}"))
}

fn is_manifest(content: &str) -> bool {
    serde_json::from_str::<serde_json::Value>(content)
        .ok()
        .and_then(|value| {
            value
                .get("manifestVersion")
                .and_then(serde_json::Value::as_u64)
        })
        .is_some()
}

#[test]
fn below_the_threshold_the_request_is_exactly_the_one_built_and_records_nothing() {
    let body = "x".repeat(400);
    let request = request_with(
        vec![
            ModelMessage::system("SYSTEM"),
            ModelMessage::user("USER"),
            assistant_calls(&["c1"]),
            ModelMessage::tool("c1", body.clone()),
        ],
        None,
    );
    let rounds = vec![CompletedToolRound {
        calls: vec![read_call_meta("c1", "a.txt", ToolResultOutcome::Succeeded)],
    }];
    let outcome = prepare(&request, &rounds, &BTreeSet::new(), 100_000);
    assert!(matches!(outcome, RequestPreparationOutcome::Unchanged));
}

#[test]
fn a_long_read_history_sends_a_fitting_request_and_keeps_batched_pairing() {
    let big = "a".repeat(40_000);
    let request = request_with(
        vec![
            ModelMessage::system("SYSTEM"),
            ModelMessage::user("USER"),
            assistant_calls(&["c1", "c2"]),
            ModelMessage::tool("c1", big.clone()),
            ModelMessage::tool("c2", big.clone()),
        ],
        None,
    );
    let rounds = vec![CompletedToolRound {
        calls: vec![
            read_call_meta("c1", "a.txt", ToolResultOutcome::Succeeded),
            read_call_meta("c2", "b.txt", ToolResultOutcome::Succeeded),
        ],
    }];
    let outcome = prepare(&request, &rounds, &BTreeSet::new(), 600);
    let RequestPreparationOutcome::Compacted { request, attempt } = outcome else {
        panic!("expected a compacted request");
    };
    assert_eq!(
        attempt.outcome,
        WithinTurnCompactionAttemptOutcome::Compacted
    );
    let full_tokens = TokenEstimate::Prose.tokens(&workspace_engine::model_request_json(&request));
    assert!(full_tokens <= 600, "full request is {full_tokens} tokens");
    // Both ids still pair with one declaration and one result.
    for id in ["c1", "c2"] {
        assert!(is_manifest(&find_tool_message(&request, id).content));
    }
    assert_eq!(request.messages[2].tool_calls.len(), 2);
}

#[test]
fn every_pinned_category_survives_byte_for_byte() {
    let unpinned = "u".repeat(20_000);
    let messages = vec![
        ModelMessage::system("SYSTEM INSTRUCTIONS"),
        ModelMessage::user("CURRENT USER REQUEST"),
        ModelMessage::system("APPROVED PLAN AND BUDGET"),
        ModelMessage::system("BOUNDED TURN NOTES"),
        assistant_calls(&[
            "read1",
            "read2",
            "cmd1",
            "fail1",
            "unknown1",
            "protected1",
            "mcp1",
            "web1",
            "fetch1",
        ]),
        ModelMessage::tool("read1", unpinned.clone()),
        ModelMessage::tool("read2", "OPEN STEP LATEST RESULT".to_string()),
        ModelMessage::tool("cmd1", "COMMAND OUTPUT".to_string()),
        ModelMessage::tool("fail1", "UNRESOLVED ERROR".to_string()),
        ModelMessage::tool("unknown1", "UNKNOWN EXTERNAL OUTCOME".to_string()),
        ModelMessage::tool("protected1", "PROTECTED PATH RESULT".to_string()),
        ModelMessage::tool("mcp1", "MCP RESULT".to_string()),
        ModelMessage::tool("web1", "BROWSER RESULT".to_string()),
        ModelMessage::tool("fetch1", "FETCH RESULT".to_string()),
    ];
    let rounds = vec![CompletedToolRound {
        calls: vec![
            read_call_meta("read1", "scratch.txt", ToolResultOutcome::Succeeded),
            read_call_meta("read2", "open-step.txt", ToolResultOutcome::Succeeded),
            CompletedToolCall {
                call_id: "cmd1".to_string(),
                tool_kind: CompactedToolKind::Command,
                identity: ToolIdentity::command("cargo test"),
                range: None,
                outcome: ToolResultOutcome::Succeeded,
                include_excerpt: false,
            },
            read_call_meta("fail1", "broken.txt", ToolResultOutcome::FailedUnresolved),
            read_call_meta(
                "unknown1",
                "network.txt",
                ToolResultOutcome::UnknownExternalOutcome,
            ),
            read_call_meta("protected1", "protected.txt", ToolResultOutcome::Succeeded),
            CompletedToolCall {
                call_id: "mcp1".to_string(),
                tool_kind: CompactedToolKind::McpCall,
                identity: ToolIdentity::endpoint("server/tool"),
                range: None,
                outcome: ToolResultOutcome::Succeeded,
                include_excerpt: false,
            },
            CompletedToolCall {
                call_id: "web1".to_string(),
                tool_kind: CompactedToolKind::WebDiagnostic,
                identity: ToolIdentity::url("https://example.test/"),
                range: None,
                outcome: ToolResultOutcome::Succeeded,
                include_excerpt: false,
            },
            CompletedToolCall {
                call_id: "fetch1".to_string(),
                tool_kind: CompactedToolKind::FetchUrl,
                identity: ToolIdentity::url("https://example.test/page"),
                range: None,
                outcome: ToolResultOutcome::Succeeded,
                include_excerpt: false,
            },
        ],
    }];
    let pinned: BTreeSet<String> = [
        "read2",
        "cmd1",
        "fail1",
        "unknown1",
        "protected1",
        "mcp1",
        "web1",
        "fetch1",
    ]
    .into_iter()
    .map(str::to_string)
    .collect();
    let request = request_with(messages.clone(), None);
    let before_full = TokenEstimate::Prose.tokens(&workspace_engine::model_request_json(&request));
    let unpinned_tokens = TokenEstimate::Prose.tokens(&provider_message_json(&messages[5]));
    let window = before_full - unpinned_tokens / 2;
    let outcome = prepare(&request, &rounds, &pinned, window);
    let RequestPreparationOutcome::Compacted { request: after, .. } = outcome else {
        panic!("expected a compacted request");
    };
    assert!(is_manifest(&find_tool_message(&after, "read1").content));
    for id in [
        "read2",
        "cmd1",
        "fail1",
        "unknown1",
        "protected1",
        "mcp1",
        "web1",
        "fetch1",
    ] {
        assert_eq!(
            find_tool_message(&after, id).content,
            find_tool_message(&request, id).content,
            "{id} must stay verbatim"
        );
    }
    for index in [0usize, 1, 2, 3] {
        assert_eq!(
            after.messages[index].content, messages[index].content,
            "non-tool message {index} must be unchanged"
        );
    }
}

#[test]
fn fixed_request_overhead_can_trigger_compaction_when_messages_alone_would_fit() {
    let body = "b".repeat(20_000);
    let big_tools = (0..40)
        .map(|index| ToolDefinition {
            name: format!("tool_{index}"),
            description: "d".repeat(2_000),
            parameters_json: "{\"type\":\"object\"}".to_string(),
        })
        .collect::<Vec<_>>();
    let request = request_with(
        vec![
            ModelMessage::system("SYSTEM"),
            ModelMessage::user("USER"),
            assistant_calls(&["c1"]),
            ModelMessage::tool("c1", body),
        ],
        Some(big_tools),
    );
    let rounds = vec![CompletedToolRound {
        calls: vec![read_call_meta("c1", "a.txt", ToolResultOutcome::Succeeded)],
    }];
    let full_tokens = TokenEstimate::Prose.tokens(&workspace_engine::model_request_json(&request));
    let message_tokens = TokenEstimate::Prose.tokens(
        &workspace_engine::model::provider_messages_json(&request.messages),
    );
    let overhead_tokens = full_tokens.saturating_sub(message_tokens);
    let window = overhead_tokens + 2_000;
    assert!(
        message_tokens <= window,
        "the message array alone must fit the chosen window"
    );
    assert!(full_tokens > window, "the complete request must exceed it");
    // The message array alone fits this window; the complete request cannot,
    // because the unchanged model/options/tool-schema overhead is added to it.
    let outcome = prepare(&request, &rounds, &BTreeSet::new(), window);
    assert!(
        matches!(outcome, RequestPreparationOutcome::Compacted { .. }),
        "fixed overhead must be counted"
    );
}

#[test]
fn fixed_overhead_refusal_reports_the_exact_empty_message_request_minimum() {
    let big_tools = (0..40)
        .map(|index| ToolDefinition {
            name: format!("tool_{index}"),
            description: "d".repeat(2_000),
            parameters_json: "{\"type\":\"object\"}".to_string(),
        })
        .collect::<Vec<_>>();
    let request = request_with(
        vec![ModelMessage::system("SYSTEM"), ModelMessage::user("USER")],
        Some(big_tools),
    );
    // The fixed overhead alone is far larger than this window, so no message
    // reduction can make the request fit.
    let outcome = prepare(&request, &[], &BTreeSet::new(), 500);
    let RequestPreparationOutcome::FailedClosed { attempt } = outcome else {
        panic!("fixed overhead over the window must fail closed");
    };
    assert_eq!(
        attempt.outcome,
        WithinTurnCompactionAttemptOutcome::CannotFit
    );

    let mut empty = request.clone();
    empty.messages.clear();
    let expected = TokenEstimate::Prose.tokens(&workspace_engine::model_request_json(&empty));
    assert_eq!(
        attempt.request_tokens_minimum,
        Some(expected),
        "the minimum must be the measured empty-message request, not a subtraction"
    );
    assert_ne!(
        attempt.request_tokens_minimum,
        Some(attempt.request_tokens_before),
        "the pre-reduction request is larger than the deterministic minimum"
    );
}

#[test]
fn a_pinned_minimum_that_cannot_fit_fails_closed_without_a_request() {
    let body = "p".repeat(40_000);
    let request = request_with(
        vec![
            ModelMessage::system("SYSTEM"),
            ModelMessage::user("USER"),
            assistant_calls(&["c1"]),
            ModelMessage::tool("c1", body),
        ],
        None,
    );
    let rounds = vec![CompletedToolRound {
        calls: vec![read_call_meta("c1", "a.txt", ToolResultOutcome::Succeeded)],
    }];
    let pinned = ["c1".to_string()].into_iter().collect::<BTreeSet<_>>();
    let outcome = prepare(&request, &rounds, &pinned, 200);
    match outcome {
        RequestPreparationOutcome::FailedClosed { attempt } => {
            assert_eq!(
                attempt.outcome,
                WithinTurnCompactionAttemptOutcome::CannotFit
            );
            assert!(attempt.request_tokens_minimum.is_some());
        }
        other => panic!("expected a cannot-fit refusal, got {other:?}"),
    }
}

#[test]
fn malformed_completed_metadata_fails_closed_without_a_request() {
    let request = request_with(
        vec![
            ModelMessage::system("SYSTEM"),
            ModelMessage::user("USER"),
            assistant_calls(&["c1"]),
            ModelMessage::tool("c1", "x".repeat(20_000)),
        ],
        None,
    );
    let rounds = vec![CompletedToolRound {
        calls: vec![
            read_call_meta("c1", "a.txt", ToolResultOutcome::Succeeded),
            read_call_meta("c1", "b.txt", ToolResultOutcome::Succeeded),
        ],
    }];
    let outcome = prepare(&request, &rounds, &BTreeSet::new(), 100);
    match outcome {
        RequestPreparationOutcome::FailedClosed { attempt } => {
            assert_eq!(
                attempt.outcome,
                WithinTurnCompactionAttemptOutcome::InvalidInput
            );
            assert!(attempt.invalid_input.is_some());
        }
        other => panic!("expected an invalid-input refusal, got {other:?}"),
    }
}

#[test]
fn preparation_is_deterministic() {
    let request = request_with(
        vec![
            ModelMessage::system("SYSTEM"),
            ModelMessage::user("USER"),
            assistant_calls(&["c1"]),
            ModelMessage::tool("c1", "z".repeat(30_000)),
        ],
        None,
    );
    let rounds = vec![CompletedToolRound {
        calls: vec![read_call_meta("c1", "a.txt", ToolResultOutcome::Succeeded)],
    }];
    let first = prepare(&request, &rounds, &BTreeSet::new(), 300);
    let second = prepare(&request, &rounds, &BTreeSet::new(), 300);
    let (
        RequestPreparationOutcome::Compacted { request: a, .. },
        RequestPreparationOutcome::Compacted { request: b, .. },
    ) = (first, second)
    else {
        panic!("expected two compacted requests");
    };
    assert_eq!(a.messages, b.messages);
}

// ---------------------------------------------------------------------------
// Live agent-loop integration
// ---------------------------------------------------------------------------

fn huge_file(repo: &Path, name: &str, marker: &str) -> String {
    // The read tool caps at 400 lines, so make each line large enough that a
    // single bounded read crosses a realistic window share.
    let line = "line of text that makes a read large ".repeat(15);
    let content = format!("{marker}\n{}", format!("{line}\n").repeat(400));
    fs::write(repo.join(name), &content).expect("file should be written");
    content
}

#[test]
fn a_threshold_crossing_attempt_is_durable_content_safe_and_leaves_the_transcript_alone() {
    let repo = temp_repo("durable");
    let secret = "sk-abcdefghijklmnopqrstuvwxyz012345";
    huge_file(&repo, "big.txt", secret);
    let engine = engine(&repo, Some(10_000), 100);
    let mut adapter = MockModelAdapter::new_sequence_with_tool_calls(
        vec!["Done.".to_string(), "Done.".to_string()],
        vec![vec![read_call("c1", "big.txt")], Vec::new()],
    );
    let (result, _) = run_turn(&engine, &repo, "Read the file.", &mut adapter);
    result.expect("the turn should finish");

    let session_id = engine
        .session_store
        .list_sessions(None)
        .expect("sessions should list")
        .first()
        .expect("a session")
        .id
        .clone();
    let attempts = engine
        .session_store
        .read_within_turn_compaction_attempts(&session_id)
        .expect("attempts should read");
    assert_eq!(attempts.len(), 1, "one attempt per trigger");
    let serialized = serde_json::to_string(&attempts).unwrap();
    assert!(!serialized.contains(secret), "no raw secret in the attempt");
    assert!(
        !serialized.contains("line of text that makes a read large"),
        "no raw tool body in the attempt"
    );
    let audit = fs::read_to_string(repo.join(".damaian/audit/events.jsonl")).unwrap();
    assert!(
        audit.contains("\"eventType\":\"within_turn_compaction\""),
        "the audit fact must exist"
    );
    assert!(!audit.contains(secret), "no raw secret in the audit fact");
    // The append-only transcript still holds the full read result.
    let messages = engine.session_store.read_messages(&session_id).unwrap();
    assert!(
        messages.iter().any(|message| message.role == "tool"
            && message
                .content
                .contains("line of text that makes a read large")),
        "the stored transcript must keep the full result"
    );
    // A fresh store over the same data directory reads the same facts.
    let fresh = SessionStore::new(repo.join(".damaian"));
    assert_eq!(
        fresh
            .read_within_turn_compaction_attempts(&session_id)
            .expect("a fresh store must read the facts"),
        attempts
    );
}

#[test]
fn a_read_compacted_away_can_be_performed_again() {
    let repo = temp_repo("reread");
    huge_file(&repo, "big.txt", "MARKER");
    let engine = engine(&repo, Some(10_000), 100);
    let mut adapter = MockModelAdapter::new_sequence_with_tool_calls(
        vec![
            "Reading.".to_string(),
            "Reading again.".to_string(),
            "Done.".to_string(),
        ],
        vec![
            vec![read_call("c1", "big.txt")],
            vec![read_call("c2", "big.txt")],
            Vec::new(),
        ],
    );
    let (result, progress) = run_turn(&engine, &repo, "Read and reread.", &mut adapter);
    result.expect("the turn should finish");
    let saw_second_exploration = progress.iter().any(|event| {
        matches!(
            event,
            TurnProgress::Phase(phase)
                if phase.kind == PhaseKind::Model && phase.exploration_round >= 2
        )
    });
    assert!(
        saw_second_exploration,
        "the compacted read must be repeatable, not a repeated-call action round"
    );
}

#[test]
fn a_request_that_cannot_fit_sends_nothing() {
    let repo = temp_repo("cannot-fit");
    let engine = engine(&repo, Some(500), 100);
    let mut adapter = MockModelAdapter::new("Done.");
    let (result, _) = run_turn(
        &engine,
        &repo,
        "This prompt is much larger than one token.",
        &mut adapter,
    );
    let result = result.expect("a cannot-fit request is a typed terminal outcome");
    assert_eq!(
        result.task.status,
        workspace_engine::TaskStatus::TaskCompactionCannotFit
    );
    assert_eq!(
        result
            .task_control
            .terminal_outcome
            .as_ref()
            .map(|outcome| outcome.kind),
        Some(workspace_engine::TaskTerminalOutcomeKind::CompactionCannotFit)
    );
    assert!(
        adapter.requests.is_empty(),
        "no partially reduced request may reach the model"
    );
    let session_id = engine
        .session_store
        .list_sessions(None)
        .expect("sessions should list")
        .first()
        .expect("a session")
        .id
        .clone();
    let attempts = engine
        .session_store
        .read_within_turn_compaction_attempts(&session_id)
        .expect("attempts should read");
    assert_eq!(attempts.len(), 1, "the failed attempt is recorded once");
    assert_eq!(
        attempts[0].outcome,
        WithinTurnCompactionAttemptOutcome::CannotFit
    );
}

#[test]
fn a_patch_proposal_still_pauses_after_a_compacted_read() {
    let repo = temp_repo("patch-boundary");
    huge_file(&repo, "big.txt", "MARKER");
    let engine = engine(&repo, Some(10_000), 100);
    let mut adapter = MockModelAdapter::new_sequence_with_tool_calls(
        vec![
            "Reading.".to_string(),
            "Patching.".to_string(),
            "Done.".to_string(),
        ],
        vec![
            vec![read_call("c1", "big.txt")],
            vec![tool_call(
                "c2",
                "propose_patch",
                r#"{"summary":"Add a file","files":[{"path":"new.txt","content":"hello\n"}]}"#,
            )],
            Vec::new(),
        ],
    );
    let (result, _) = run_turn(&engine, &repo, "Read then patch.", &mut adapter);
    let result = result.expect("the turn should pause, not fail");
    assert!(
        result.patch_proposal.is_some(),
        "a patch must still pause for its own review"
    );
}

#[test]
fn a_question_still_pauses_after_a_compacted_read() {
    let repo = temp_repo("question-boundary");
    huge_file(&repo, "big.txt", "MARKER");
    let engine = engine(&repo, Some(10_000), 100);
    let mut adapter = MockModelAdapter::new_sequence_with_tool_calls(
        vec![
            "Reading.".to_string(),
            "Asking.".to_string(),
            "Done.".to_string(),
        ],
        vec![
            vec![read_call("c1", "big.txt")],
            vec![tool_call(
                "c2",
                "ask_user",
                r#"{"question":"Which one?","options":["A","B"]}"#,
            )],
            Vec::new(),
        ],
    );
    let (result, _) = run_turn(&engine, &repo, "Read then ask.", &mut adapter);
    let result = result.expect("the turn should pause, not fail");
    assert!(
        result.question_proposal.is_some(),
        "a question must still pause for an answer"
    );
}

#[test]
fn a_budgeted_worker_plan_approval_pins_the_exact_plan_and_budget_through_compaction() {
    let repo = temp_repo("budget-approval");
    huge_file(&repo, "big.txt", "MARKER");
    let engine = engine(&repo, Some(20_000), 100);
    let session = worker_session(&engine, &repo);
    let mut adapter = MockModelAdapter::new_sequence_with_tool_calls(
        vec![
            "Reading.".to_string(),
            "Planning.".to_string(),
            "Patching.".to_string(),
        ],
        vec![
            vec![read_call("c1", "big.txt")],
            vec![tool_call(
                "c2",
                "propose_plan",
                r#"{"steps":[{"title":"Read the retry helper"},{"title":"Add a bounded backoff"},{"title":"Rewrite the scheduler"}]}"#,
            )],
            vec![tool_call(
                "c3",
                "propose_patch",
                r#"{"summary":"Add src/retry.rs","files":[{"path":"src/retry.rs","content":"pub fn added() {}\n"}]}"#,
            )],
        ],
    );
    let (result, _) = run_turn_for_session(
        &engine,
        &repo,
        Some(&session.id),
        "Add retry handling.",
        &mut adapter,
    );
    let result = result.expect("the worker turn should pause for plan review");
    let proposal = result.plan_proposal.expect("the worker plan is reviewed");
    let review = proposal
        .budget_review
        .clone()
        .expect("a genuine worker review offers a task budget");
    let budget = review.budget;
    assert_eq!(
        engine
            .session_store
            .read_task_budget(&session.id, &result.task.id)
            .unwrap(),
        None,
        "nothing is approved before the decision"
    );

    let attempts_before = engine
        .session_store
        .read_within_turn_compaction_attempts(&session.id)
        .expect("attempts should read")
        .len();

    let mut resume = MockModelAdapter::new("Done.");
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
            &proposal.id,
            true,
            None,
            Some(&budget),
            "tester",
            &mut resume,
            &mut sink,
        )
        .expect("the reviewed plan and budget should be approved");

    assert_eq!(
        engine
            .session_store
            .read_task_budget(&session.id, &result.task.id)
            .unwrap(),
        Some(budget),
        "the approved budget is the one that was persisted"
    );

    // The resumed request holds the whole prior read, so it crosses the share
    // again and the reducer replaces that body. The approval note is user
    // content, not a tool body, and must stay untouched.
    let attempts = engine
        .session_store
        .read_within_turn_compaction_attempts(&session.id)
        .expect("attempts should read");
    assert!(
        attempts.len() > attempts_before,
        "the resume must record an attempt"
    );
    assert!(
        attempts[attempts_before..]
            .iter()
            .any(|attempt| attempt.outcome == WithinTurnCompactionAttemptOutcome::Compacted),
        "the resumed request must actually be compacted: {:?}",
        &attempts[attempts_before..]
    );

    let plan_lines = proposal
        .plan
        .steps
        .iter()
        .enumerate()
        .map(|(index, step)| format!("{}. {}", index + 1, step.title))
        .collect::<Vec<_>>()
        .join("\n");
    let expected_note = format!(
        "The user reviewed the plan and approved it. The plan is now:\n{plan_lines}\nContinue with the current step. You were about to: {}.\n\n{}",
        proposal.deferred_action,
        budget.approval_note()
    );

    assert!(
        !resume.requests.is_empty(),
        "the resumed turn must call the model"
    );
    for request in &resume.requests {
        let note = request
            .messages
            .iter()
            .find(|message| {
                message.role == "user"
                    && message
                        .content
                        .starts_with("The user reviewed the plan and approved it.")
            })
            .expect("the approval note must reach the model");
        assert_eq!(
            note.content, expected_note,
            "the approved plan and all four budget values must survive compaction byte for byte"
        );
        assert!(
            is_manifest(&find_tool_message(request, "c1").content),
            "the request must be the compacted view, not the uncompacted transcript"
        );
    }
    for value in [
        budget.action_rounds.to_string(),
        budget.exploration_rounds.to_string(),
        budget.input_tokens.to_string(),
        budget.output_tokens.to_string(),
    ] {
        assert!(
            expected_note.contains(&value),
            "value {value} must be in the approved note"
        );
    }
}

#[test]
fn a_command_approval_still_pauses_after_a_compacted_read() {
    let repo = temp_repo("command-boundary");
    huge_file(&repo, "big.txt", "MARKER");
    fs::write(repo.join("src_file.txt"), "contents\n").unwrap();
    let engine = engine(&repo, Some(10_000), 100);
    let mut adapter = MockModelAdapter::new_sequence_with_tool_calls(
        vec![
            "Reading.".to_string(),
            "Running.".to_string(),
            "Done.".to_string(),
        ],
        vec![
            vec![read_call("c1", "big.txt")],
            vec![tool_call(
                "c2",
                "run_command",
                "{\"command\":\"rm -rf build\",\"reason\":\"clean\"}",
            )],
            Vec::new(),
        ],
    );
    let (result, _) = run_turn(&engine, &repo, "Read then run.", &mut adapter);
    let result = result.expect("the turn should pause, not fail");
    assert!(
        result.command_proposal.is_some(),
        "a command requiring approval must still pause"
    );
}

/// Task 4's reload requirement: a fresh engine over the persisted pause must
/// reconstruct the same next request as the still-live engine, without the
/// request path rewriting the append-only transcript.
#[test]
fn a_reload_reconstructs_the_same_next_request_without_rewriting_the_transcript() {
    let repo = temp_repo("reload-request");
    huge_file(&repo, "big.txt", "MARKER");
    let note = "Approvals are decided in command_policy.rs.";
    let live = engine(&repo, Some(10_000), 100);
    let mut adapter = MockModelAdapter::new_sequence_with_tool_calls(
        vec!["Reading.".to_string(), "Asking.".to_string()],
        vec![
            vec![
                tool_call(
                    "n1",
                    "turn_notes",
                    &serde_json::json!({ "text": note }).to_string(),
                ),
                read_call("c1", "big.txt"),
            ],
            vec![tool_call(
                "c2",
                "ask_user",
                r#"{"question":"Which one?","options":["A","B"]}"#,
            )],
        ],
    );
    let (result, _) = run_turn(&live, &repo, "Read then ask.", &mut adapter);
    let result = result.expect("the turn should pause at the question");
    let question = result
        .question_proposal
        .expect("the question must pause the turn");
    let session_id = result.session.id.clone();

    // The paused turn must carry everything the resumed request needs: the
    // full transcript messages, non-empty turn notes, the completed rounds and
    // the durable marker that the read body was compacted.
    let pending_path = live
        .config
        .data_dir
        .join("chat/pending")
        .join(format!("{}.json", question.question_id));
    let pending: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&pending_path).expect("the pending turn"))
            .expect("the pending turn is JSON");
    assert!(
        pending["messages"]
            .as_array()
            .is_some_and(|messages| !messages.is_empty()),
        "the pause persists the turn messages"
    );
    assert!(
        pending["messages"]
            .to_string()
            .contains("line of text that makes a read large"),
        "the pending messages keep the full read body"
    );
    assert_eq!(pending["turn_notes"], note);
    assert!(
        pending["completed_tool_rounds"]
            .as_array()
            .is_some_and(|rounds| !rounds.is_empty()),
        "the pause persists the completed tool rounds"
    );
    assert!(
        pending["calls"]
            .as_array()
            .is_some_and(|calls| calls.iter().any(|call| call["compacted"] == true)),
        "the compacted read is durable in the pending call list"
    );

    let pause_messages = live
        .session_store
        .read_messages(&session_id)
        .expect("pause transcript should read");
    let pause_attempts = live
        .session_store
        .read_within_turn_compaction_attempts(&session_id)
        .expect("pause attempts should read");
    assert_eq!(
        pause_attempts.len(),
        1,
        "the threshold-crossing question request records exactly one attempt"
    );
    assert_eq!(
        pause_attempts[0].outcome,
        WithinTurnCompactionAttemptOutcome::Compacted
    );
    assert!(
        pause_attempts[0]
            .facts
            .iter()
            .any(|fact| fact.call_id == "c1"),
        "the read is the result that was compacted"
    );

    // Snapshot the whole persisted state before either branch resumes. The
    // fresh engine reads only this copy; the live engine keeps its own store.
    let copy_root = temp_repo("reload-request-copy");
    let copy_data_dir = copy_root.join(".damaian");
    copy_dir_all(&live.config.data_dir, &copy_data_dir);
    let fresh = engine_with_data_dir(&copy_data_dir, Some(10_000), 100);
    assert_eq!(
        fresh
            .session_store
            .read_messages(&session_id)
            .expect("the copied transcript should read"),
        pause_messages,
        "the copy is the exact persisted transcript"
    );
    assert_eq!(
        fresh
            .session_store
            .read_within_turn_compaction_attempts(&session_id)
            .expect("the copied attempts should read"),
        pause_attempts,
        "the copy is the exact persisted attempt history"
    );

    let mut live_resume = MockModelAdapter::new("Done.");
    resume_answer(&live, &question.question_id, "A", Some(0), &mut live_resume)
        .expect("the live engine should resume the answer");
    let mut fresh_resume = MockModelAdapter::new("Done.");
    resume_answer(
        &fresh,
        &question.question_id,
        "A",
        Some(0),
        &mut fresh_resume,
    )
    .expect("the fresh engine should resume the same answer");

    assert_eq!(live_resume.requests.len(), 1, "one next request");
    assert_eq!(fresh_resume.requests.len(), 1, "one next request");
    let live_request = &live_resume.requests[0];
    let fresh_request = &fresh_resume.requests[0];
    assert_eq!(
        model_request_json(live_request),
        model_request_json(fresh_request),
        "the reloaded engine must reconstruct the identical next request byte for byte"
    );

    // What the identical request contains: the compacted read manifest, the
    // still-pinned notes result, the note text and the unchanged call pairing.
    assert!(
        is_manifest(&find_tool_message(live_request, "c1").content),
        "the read must be sent as a manifest"
    );
    assert_eq!(
        find_tool_message(live_request, "n1").content,
        find_tool_message(fresh_request, "n1").content,
        "the pinned notes result stays byte-identical"
    );
    assert!(
        !is_manifest(&find_tool_message(live_request, "n1").content),
        "a pinned non-read result is never replaced"
    );
    assert!(
        live_request
            .messages
            .iter()
            .any(|message| message.content.contains(note)),
        "the turn notes are pinned into the next request"
    );
    assert!(
        live_request.messages.iter().any(|message| {
            message.role == "assistant"
                && message
                    .tool_calls
                    .iter()
                    .any(|call| call.id == "c1" && call.name == "read_file")
        }),
        "the compacted result keeps its assistant declaration and id"
    );

    // The request path changes only what is sent. The append-only transcript
    // still holds the full bodies, and both branches append the same messages.
    let live_messages = live
        .session_store
        .read_messages(&session_id)
        .expect("the live transcript should read");
    let fresh_messages = fresh
        .session_store
        .read_messages(&session_id)
        .expect("the fresh transcript should read");
    assert_eq!(
        &live_messages[..pause_messages.len()],
        &pause_messages[..],
        "request preparation must not rewrite the paused transcript prefix"
    );
    assert_eq!(
        &fresh_messages[..pause_messages.len()],
        &pause_messages[..],
        "the reload branch must keep the paused transcript prefix"
    );
    let shape = |messages: &[workspace_engine::ChatMessage]| {
        messages
            .iter()
            .map(|message| (message.role.clone(), message.content.clone()))
            .collect::<Vec<_>>()
    };
    assert_eq!(
        shape(&live_messages),
        shape(&fresh_messages),
        "both branches persist the same conversation content"
    );
    for messages in [&live_messages, &fresh_messages] {
        assert_eq!(
            messages
                .iter()
                .filter(|message| {
                    message.role == "tool"
                        && message
                            .content
                            .contains("line of text that makes a read large")
                })
                .count(),
            1,
            "the transcript keeps the full read body"
        );
        assert!(
            messages
                .iter()
                .filter(|message| message.role == "tool")
                .all(|message| !is_manifest(&message.content)),
            "request preparation must not write a manifest into the transcript"
        );
    }

    // Exactly one attempt per threshold-crossing request in each branch, and
    // the resumed attempts themselves are identical.
    let live_attempts = live
        .session_store
        .read_within_turn_compaction_attempts(&session_id)
        .expect("live attempts should read");
    let fresh_attempts = fresh
        .session_store
        .read_within_turn_compaction_attempts(&session_id)
        .expect("fresh attempts should read");
    assert_eq!(
        live_attempts.len(),
        pause_attempts.len() + 1,
        "the live resume adds exactly one attempt"
    );
    assert_eq!(
        fresh_attempts.len(),
        pause_attempts.len() + 1,
        "the reload resume adds exactly one attempt"
    );
    assert_eq!(
        &live_attempts[..pause_attempts.len()],
        &pause_attempts[..],
        "the live branch preserves the pause history"
    );
    assert_eq!(
        &fresh_attempts[..pause_attempts.len()],
        &pause_attempts[..],
        "the reload branch preserves the pause history"
    );
    assert_eq!(
        live_attempts.last(),
        fresh_attempts.last(),
        "the resumed request records the same attempt"
    );
    assert_eq!(
        live_attempts.last().map(|attempt| attempt.outcome),
        Some(WithinTurnCompactionAttemptOutcome::Compacted),
        "the resumed request is compacted, not refused"
    );
}
