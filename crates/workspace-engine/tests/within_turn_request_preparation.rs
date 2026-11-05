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
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use workspace_engine::finding::{Finding, FindingDraft, FindingSource, Severity, SourceRange};
use workspace_engine::model::{model_request_json, provider_message_json};
use workspace_engine::{
    CancelToken, ChatTurnOptions, ChatTurnResult, CompactedToolKind, CompletedToolCall,
    CompletedToolRound, Config, Evidence, MockModelAdapter, ModelMessage, ModelProviderConfig,
    ModelRequest, PatchedFile, PhaseKind, ProtectedPath, ProtectedPathKind,
    RequestPreparationInput, RequestPreparationOutcome, SecretScanner, Session, SessionStore,
    TokenEstimate, ToolCall, ToolDefinition, ToolIdentity, ToolResultOutcome, TurnProgress,
    TurnSink, WithinTurnCompactionAttemptOutcome, WorkerRecord, WorkspaceEngine, prepare_request,
    repository_id_for_root,
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
            ChatTurnOptions::interactive(),
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

fn approve_plan_with_budget(
    engine: &WorkspaceEngine,
    proposal: &workspace_engine::AgentPlanProposal,
    budget: Option<&workspace_engine::TaskExecutionBudget>,
    adapter: &mut MockModelAdapter,
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
        .resume_after_plan_decision_with_budget(
            &proposal.id,
            true,
            None,
            budget,
            "tester",
            adapter,
            &mut sink,
        )
        .expect("the reviewed plan should be approved")
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
    worker_session_with_protected_paths(engine, repo, Vec::new())
}

fn worker_session_with_protected_paths(
    engine: &WorkspaceEngine,
    repo: &Path,
    inherited_protected_paths: Vec<ProtectedPath>,
) -> Session {
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
                inherited_protected_paths,
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

fn tool_message<'a>(request: &'a ModelRequest, call_id: &str) -> Option<&'a ModelMessage> {
    request
        .messages
        .iter()
        .find(|message| message.role == "tool" && message.tool_call_id.as_deref() == Some(call_id))
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

/// A read large enough to matter but small enough to stay pinned inside the
/// window while other, larger reads are compacted around it.
fn medium_file(repo: &Path, name: &str, marker: &str) -> String {
    let line = "line of text that makes a read large ".repeat(15);
    let content = format!("{marker}\n{}", format!("{line}\n").repeat(80));
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
fn a_planned_worker_compacts_an_older_read_and_keeps_the_open_step_result() {
    let repo = temp_repo("planned-read-compaction");
    huge_file(&repo, "old.txt", "OLD");
    huge_file(&repo, "new.txt", "NEW");
    let engine = engine(&repo, Some(100_000), 100);
    let session = worker_session(&engine, &repo);
    let mut adapter = MockModelAdapter::new_sequence_with_tool_calls(
        vec![
            "Planning.".to_string(),
            "Preparing the change.".to_string(),
            "Reading the first file.".to_string(),
            "Reading the second file.".to_string(),
            "Re-reading the first file.".to_string(),
            "Done.".to_string(),
        ],
        vec![
            vec![tool_call(
                "plan",
                "propose_plan",
                r#"{"steps":[{"title":"Compare the two files"},{"title":"Report the result"}]}"#,
            )],
            // The plan-review gate pauses before a mutating action. This call is
            // deferred and dropped, so the resume asks the model again and the
            // next scripted turn is the first read.
            vec![tool_call(
                "deferred",
                "propose_patch",
                r#"{"summary":"Add a file","files":[{"path":"never.txt","content":"never\n"}]}"#,
            )],
            vec![read_call("old_read", "old.txt")],
            vec![read_call("new_read", "new.txt")],
            vec![read_call("old_reread", "old.txt")],
            Vec::new(),
        ],
    );
    let (paused, _) = run_turn_for_session(
        &engine,
        &repo,
        Some(&session.id),
        "Compare the two files.",
        &mut adapter,
    );
    let paused = paused.expect("the worker should pause on its plan review");
    let proposal = paused.plan_proposal.expect("the plan is reviewed");
    let budget = proposal
        .budget_review
        .as_ref()
        .expect("a worker review offers the task budget")
        .budget;
    let finished = approve_plan_with_budget(&engine, &proposal, Some(&budget), &mut adapter);

    // The older read is a manifest while the newest result for the still-open
    // step keeps its full body. `latest_call_id_for_step` is the only reason
    // the second read stays pinned, so this is the planned-worker regression.
    let attempts = engine
        .session_store
        .read_within_turn_compaction_attempts(&finished.session.id)
        .expect("attempts should read");
    let first = attempts
        .iter()
        .find(|attempt| attempt.outcome == WithinTurnCompactionAttemptOutcome::Compacted)
        .expect("the planned worker should compact an older read");
    assert!(
        first.eligible_results > 0,
        "a planned worker's older read must be eligible: {first:?}"
    );
    assert!(
        first.facts.iter().any(|fact| fact.call_id == "old_read"),
        "the first read is the older completed result: {first:?}"
    );
    assert!(
        first.facts.iter().all(|fact| fact.call_id != "new_read"),
        "the latest result for the open step must stay pinned: {first:?}"
    );

    let compacted_request = adapter
        .requests
        .iter()
        .find(|request| {
            let old = tool_message(request, "old_read");
            let new = tool_message(request, "new_read");
            old.is_some_and(|message| is_manifest(&message.content))
                && new.is_some_and(|message| !is_manifest(&message.content))
        })
        .expect("one request must hold the manifest and the pinned current result");
    assert!(
        tool_message(compacted_request, "new_read")
            .expect("the current result is present")
            .content
            .contains("Content of new.txt"),
        "the latest result must reach the model unchanged"
    );

    // The durable evidence keeps the path and the hash of every read even when
    // the request body is gone.
    let plan = engine
        .session_store
        .read_task_plan(&finished.session.id, &finished.task.id)
        .expect("the plan should read")
        .expect("the plan should exist");
    let mut reads = Vec::new();
    for step in &plan.steps {
        for evidence in &step.evidence {
            if let Evidence::FileRead { path, hash } = evidence {
                reads.push((path.clone(), hash.clone()));
            }
        }
    }
    for (path, expected_file) in [("old.txt", "old.txt"), ("new.txt", "new.txt")] {
        let expected = workspace_engine::hash::file_hash(repo.join(expected_file))
            .expect("the fixture file should hash");
        assert!(
            reads
                .iter()
                .any(|(read, hash)| read == path && hash == &expected),
            "the durable FileRead evidence for {path} keeps the file hash: {reads:?}"
        );
    }
    assert_eq!(
        reads.iter().filter(|(path, _)| path == "old.txt").count(),
        2,
        "the reread is durable evidence too: {reads:?}"
    );
}

#[test]
fn a_planned_worker_pins_protected_and_base_hash_paths_through_compaction() {
    let repo = temp_repo("planned-safety-paths");
    huge_file(&repo, "old1.txt", "OLD ONE");
    huge_file(&repo, "protected.txt", "PROTECTED");
    huge_file(&repo, "old2.txt", "OLD TWO");
    huge_file(&repo, "base.txt", "BASE");
    let engine = engine(&repo, Some(150_000), 100);
    // The protection comes from the durable worker record, not from parsing the
    // prompt, so this test exercises `TaskProtectedPaths` in `within_turn_pins`.
    let session = worker_session_with_protected_paths(
        &engine,
        &repo,
        vec![ProtectedPath {
            path: "protected.txt".to_string(),
            kind: ProtectedPathKind::File,
        }],
    );
    let mut adapter = MockModelAdapter::new_sequence_with_tool_calls(
        vec![
            "Planning.".to_string(),
            "Preparing the change.".to_string(),
            "Reading the first old file.".to_string(),
            "Reading the protected file.".to_string(),
            "Reading the second old file.".to_string(),
            "Reading the base-hash file.".to_string(),
            "Done.".to_string(),
        ],
        vec![
            vec![tool_call(
                "plan",
                "propose_plan",
                r#"{"steps":[{"title":"Inspect all four files"},{"title":"Report the result"}]}"#,
            )],
            vec![tool_call(
                "deferred",
                "propose_patch",
                r#"{"summary":"Add a file","files":[{"path":"never.txt","content":"never\n"}]}"#,
            )],
            vec![read_call("old1_read", "old1.txt")],
            vec![read_call("protected_read", "protected.txt")],
            vec![read_call("old2_read", "old2.txt")],
            vec![read_call("base_read", "base.txt")],
            Vec::new(),
        ],
    );
    let (paused, _) = run_turn_for_session(
        &engine,
        &repo,
        Some(&session.id),
        "Inspect the four files.",
        &mut adapter,
    );
    let paused = paused.expect("the worker should pause on its plan review");
    // A patch already applied to `base.txt` is a safety path independent of the
    // worker record: its base hash is what a later patch decision verifies.
    engine
        .session_store
        .append_step_evidence(
            &session.id,
            &paused.task.id,
            Evidence::PatchApplied {
                marker_id: "base-marker".to_string(),
                files: vec![PatchedFile {
                    path: "base.txt".to_string(),
                    applied_hash: "base-hash".to_string(),
                }],
            },
        )
        .expect("the durable patch evidence should append");
    let proposal = paused.plan_proposal.expect("the plan is reviewed");
    let budget = proposal
        .budget_review
        .as_ref()
        .expect("a worker review offers the task budget")
        .budget;
    let finished = approve_plan_with_budget(&engine, &proposal, Some(&budget), &mut adapter);

    let attempts = engine
        .session_store
        .read_within_turn_compaction_attempts(&finished.session.id)
        .expect("attempts should read");
    let compacted: Vec<_> = attempts
        .iter()
        .filter(|attempt| attempt.outcome == WithinTurnCompactionAttemptOutcome::Compacted)
        .collect();
    assert!(!compacted.is_empty(), "the long read history must compact");
    // At the final crossing the protected read is older than `old2_read`, so an
    // unprotected reducer would choose it first. Choosing `old2_read` instead is
    // what makes this assertion load-bearing rather than incidental.
    assert!(
        compacted
            .iter()
            .any(|attempt| attempt.facts.iter().any(|fact| fact.call_id == "old2_read")),
        "the eligible second read compacts around the protected path: {compacted:?}"
    );
    for pinned in ["protected_read", "base_read"] {
        assert!(
            compacted
                .iter()
                .all(|attempt| attempt.facts.iter().all(|fact| fact.call_id != pinned)),
            "{pinned} must never become a manifest: {compacted:?}"
        );
    }

    let compacted_request = adapter
        .requests
        .iter()
        .find(|request| {
            tool_message(request, "old2_read").is_some_and(|message| is_manifest(&message.content))
        })
        .expect("one request holds the compacted eligible read");
    for (call_id, path) in [
        ("protected_read", "protected.txt"),
        ("base_read", "base.txt"),
    ] {
        let message =
            tool_message(compacted_request, call_id).expect("the pinned result is present");
        assert!(
            !is_manifest(&message.content) && message.content.contains(path),
            "{path} must reach the model verbatim"
        );
    }

    let plan = engine
        .session_store
        .read_task_plan(&finished.session.id, &finished.task.id)
        .expect("the plan should read")
        .expect("the plan should exist");
    assert!(
        plan.steps.iter().any(|step| {
            step.evidence.iter().any(|evidence| {
                matches!(
                    evidence,
                    Evidence::PatchApplied { files, .. }
                        if files.iter().any(|file| file.path == "base.txt")
                )
            })
        }),
        "the base-hash evidence stays durable"
    );
}

#[test]
fn a_planned_worker_pins_an_active_error_finding_and_its_origin_call() {
    let repo = temp_repo("planned-finding-pins");
    medium_file(&repo, "origin.txt", "ORIGIN");
    medium_file(&repo, "finding.txt", "FINDING");
    huge_file(&repo, "old1.txt", "OLD ONE");
    huge_file(&repo, "old2.txt", "OLD TWO");
    huge_file(&repo, "latest.txt", "LATEST");
    let engine = engine(&repo, Some(150_000), 100);
    let session = worker_session(&engine, &repo);
    let finding = Finding::new(
        FindingDraft {
            source: FindingSource::Compiler,
            severity: Severity::Error,
            summary: "The compiler reports a broken call".to_string(),
            details: None,
            range: Some(SourceRange {
                path: "finding.txt".to_string(),
                start_line: 1,
                start_column: Some(1),
                end_line: Some(2),
                end_column: None,
            }),
            code: Some("E0308".to_string()),
        },
        &SecretScanner::default(),
    )
    .with_origin_ref("origin_read")
    .with_file_hash(
        workspace_engine::hash::file_hash(repo.join("finding.txt"))
            .expect("the finding file should hash"),
    );
    engine
        .session_store
        .record_finding(&session.id, &finding)
        .expect("the open finding should record");

    let mut adapter = MockModelAdapter::new_sequence_with_tool_calls(
        vec![
            "Planning.".to_string(),
            "Preparing the change.".to_string(),
            "Reading the finding origin.".to_string(),
            "Reading the finding file.".to_string(),
            "Reading the first old file.".to_string(),
            "Reading the second old file.".to_string(),
            "Reading the latest file.".to_string(),
            "Done.".to_string(),
        ],
        vec![
            vec![tool_call(
                "plan",
                "propose_plan",
                r#"{"steps":[{"title":"Inspect the files"},{"title":"Report the result"}]}"#,
            )],
            vec![tool_call(
                "deferred",
                "propose_patch",
                r#"{"summary":"Add a file","files":[{"path":"never.txt","content":"never\n"}]}"#,
            )],
            vec![read_call("origin_read", "origin.txt")],
            vec![read_call("finding_read", "finding.txt")],
            vec![read_call("old1_read", "old1.txt")],
            vec![read_call("old2_read", "old2.txt")],
            vec![read_call("latest_read", "latest.txt")],
            Vec::new(),
        ],
    );
    let (paused, _) = run_turn_for_session(
        &engine,
        &repo,
        Some(&session.id),
        "Inspect the files.",
        &mut adapter,
    );
    let paused = paused.expect("the worker should pause on its plan review");
    let proposal = paused.plan_proposal.expect("the plan is reviewed");
    let budget = proposal
        .budget_review
        .as_ref()
        .expect("a worker review offers the task budget")
        .budget;
    let finished = approve_plan_with_budget(&engine, &proposal, Some(&budget), &mut adapter);

    let attempts = engine
        .session_store
        .read_within_turn_compaction_attempts(&finished.session.id)
        .expect("attempts should read");
    let compacted: Vec<_> = attempts
        .iter()
        .filter(|attempt| attempt.outcome == WithinTurnCompactionAttemptOutcome::Compacted)
        .collect();
    assert!(!compacted.is_empty(), "the long read history must compact");
    for pinned in ["origin_read", "finding_read"] {
        assert!(
            compacted
                .iter()
                .all(|attempt| attempt.facts.iter().all(|fact| fact.call_id != pinned)),
            "{pinned} is pinned by the open error finding: {compacted:?}"
        );
    }
    assert!(
        compacted.iter().any(|attempt| attempt
            .facts
            .iter()
            .any(|fact| fact.call_id == "old1_read" || fact.call_id == "old2_read")),
        "the unprotected reads compact around the finding pins: {compacted:?}"
    );

    let compacted_request = adapter
        .requests
        .iter()
        .find(|request| {
            tool_message(request, "old1_read").is_some_and(|message| is_manifest(&message.content))
                || tool_message(request, "old2_read")
                    .is_some_and(|message| is_manifest(&message.content))
        })
        .expect("one request holds an unprotected manifest");
    for (call_id, path, marker) in [
        ("origin_read", "origin.txt", "ORIGIN"),
        ("finding_read", "finding.txt", "FINDING"),
    ] {
        let message =
            tool_message(compacted_request, call_id).expect("the pinned result is present");
        assert!(
            !is_manifest(&message.content) && message.content.contains(marker),
            "{path} must reach the model verbatim"
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

/// An engine at the product's **default** `agent_max_turn_messages` (24).
///
/// R7 is specifically that safety pinning never ran at the default once the
/// legacy count cut applied, so raising the limit here would hide the bug.
/// The allowlisted `./check` lets a failing command run without its own
/// approval pause, so one early round can carry a real failed result.
fn default_limit_engine(data_dir: &Path, context_window: Option<u32>) -> WorkspaceEngine {
    let mut config = Config {
        data_dir: data_dir.to_path_buf(),
        command_allowlist: vec!["./check".to_string()],
        conversation_window_share: 10_000,
        enable_index_watcher: false,
        ..Config::default()
    };
    config
        .model_providers
        .push(native_tool_provider(context_window));
    WorkspaceEngine::new(config)
}

/// Fifteen readable fixture files plus a command that fails with a marker.
fn r7_fixture(repo: &Path) {
    fs::write(
        repo.join("check"),
        "#!/bin/sh\necho R7-FAILED-COMMAND-OUTPUT\nexit 1\n",
    )
    .expect("the failing check should be written");
    fs::set_permissions(repo.join("check"), std::fs::Permissions::from_mode(0o755))
        .expect("the failing check should be executable");
    for index in 0..15 {
        fs::write(
            repo.join(format!("r7_{index:02}.txt")),
            format!("R7 READ {index:02} MARKER\n"),
        )
        .expect("the fixture file should be written");
    }
}

/// A budgeted worker that makes one failing command call after plan approval,
/// then fifteen unconnected reads, then answers. The extra rounds are what push
/// the default 24-message window.
fn long_worker_script() -> (Vec<String>, Vec<Vec<ToolCall>>) {
    let mut responses = vec![
        "Planning.".to_string(),
        "Starting the work.".to_string(),
        "Checking the fixture.".to_string(),
    ];
    let mut calls = vec![
        vec![tool_call(
            "plan",
            "propose_plan",
            r#"{"steps":[{"title":"Inspect the fixture set"},{"title":"Report the result"}]}"#,
        )],
        // The mutation is only what raises plan review; the deferred call is
        // dropped and the resumed turn asks the model again.
        vec![tool_call(
            "review",
            "propose_patch",
            r#"{"summary":"review trigger","files":[{"path":"never.txt","content":"never\n"}]}"#,
        )],
        vec![tool_call(
            "failed_command",
            "run_command",
            r#"{"command":"./check","reason":"Check the fixture"}"#,
        )],
    ];
    for index in 0..15 {
        responses.push(format!("Read {index}."));
        calls.push(vec![read_call(
            &format!("read_{index:02}"),
            &format!("r7_{index:02}.txt"),
        )]);
    }
    responses.push("Done.".to_string());
    calls.push(Vec::new());
    (responses, calls)
}

/// Runs the plan-review pause and approval shared by the R7 regressions.
fn run_budgeted_worker(
    engine: &WorkspaceEngine,
    repo: &Path,
    script: (Vec<String>, Vec<Vec<ToolCall>>),
) -> (
    MockModelAdapter,
    ChatTurnResult,
    workspace_engine::AgentPlanProposal,
) {
    let session = worker_session(engine, repo);
    let mut adapter = MockModelAdapter::new_sequence_with_tool_calls(script.0, script.1);
    let (paused, _) = run_turn_for_session(
        engine,
        repo,
        Some(&session.id),
        "Inspect the fixture set.",
        &mut adapter,
    );
    let paused = paused.expect("the worker should pause on its plan review");
    let proposal = paused.plan_proposal.expect("the plan is reviewed");
    let budget = proposal
        .budget_review
        .as_ref()
        .expect("a worker review offers the task budget")
        .budget;
    let finished = approve_plan_with_budget(engine, &proposal, Some(&budget), &mut adapter);
    (adapter, finished, proposal)
}

/// Every tool result in the selected request has the declaration it answers,
/// and every declaration has its results. A cut by count must not split a
/// round, even when some other round is pinned across it.
fn assert_request_pairing(request: &ModelRequest, context: &str) {
    let declared: BTreeSet<&str> = request
        .messages
        .iter()
        .flat_map(|message| &message.tool_calls)
        .map(|call| call.id.as_str())
        .collect();
    for message in request
        .messages
        .iter()
        .filter(|message| message.role == "tool")
    {
        let id = message
            .tool_call_id
            .as_deref()
            .unwrap_or_else(|| panic!("{context}: a tool result has no call id"));
        assert!(
            declared.contains(id),
            "{context}: `{id}` is an orphaned tool result"
        );
    }
    for message in request
        .messages
        .iter()
        .filter(|message| !message.tool_calls.is_empty())
    {
        for call in &message.tool_calls {
            assert!(
                request.messages.iter().any(|candidate| {
                    candidate.role == "tool"
                        && candidate.tool_call_id.as_deref() == Some(call.id.as_str())
                }),
                "{context}: `{}` has no tool result",
                call.id
            );
        }
    }
}

/// Runs R7's long scenario at the product default and returns the pieces each
/// focused regression needs. The default limit is asserted here so a future
/// config change cannot silently make these tests vacuous.
fn long_worker_at_the_default_message_limit(
    repo_name: &str,
    context_window: Option<u32>,
) -> (
    WorkspaceEngine,
    MockModelAdapter,
    ChatTurnResult,
    workspace_engine::AgentPlanProposal,
) {
    let repo = temp_repo(repo_name);
    r7_fixture(&repo);
    let engine = default_limit_engine(&repo.join(".damaian"), context_window);
    assert_eq!(
        engine.config.agent_max_turn_messages,
        Config::default().agent_max_turn_messages,
        "this regression must run at the product default, not a raised limit"
    );
    assert_eq!(
        engine.config.agent_max_turn_messages, 24,
        "the default this regression exists for changed; re-check the round count"
    );
    let (adapter, finished, proposal) = run_budgeted_worker(&engine, &repo, long_worker_script());
    assert!(
        adapter.requests.len() >= 17,
        "the scenario must run at least 15 tool rounds after approval, got {} requests",
        adapter.requests.len()
    );
    (engine, adapter, finished, proposal)
}

/// Asserts the exact reviewed plan and budget reach the final request once.
fn assert_plan_approval_note(
    adapter: &MockModelAdapter,
    proposal: &workspace_engine::AgentPlanProposal,
) {
    let final_request = adapter
        .requests
        .last()
        .expect("the resumed turn must call the model");
    assert_request_pairing(final_request, "with the plan approval note");

    // (a) The approved plan and all four budget values stay byte-identical.
    let plan_lines = proposal
        .plan
        .steps
        .iter()
        .enumerate()
        .map(|(index, step)| format!("{}. {}", index + 1, step.title))
        .collect::<Vec<_>>()
        .join("\n");
    let budget = proposal.budget_review.as_ref().unwrap().budget;
    let expected_note = format!(
        "The user reviewed the plan and approved it. The plan is now:\n{plan_lines}\nContinue with the current step. You were about to: {}.\n\n{}",
        proposal.deferred_action,
        budget.approval_note()
    );
    let notes: Vec<&ModelMessage> = final_request
        .messages
        .iter()
        .filter(|message| {
            message.role == "user"
                && message
                    .content
                    .starts_with("The user reviewed the plan and approved it.")
        })
        .collect();
    assert_eq!(
        notes.len(),
        1,
        "the plan-approval note must reach the final request exactly once"
    );
    assert_eq!(
        notes[0].content, expected_note,
        "the approved plan and budget must survive the legacy count cut byte for byte"
    );
}

/// R7(a) at the product default: the legacy count cut used to drop the
/// plan-approval note after about eleven tool rounds because safety pinning
/// only ran after a window-threshold crossing.
#[test]
fn a_long_budgeted_worker_at_the_default_message_limit_keeps_the_plan_approval_note() {
    let (_engine, adapter, _finished, proposal) =
        long_worker_at_the_default_message_limit("r7-default-limit-plan-note", Some(1_000_000));
    assert_plan_approval_note(&adapter, &proposal);
}

/// R7(a) also holds where no provider context window is configured: that was
/// an even shorter path back to the legacy count cut.
#[test]
fn a_long_budgeted_worker_without_a_context_window_keeps_the_plan_approval_note() {
    let (_engine, adapter, _finished, proposal) =
        long_worker_at_the_default_message_limit("r7-default-limit-no-window", None);
    assert_plan_approval_note(&adapter, &proposal);
}

/// R7(b): the failed command result from the first post-approval round is an
/// early, non-read result, so the legacy count cut used to be free to drop it.
#[test]
fn a_long_budgeted_worker_at_the_default_message_limit_keeps_the_early_failed_command() {
    let (_engine, adapter, _finished, _proposal) = long_worker_at_the_default_message_limit(
        "r7-default-limit-failed-command",
        Some(1_000_000),
    );

    let resumed = &adapter.requests[2..];
    let final_request = resumed
        .last()
        .expect("the resumed turn must call the model");
    assert_request_pairing(final_request, "with the early failed command");

    let first_command = resumed
        .iter()
        .find_map(|request| tool_message(request, "failed_command"))
        .expect("the failed command result must reach the model at least once");
    let final_command = find_tool_message(final_request, "failed_command");
    assert_eq!(
        final_command.content, first_command.content,
        "the early failed command result must stay byte-identical"
    );
    assert!(
        final_command.content.contains("R7-FAILED-COMMAND-OUTPUT"),
        "the failed command result is present but lost its output: {}",
        final_command.content
    );
}

/// R7(c)+(d): the latest result for the still-open step survives, while the
/// pin set stays a safety allowance rather than removing the count bound.
#[test]
fn a_long_budgeted_worker_at_the_default_message_limit_keeps_the_open_step_result() {
    let (engine, adapter, _finished, _proposal) =
        long_worker_at_the_default_message_limit("r7-default-limit-open-step", Some(1_000_000));

    let resumed = &adapter.requests[2..];
    let final_request = resumed
        .last()
        .expect("the resumed turn must call the model");
    assert_request_pairing(final_request, "with the open-step result");

    // (c) The latest result for the still-open step stays too.
    let first_last_read = resumed
        .iter()
        .find_map(|request| tool_message(request, "read_14"))
        .expect("the newest read must reach the model");
    let final_last_read = find_tool_message(final_request, "read_14");
    assert_eq!(
        final_last_read.content, first_last_read.content,
        "the latest result for the open step must stay byte-identical"
    );
    assert!(
        final_last_read.content.contains("R7 READ 14 MARKER"),
        "the newest open-step read is present but not verbatim: {}",
        final_last_read.content
    );

    // (d) Pinning is not unboundedness: early unpinned completed rounds are
    // still cut by the message window.
    let message_limit = engine.config.agent_max_turn_messages;
    assert!(
        final_request.messages.len() <= message_limit + 2,
        "the request grew past the count bound plus the pinned blocks: {} > {}",
        final_request.messages.len(),
        message_limit + 2
    );
    assert!(
        tool_message(final_request, "read_00").is_none(),
        "the oldest unpinned completed round must still be cut"
    );
    let kept_reads = (0..15)
        .filter(|index| tool_message(final_request, &format!("read_{index:02}")).is_some())
        .count();
    assert!(
        kept_reads < 15,
        "no unpinned round was cut, so the bound is not a bound"
    );
    assert!(kept_reads >= 1, "the newest unpinned round must remain");
}

/// The other half of R7: below the limit nothing changes. The same short turn
/// at the default limit and at a limit that cannot cut it must produce the
/// identical request.
#[test]
fn a_short_turn_is_byte_identical_at_the_default_message_limit() {
    let repo = temp_repo("r7-short-turn");
    r7_fixture(&repo);
    let default_engine =
        default_limit_engine(&repo.join(".damaian-default-limit"), Some(1_000_000));
    let wide_engine =
        engine_with_data_dir(&repo.join(".damaian-wide-limit"), Some(1_000_000), 1000);

    let short_script = || {
        (
            vec![
                "Planning.".to_string(),
                "Starting.".to_string(),
                "Read 0.".to_string(),
                "Read 1.".to_string(),
                "Read 2.".to_string(),
                "Done.".to_string(),
            ],
            vec![
                vec![tool_call(
                    "plan",
                    "propose_plan",
                    r#"{"steps":[{"title":"Inspect the fixture set"},{"title":"Report the result"}]}"#,
                )],
                vec![tool_call(
                    "review",
                    "propose_patch",
                    r#"{"summary":"review trigger","files":[{"path":"never.txt","content":"never\n"}]}"#,
                )],
                vec![read_call("read_00", "r7_00.txt")],
                vec![read_call("read_01", "r7_01.txt")],
                vec![read_call("read_02", "r7_02.txt")],
                Vec::new(),
            ],
        )
    };

    let (default_adapter, default_result, _) =
        run_budgeted_worker(&default_engine, &repo, short_script());
    let (wide_adapter, wide_result, _) = run_budgeted_worker(&wide_engine, &repo, short_script());

    let default_request = default_adapter.requests.last().expect("a default request");
    let wide_request = wide_adapter.requests.last().expect("a wide request");
    assert_eq!(
        default_result.task.status, wide_result.task.status,
        "the same short turn must settle the same way"
    );
    assert_eq!(
        model_request_json(default_request),
        model_request_json(wide_request),
        "below the message limit the request must be byte-identical to the uncut one"
    );
    assert!(
        !default_request
            .messages
            .iter()
            .any(|message| message.content.contains("elided")),
        "a short turn must not gain an elision notice"
    );
    assert_request_pairing(default_request, "below the message limit");
}

/// A command-heavy fixture: `./check` succeeds and prints a line large enough
/// that keeping every successful command result verbatim cannot fit a small
/// but realistic context window, while the bounded selection can.
fn command_heavy_fixture(repo: &Path, repeats: usize) {
    let output = "R7-COMMAND-OUTPUT ".repeat(repeats);
    fs::write(
        repo.join("check"),
        format!("#!/bin/sh\necho \"{output}\"\n"),
    )
    .expect("the command fixture should be written");
    fs::set_permissions(repo.join("check"), std::fs::Permissions::from_mode(0o755))
        .expect("the command fixture should be executable");
}

/// A budgeted worker that runs `commands_per_step` successful command rounds
/// for each plan step and closes that step in the last of them. One command per
/// model round, so the count and the action budget measure the same rounds.
fn command_heavy_script(
    steps: usize,
    commands_per_step: usize,
) -> (Vec<String>, Vec<Vec<ToolCall>>) {
    let plan_steps: Vec<serde_json::Value> = (0..steps)
        .map(|step| serde_json::json!({ "title": format!("Step {step}") }))
        .collect();
    let plan_json = serde_json::json!({ "steps": plan_steps }).to_string();
    let mut responses = vec!["Planning.".to_string(), "Starting the work.".to_string()];
    let mut calls = vec![
        vec![tool_call("plan", "propose_plan", &plan_json)],
        // The mutation only raises plan review; the deferred call is dropped
        // and the resumed turn asks the model again.
        vec![tool_call(
            "review",
            "propose_patch",
            r#"{"summary":"review trigger","files":[{"path":"never.txt","content":"never\n"}]}"#,
        )],
    ];
    for step in 0..steps {
        for command in 0..commands_per_step {
            responses.push(format!("Step {step}, command {command}."));
            let mut round = vec![tool_call(
                &format!("cmd_{step}_{command}"),
                "run_command",
                r#"{"command":"./check","reason":"Run the successful check"}"#,
            )];
            if command + 1 == commands_per_step {
                round.push(tool_call(&format!("close_{step}"), "complete_step", "{}"));
            }
            calls.push(round);
        }
    }
    responses.push("Done.".to_string());
    calls.push(Vec::new());
    (responses, calls)
}

/// A command that fails once, then succeeds on every later invocation.
fn fail_then_succeed_fixture(repo: &Path) {
    fs::write(
        repo.join("check"),
        "#!/bin/sh\nif [ -f .fixed ]; then echo R7-RESOLVED; exit 0; fi\n\
         echo R7-UNRESOLVED-MARKER\ntouch .fixed\nexit 1\n",
    )
    .expect("the fail-then-succeed fixture should be written");
    fs::set_permissions(repo.join("check"), std::fs::Permissions::from_mode(0o755))
        .expect("the fail-then-succeed fixture should be executable");
}

/// R7 follow-up: a failed result is pinned only while it is unresolved. Once
/// the same command succeeds, the old failure may leave the count window; it
/// must not stay pinned forever.
#[test]
fn a_failed_command_stops_being_pinned_after_the_same_command_succeeds() {
    let repo = temp_repo("r7-failed-then-resolved");
    r7_fixture(&repo);
    fail_then_succeed_fixture(&repo);
    let engine = default_limit_engine(&repo.join(".damaian"), Some(1_000_000));

    let mut responses = vec![
        "Planning.".to_string(),
        "Starting the work.".to_string(),
        "Checking the fixture.".to_string(),
    ];
    let mut calls = vec![
        vec![tool_call(
            "plan",
            "propose_plan",
            r#"{"steps":[{"title":"Check the fixture"},{"title":"Report the result"}]}"#,
        )],
        vec![tool_call(
            "review",
            "propose_patch",
            r#"{"summary":"review trigger","files":[{"path":"never.txt","content":"never\n"}]}"#,
        )],
        vec![tool_call(
            "cmd_fail",
            "run_command",
            r#"{"command":"./check","reason":"Check the fixture"}"#,
        )],
    ];
    for index in 0..10 {
        responses.push(format!("Read {index}."));
        calls.push(vec![read_call(
            &format!("read_{index:02}"),
            &format!("r7_{index:02}.txt"),
        )]);
    }
    responses.push("Checking again.".to_string());
    calls.push(vec![tool_call(
        "cmd_success",
        "run_command",
        r#"{"command":"./check","reason":"Check the fixture again"}"#,
    )]);
    for index in 10..15 {
        responses.push(format!("Read {index}."));
        calls.push(vec![read_call(
            &format!("read_{index:02}"),
            &format!("r7_{index:02}.txt"),
        )]);
    }
    responses.push("Done.".to_string());
    calls.push(Vec::new());

    let (adapter, _finished, _proposal) = run_budgeted_worker(&engine, &repo, (responses, calls));

    let before_success = adapter
        .requests
        .iter()
        .find(|request| {
            request
                .messages
                .iter()
                .any(|message| message.content.contains("Read 9."))
        })
        .expect("the request before the successful rerun must exist");
    assert!(
        tool_message(before_success, "cmd_fail").is_some(),
        "an unresolved failure must survive the count cut"
    );

    let final_request = adapter
        .requests
        .last()
        .expect("the resumed turn must call the model");
    assert!(
        tool_message(final_request, "cmd_fail").is_none(),
        "once the same command succeeds, the old failure must stop being pinned"
    );
    assert!(
        tool_message(final_request, "cmd_success").is_some(),
        "the successful rerun is the newest completed round"
    );
    assert_request_pairing(final_request, "after resolving the failed command");
}

/// R7 follow-up: the pin set the legacy count cut now obeys must not keep every
/// successful command forever. Forty successful command rounds across ten
/// completed steps previously grew the request past the window and stopped the
/// worker `CannotFit` — a new way to get stuck, which this work exists to remove.
#[test]
fn a_command_heavy_budgeted_worker_stays_bounded_at_the_default_message_limit() {
    let repo = temp_repo("r7-command-heavy");
    command_heavy_fixture(&repo, 60);
    // A small but realistic window: keeping all 40 successful command results
    // verbatim is about 12k tokens before fixed overhead, while the bounded
    // selection is about 3k plus the approved plan note.
    let engine = default_limit_engine(&repo.join(".damaian"), Some(8_000));
    let (adapter, finished, _proposal) =
        run_budgeted_worker(&engine, &repo, command_heavy_script(10, 4));

    assert_ne!(
        finished.task.status,
        workspace_engine::TaskStatus::TaskCompactionCannotFit,
        "the command-heavy worker must not stop because every command is pinned"
    );
    assert!(
        finished.task_control.terminal_outcome.is_none(),
        "the command-heavy worker must not settle a bounded stop: {:?}",
        finished
            .task_control
            .terminal_outcome
            .as_ref()
            .map(|outcome| outcome.kind)
    );
    assert!(
        adapter.requests.len() >= 40,
        "the scenario must run 40 command rounds, got {} requests",
        adapter.requests.len()
    );

    let final_request = adapter
        .requests
        .last()
        .expect("the resumed turn must call the model");
    assert_request_pairing(final_request, "with a command-heavy history");

    // The bound is the 24-message count window (3 reserved for head/notices)
    // plus the approved-plan user note and at most one generated warning block.
    // It is deliberately much smaller than the pinned-every-command shape.
    let message_limit = engine.config.agent_max_turn_messages;
    assert!(
        final_request.messages.len() <= message_limit + 3,
        "the command-heavy request grew to {} messages, past the count bound plus \
         requirement-6 state",
        final_request.messages.len()
    );
    assert!(
        tool_message(final_request, "cmd_0_0").is_none(),
        "an old successful command round must be elided"
    );
    assert!(
        tool_message(final_request, "cmd_9_3").is_some(),
        "the newest completed round must remain"
    );
    assert!(
        final_request.messages.iter().any(|message| {
            message.role == "user"
                && message
                    .content
                    .starts_with("The user reviewed the plan and approved it.")
        }),
        "the approved plan and budget must remain pinned"
    );
}

/// R7 follow-up point 3: an unpinned, completed **non-read** round is compacted
/// like any other eligible result. Before the pin set was narrowed every
/// command was pinned, so the reducer never saw one and `CannotFit` was the
/// only outcome once the window was small.
#[test]
fn an_unpinned_completed_command_round_is_elided_by_compaction() {
    let repo = temp_repo("r7-command-compaction");
    command_heavy_fixture(&repo, 150);
    let engine = default_limit_engine(&repo.join(".damaian"), Some(8_000));
    let (adapter, finished, _proposal) =
        run_budgeted_worker(&engine, &repo, command_heavy_script(3, 4));

    assert_ne!(
        finished.task.status,
        workspace_engine::TaskStatus::TaskCompactionCannotFit,
        "an unpinned successful command must be reducible"
    );

    let attempts = engine
        .session_store
        .read_within_turn_compaction_attempts(&finished.session.id)
        .expect("compaction attempts should read");
    let compacted_command = attempts.iter().find_map(|attempt| {
        attempt
            .facts
            .iter()
            .find(|fact| fact.call_id.starts_with("cmd_"))
            .map(|fact| (attempt, fact.call_id.clone()))
    });
    let (attempt, call_id) = compacted_command.expect(
        "a successful, unpinned command must become a manifest rather than a permanent pin",
    );
    assert_eq!(
        attempt.outcome,
        WithinTurnCompactionAttemptOutcome::Compacted,
        "the non-read reduction must be a successful compaction"
    );

    let reduced_request = adapter
        .requests
        .iter()
        .find(|request| {
            tool_message(request, &call_id).is_some_and(|message| is_manifest(&message.content))
        })
        .expect("the live request must send the command result as a manifest");
    assert_request_pairing(reduced_request, "with a compacted command result");
}
