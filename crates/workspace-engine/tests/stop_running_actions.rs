//! Spec 109 D8: the user's Stop ends every running action, not only commands.
//!
//! Commands are covered in `command_lifecycle.rs` and hooks in `hooks.rs`;
//! this file holds the runners that reach the turn's token through
//! [`CancelToken::scope`] rather than a parameter: MCP calls (and the browser
//! diagnostics built on them), and the stopped turn's report.

use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use workspace_engine::{
    CancelToken, ChatTurnResult, ClientError, Config, McpClient, McpServerConfig, McpTransport,
    MockModelAdapter, ModelAdapter, ModelRequest, ModelRun, ProcessRegistry, ToolCall,
    TurnProgress, TurnSink, WorkspaceEngine,
};

static COUNTER: AtomicU64 = AtomicU64::new(1);

fn temp_dir(name: &str) -> PathBuf {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock should work")
        .as_nanos();
    let dir = std::env::temp_dir().join(format!(
        "damaian-stop-{name}-{now}-{}",
        COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir_all(&dir).expect("temp dir should be created");
    dir
}

fn is_alive(pid: i32) -> bool {
    // SAFETY: signal 0 only checks that the pid exists; nothing is sent.
    unsafe { libc::kill(pid, 0) == 0 }
}

/// A stdio server that completes the handshake, then on `tools/call` starts a
/// long background child (its pid written to `pids`) and never answers: an
/// MCP call stuck part-way, as a slow side-effecting tool would be.
fn hanging_server(root: &std::path::Path, pids: &std::path::Path) -> McpServerConfig {
    let script = root.join("server.sh");
    fs::write(
        &script,
        format!(
            r#"#!/bin/sh
while IFS= read -r line; do
  id=$(printf '%s' "$line" | sed 's/.*"id":\([0-9]*\).*/\1/')
  case "$line" in
    *'"method":"initialize"'*)
      printf '{{"jsonrpc":"2.0","id":%s,"result":{{"protocolVersion":"2025-06-18","capabilities":{{}},"serverInfo":{{"name":"slow","version":"0"}}}}}}\n' "$id" ;;
    *'"method":"tools/call"'*)
      sleep 30 &
      echo $! > '{}'
      wait ;;
  esac
done
"#,
            pids.to_string_lossy()
        ),
    )
    .unwrap();
    McpServerConfig {
        id: "slow".to_string(),
        label: "Slow".to_string(),
        transport: McpTransport::Stdio,
        command: "sh".to_string(),
        args: vec![script.to_string_lossy().to_string()],
        env: Vec::new(),
        url: String::new(),
        auth_token_env: String::new(),
        enabled: true,
        require_approval: true,
    }
}

#[test]
fn a_stop_ends_a_running_mcp_call_and_its_server_group_without_a_retry() {
    let root = temp_dir("mcp");
    let pids = root.join("pids");
    let config = hanging_server(&root, &pids);
    let registry = ProcessRegistry::open(&root).expect("registry");
    let mut client = McpClient::connect(&config, None, &registry, "ses_1", Duration::from_secs(10))
        .expect("connect + initialize");

    let cancel = CancelToken::new();
    let stopper = {
        let cancel = cancel.clone();
        let pids = pids.clone();
        std::thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(20);
            while !fs::read_to_string(&pids).is_ok_and(|text| !text.trim().is_empty())
                && Instant::now() < deadline
            {
                std::thread::sleep(Duration::from_millis(10));
            }
            cancel.cancel();
        })
    };
    let started = Instant::now();
    let result = cancel.scope(|| client.call_tool("work", "{}", Duration::from_secs(60)));
    stopper.join().unwrap();

    assert_eq!(result.unwrap_err(), ClientError::Cancelled);
    assert!(
        started.elapsed() < Duration::from_secs(25),
        "the stop did not end the call before its 60 s deadline"
    );
    // A stop is never mistaken for a transient failure worth re-sending.
    assert!(!ClientError::Cancelled.is_retryable());
    let child: i32 = fs::read_to_string(&pids).unwrap().trim().parse().unwrap();
    let settle = Instant::now() + Duration::from_secs(1);
    while is_alive(child) && Instant::now() < settle {
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(!is_alive(child), "the call's child outlived the stop");

    drop(client);
    assert!(
        registry.entries().unwrap().is_empty(),
        "a stopped server leaves no registry entry"
    );
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn without_a_stop_the_same_call_runs_to_its_deadline() {
    // The control: the scoped token is what ended the call above, not the
    // server dying by itself.
    let root = temp_dir("mcp-control");
    let pids = root.join("pids");
    let config = hanging_server(&root, &pids);
    let registry = ProcessRegistry::open(&root).expect("registry");
    let mut client = McpClient::connect(&config, None, &registry, "ses_1", Duration::from_secs(10))
        .expect("connect + initialize");
    let cancel = CancelToken::new();
    let result = cancel.scope(|| client.call_tool("work", "{}", Duration::from_secs(1)));
    assert!(
        matches!(result, Err(ClientError::McpTimeout(_))),
        "{result:?}"
    );
    drop(client);
    let _ = fs::remove_dir_all(&root);
}

// ---------------------------------------------------------------------------
// The stopped turn's report names only the action the stop ended.
// ---------------------------------------------------------------------------

/// `command` runs without a card: it is on the allowlist, and a short
/// `timeout_secs` lets a test make it time out by itself.
fn turn_engine(repo: &std::path::Path, command: &str, timeout_secs: usize) -> WorkspaceEngine {
    WorkspaceEngine::new(Config {
        data_dir: repo.join(".damaian"),
        enable_index_watcher: false,
        shell: "/bin/sh".to_string(),
        command_timeout_secs: timeout_secs,
        command_allowlist: vec![command.to_string()],
        ..Config::default()
    })
}

/// One `run_command` round. The next model call is the user's Stop: it
/// cancels the turn and reports `Cancelled`, as the transport does when Stop
/// lands while the model is being asked.
struct CommandThenStop {
    command: MockModelAdapter,
    calls: usize,
}

impl ModelAdapter for CommandThenStop {
    fn stream_response(
        &mut self,
        request: &ModelRequest,
        cancel: &CancelToken,
        on_token: &mut dyn FnMut(&str),
        on_wait: &mut dyn FnMut(u64),
    ) -> workspace_engine::Result<ModelRun> {
        self.calls += 1;
        if self.calls == 1 {
            return self
                .command
                .stream_response(request, cancel, on_token, on_wait);
        }
        cancel.cancel();
        Err(ClientError::Cancelled)
    }
}

fn run_command_call(command: &str) -> MockModelAdapter {
    let arguments = serde_json::json!({ "command": command, "reason": "Wait for it" });
    MockModelAdapter::new_sequence_with_tool_calls(
        vec![String::new()],
        vec![vec![ToolCall {
            id: "call_run_command".to_string(),
            name: "run_command".to_string(),
            arguments_json: arguments.to_string(),
        }]],
    )
}

fn ask(
    engine: &WorkspaceEngine,
    repo: &std::path::Path,
    adapter: &mut dyn ModelAdapter,
    cancel: &CancelToken,
) -> ChatTurnResult {
    let (mut on_token, mut on_progress) = (|_: &str| {}, |_: TurnProgress| {});
    let mut sink = TurnSink {
        on_token: &mut on_token,
        on_progress: &mut on_progress,
        cancel,
        steering: None,
    };
    engine
        .chat_orchestrator
        .ask_with_session(repo, "Wait for it", &[], None, adapter, &mut sink)
        .expect("a stopped turn is a result, not an error")
}

#[test]
fn a_command_the_stop_ended_is_named_in_the_stopped_turns_report() {
    let repo = temp_dir("turn-stopped-command");
    // Far under its timeout, so only the stop ends it.
    let command = "sleep 30";
    let engine = turn_engine(&repo, command, 60);
    let cancel = CancelToken::new();
    // Stop once the command is running, not after a fixed delay: a stop that
    // lands during context assembly ends the turn before the model is ever
    // asked (OBSERVATIONS 107). The runner registers the child right after
    // spawning it, so its registry entry means the command is running.
    let registry = ProcessRegistry::open(repo.join(".damaian")).expect("registry");
    let stopper = {
        let cancel = cancel.clone();
        std::thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(20);
            let running = || {
                registry.entries().is_ok_and(|entries| {
                    entries
                        .iter()
                        .any(|(_, entry)| entry.as_ref().is_some_and(|e| e.kind == "command"))
                })
            };
            while !running() && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(10));
            }
            cancel.cancel();
        })
    };
    let started = Instant::now();
    let mut adapter = CommandThenStop {
        command: run_command_call(command),
        calls: 0,
    };
    let result = ask(&engine, &repo, &mut adapter, &cancel);
    stopper.join().unwrap();

    assert!(
        result.command_proposal.is_none(),
        "the command ran without a card"
    );
    assert!(result.cancelled);
    assert!(
        started.elapsed() < Duration::from_secs(25),
        "the stop did not end the command before its 60 s timeout"
    );
    assert_eq!(adapter.calls, 1, "a stopped command is not retried");
    assert!(
        result.response.contains(&format!(
            "Stopped by you while running the command `{command}`"
        )),
        "{}",
        result.response
    );
    assert!(
        result.response.contains("outcome is unknown"),
        "{}",
        result.response
    );
    let _ = fs::remove_dir_all(&repo);
}

#[test]
fn a_command_that_timed_out_earlier_is_not_blamed_for_a_later_stop() {
    let repo = temp_dir("turn-timed-out-command");
    // Past the one-second timeout: it ends `unknown` by itself.
    let command = "sleep 3";
    let engine = turn_engine(&repo, command, 1);
    let cancel = CancelToken::new();
    let mut adapter = CommandThenStop {
        command: run_command_call(command),
        calls: 0,
    };
    let result = ask(&engine, &repo, &mut adapter, &cancel);

    assert!(result.cancelled);
    assert_eq!(
        adapter.calls, 2,
        "the stop landed during the next model call"
    );
    assert!(
        !result.response.contains("Stopped by you while running"),
        "the timed-out command was named as stopped: {}",
        result.response
    );
    let _ = fs::remove_dir_all(&repo);
}
