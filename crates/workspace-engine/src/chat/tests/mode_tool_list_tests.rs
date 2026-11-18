use super::*;
use crate::config::{McpServerConfig, ModelProviderConfig};
use crate::mode::SessionMode;
use crate::model::MockModelAdapter;
use crate::web_diagnostics::{WebDiagnosticReport, WebDiagnosticsRunner};
use crate::workspace_engine::WorkspaceEngine;
use std::sync::atomic::{AtomicU64, Ordering};

static COUNTER: AtomicU64 = AtomicU64::new(1);

fn temp_repo(name: &str) -> PathBuf {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock should work")
        .as_nanos();
    let dir = std::env::temp_dir().join(format!(
        "damaian-mode-tool-list-{name}-{now}-{}",
        COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("README.md"), "# Mode tool-list test\n").unwrap();
    dir
}

fn native_provider() -> ModelProviderConfig {
    ModelProviderConfig {
        id: "openai".to_string(),
        label: "OpenAI".to_string(),
        base_url: String::new(),
        api_key_env: String::new(),
        models: Vec::new(),
        supports_native_tools: true,
        max_output_tokens: None,
        context_token_budget: None,
        context_window: None,
        provider_reports_usage: true,
        price_per_million_input_tokens: None,
        price_per_million_output_tokens: None,
        price_per_million_cached_input_tokens: None,
        supports_explicit_cache_breakpoints: false,
    }
}

fn engine_with(repo: &Path) -> WorkspaceEngine {
    let mut config = Config {
        data_dir: repo.join(".damaian"),
        enable_index_watcher: false,
        ..Config::default()
    };
    config.model_providers.push(native_provider());
    WorkspaceEngine::new(config)
}

#[derive(Debug, Clone)]
struct StaticWebRunner;

impl WebDiagnosticsRunner for StaticWebRunner {
    fn inspect(&self, _call: &WebDiagnosticCall) -> Result<WebDiagnosticReport> {
        Ok(WebDiagnosticReport::from_text("diagnostics", false))
    }

    fn run_scenario(&self, _call: &WebDiagnosticCall) -> Result<WebDiagnosticReport> {
        Ok(WebDiagnosticReport::from_text("scenario", false))
    }
}

/// Runs a warm-up turn so a session exists, sets its mode, then runs the
/// turn whose offer list is returned — the mode is set on a session that
/// already exists, which is why this is two turns rather than one.
fn offered_tool_names(engine: &WorkspaceEngine, repo: &Path, mode: SessionMode) -> Vec<String> {
    let mut warm = MockModelAdapter::new("Ready.");
    let mut on_token = |_token: &str| {};
    let first = engine
        .chat_orchestrator
        .ask(repo, "warm up", &[], &mut warm, &mut on_token)
        .unwrap();
    engine
        .session_store
        .set_session_mode(&first.session.id, mode, "user")
        .unwrap();

    let mut adapter = MockModelAdapter::new("Done.");
    let cancel = CancelToken::new();
    let mut on_progress = |_event: TurnProgress| {};
    let mut sink = TurnSink {
        on_token: &mut on_token,
        on_progress: &mut on_progress,
        cancel: &cancel,
        steering: None,
    };
    engine
        .chat_orchestrator
        .ask_with_session(
            repo,
            "Which tools can you use?",
            &[],
            Some(&first.session.id),
            &mut adapter,
            &mut sink,
        )
        .unwrap();
    adapter.requests[0]
        .tools
        .as_ref()
        .expect("native tools should be offered")
        .iter()
        .map(|tool| tool.name.clone())
        .collect()
}

/// The command-shaped and work-product tools Ask must never offer. Reads
/// stay, which is what makes the empty intersection meaningful.
const NON_READ_TOOLS: [&str; 5] = [
    "run_command",
    "propose_patch",
    "edit_file",
    "propose_plan",
    "complete_step",
];

#[test]
fn ask_mode_offers_no_run_command_tool_definition_at_all() {
    let repo = temp_repo("ask");
    let engine = engine_with(&repo);
    let names = offered_tool_names(&engine, &repo, SessionMode::Ask);

    for name in NON_READ_TOOLS {
        assert!(
            !names.iter().any(|offered| offered == name),
            "Ask must offer no {name}: {names:?}"
        );
    }
    assert!(
        names.iter().any(|name| name == "read_file"),
        "reads must remain offered, or this proves nothing: {names:?}"
    );

    fs::remove_dir_all(repo).unwrap();
}

#[test]
fn plan_mode_offers_run_command_but_not_propose_patch_or_edit_file() {
    let repo = temp_repo("plan");
    let engine = engine_with(&repo);
    let names = offered_tool_names(&engine, &repo, SessionMode::Plan);

    assert!(names.iter().any(|name| name == "run_command"), "{names:?}");
    assert!(
        !names.iter().any(|name| name == "propose_patch"),
        "{names:?}"
    );
    assert!(!names.iter().any(|name| name == "edit_file"), "{names:?}");
    // `context.md` §1's planning row is Plan and Code, so the tool that
    // keeps a plan progressing is offered here and after a Plan→Code switch.
    assert!(names.iter().any(|name| name == "propose_plan"), "{names:?}");
    assert!(
        names.iter().any(|name| name == "complete_step"),
        "{names:?}"
    );

    fs::remove_dir_all(repo).unwrap();
}

#[test]
fn code_mode_offers_every_native_tool() {
    let repo = temp_repo("code");
    let engine = engine_with(&repo);
    let names = offered_tool_names(&engine, &repo, SessionMode::Code);

    for expected in [
        "run_command",
        "propose_patch",
        "propose_plan",
        "complete_step",
        "read_file",
        "list_directory",
        "search_content",
        "edit_file",
        "search_codebase",
        "read_git_status",
        "read_git_diff",
    ] {
        assert!(
            names.iter().any(|name| name == expected),
            "Code must offer {expected}: {names:?}"
        );
    }

    fs::remove_dir_all(repo).unwrap();
}

#[test]
fn review_mode_offers_read_only_commands_and_web_diagnostics_but_not_mutation() {
    let repo = temp_repo("review");
    let mut engine = engine_with(&repo);
    engine
        .chat_orchestrator
        .set_web_diagnostics_runner(WebDiagnosticsRunnerHandle::new(StaticWebRunner));
    let names = offered_tool_names(&engine, &repo, SessionMode::Review);

    assert!(names.iter().any(|name| name == "run_command"), "{names:?}");
    assert!(
        names.iter().any(|name| name == "inspect_web_page"),
        "{names:?}"
    );
    assert!(
        names.iter().any(|name| name == "run_web_scenario"),
        "{names:?}"
    );
    assert!(
        !names.iter().any(|name| name == "propose_patch"),
        "{names:?}"
    );
    assert!(!names.iter().any(|name| name == "edit_file"), "{names:?}");

    fs::remove_dir_all(repo).unwrap();
}

/// A tiny stdio MCP server replying to `initialize` and `tools/list` with
/// the given tools array, reusing `tests/foundation.rs`'s
/// `mcp_stdio_client_handshakes_lists_and_calls_tools` script shape.
fn write_mcp_server(repo: &Path, tools_array: &str) -> PathBuf {
    let script = repo.join("mcp-server.sh");
    fs::write(
        &script,
        format!(
            r#"#!/bin/sh
while IFS= read -r line; do
  id=$(printf '%s' "$line" | sed 's/.*"id":\([0-9]*\).*/\1/')
  case "$line" in
    *'"method":"initialize"'*)
      printf '{{"jsonrpc":"2.0","id":%s,"result":{{"protocolVersion":"2025-06-18","capabilities":{{}},"serverInfo":{{"name":"fake","version":"0"}}}}}}\n' "$id" ;;
    *'"method":"tools/list"'*)
      printf '{{"jsonrpc":"2.0","id":%s,"result":{{"tools":{tools_array}}}}}\n' "$id" ;;
  esac
done
"#
        ),
    )
    .unwrap();
    script
}

fn engine_with_mcp(repo: &Path, tools_array: &str) -> WorkspaceEngine {
    let script = write_mcp_server(repo, tools_array);
    let mut config = Config {
        data_dir: repo.join(".damaian"),
        enable_index_watcher: false,
        ..Config::default()
    };
    config.model_providers.push(native_provider());
    config.mcp_servers.push(McpServerConfig {
        id: "fake".to_string(),
        label: "Fake".to_string(),
        transport: McpTransport::Stdio,
        command: "sh".to_string(),
        args: vec![script.to_string_lossy().to_string()],
        env: Vec::new(),
        url: String::new(),
        auth_token_env: String::new(),
        enabled: true,
        require_approval: true,
    });
    WorkspaceEngine::new(config)
}

#[test]
fn an_mcp_tool_without_a_read_only_hint_is_withheld_in_ask() {
    let repo = temp_repo("mcp-no-hint");
    let engine = engine_with_mcp(
        &repo,
        r#"[{"name":"echo","description":"Echoes text","inputSchema":{"type":"object"}}]"#,
    );

    // Code proves the server connected and produced the tool, so the Ask
    // assertion below is a withheld tool, not a server that never spoke.
    let code = offered_tool_names(&engine, &repo, SessionMode::Code);
    assert!(
        code.iter().any(|name| name == "mcp__fake__echo"),
        "the fixture must offer its tool in Code: {code:?}"
    );
    let ask = offered_tool_names(&engine, &repo, SessionMode::Ask);
    assert!(
        !ask.iter().any(|name| name == "mcp__fake__echo"),
        "an MCP tool with no read-only claim is mutation-class: {ask:?}"
    );

    fs::remove_dir_all(repo).unwrap();
}

#[test]
fn an_mcp_tool_with_a_true_read_only_hint_is_offered_in_ask() {
    let repo = temp_repo("mcp-read-only-hint");
    let engine = engine_with_mcp(
        &repo,
        r#"[{"name":"echo","description":"Echoes text","inputSchema":{"type":"object"},"annotations":{"readOnlyHint":true}}]"#,
    );

    let ask = offered_tool_names(&engine, &repo, SessionMode::Ask);
    assert!(
        ask.iter().any(|name| name == "mcp__fake__echo"),
        "a read-only-annotated MCP tool is offered in Ask: {ask:?}"
    );

    fs::remove_dir_all(repo).unwrap();
}
