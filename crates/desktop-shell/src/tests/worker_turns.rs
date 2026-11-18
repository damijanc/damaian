use super::*;
use crate::run_chat_request_with;
use crate::worker::{ControlCommand, WorkerHost, host_in_process};
use crate::{run_continue_worker_result_request_with, worker_return_continuation_prompt};
use std::path::Path;
use std::process::Command;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use workspace_engine::worker_progress::read_progress_after;
use workspace_engine::{
    ModelAdapter, ModelRequest, ModelRun, SecretScanner, SessionStore, TaskStatus, WorkerRecord,
    WorkerReturn,
};

static COUNTER: AtomicU64 = AtomicU64::new(1);

/// A committed one-file repository. Canonical, because the session's
/// repository id is taken from the canonical root.
fn fixture(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "dwt-{}-{tag}-{}",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("README.md"), "# Worker fixture\n").unwrap();
    for args in [
        &["init", "-q"][..],
        &["add", "."][..],
        &[
            "-c",
            "user.name=Fixture",
            "-c",
            "user.email=fixture@example.test",
            "commit",
            "-qm",
            "init",
        ][..],
    ] {
        let status = Command::new("git")
            .args(args)
            .current_dir(&dir)
            .status()
            .unwrap();
        assert!(status.success(), "git {args:?}");
    }
    fs::canonicalize(&dir).unwrap()
}

fn store() -> SessionStore {
    SessionStore::new(isolated_data_dir())
}

/// A session for `repo`; with `worker`, one whose worker record names
/// `repo` as its worktree, as Task 7's spawn will write it.
fn session_for(repo: &Path, worker: bool) -> String {
    session_for_in(repo, worker, isolated_data_dir())
}

fn session_for_in(repo: &Path, worker: bool, data_dir: &Path) -> String {
    let store = SessionStore::new(data_dir);
    let session = store
        .create_session(
            &workspace_engine::hash::repository_id_for_root(repo),
            "fixture",
        )
        .unwrap();
    if worker {
        store
            .record_worker_spawned(
                &session.id,
                &WorkerRecord {
                    parent_session_id: None,
                    parent_task_id: None,
                    inherited_protected_paths: Vec::new(),
                    source_repository: repo.to_path_buf(),
                    worktree_path: repo.to_path_buf(),
                    branch: "damaian/fixture".to_string(),
                    base_commit: "HEAD".to_string(),
                    included_uncommitted: false,
                    git_commands: Vec::new(),
                },
            )
            .unwrap();
    }
    session.id
}

fn call(name: &str, arguments_json: &str) -> ToolCall {
    ToolCall {
        id: format!("call_{name}"),
        name: name.to_string(),
        arguments_json: arguments_json.to_string(),
    }
}

/// One tool call, then a final answer.
fn calls_then(name: &str, arguments_json: &str, answer: &str) -> Scripted {
    Scripted::new(MockModelAdapter::new_sequence_with_tool_calls(
        vec![String::new(), answer.to_string()],
        vec![vec![call(name, arguments_json)], Vec::new()],
    ))
}

const PWD: &str = r#"{"command":"pwd","reason":"See where the turn runs"}"#;
const TOUCH: &str = r#"{"command":"touch approved.txt","reason":"Mark the tree"}"#;

/// A read-only command envelope the policy runs automatically, so a
/// turn of it walks the round budget without pausing for a command
/// approval (spec 61).
fn read_only_round() -> String {
    "DAMAIAN_COMMAND_V1\nCOMMAND: pwd\nREASON: Confirm the working directory.\nEND_COMMAND\n"
        .to_string()
}

/// The worker builds an adapter per job; sharing one script across
/// them lets a turn and its resume read one sequence, as one turn.
#[derive(Clone)]
struct Scripted(Arc<Mutex<MockModelAdapter>>);

impl Scripted {
    fn new(adapter: MockModelAdapter) -> Self {
        Self(Arc::new(Mutex::new(adapter)))
    }
}

impl ModelAdapter for Scripted {
    fn stream_response(
        &mut self,
        request: &ModelRequest,
        cancel: &CancelToken,
        on_token: &mut dyn FnMut(&str),
        on_wait: &mut dyn FnMut(u64),
    ) -> workspace_engine::Result<ModelRun> {
        self.0
            .lock()
            .unwrap()
            .stream_response(request, cancel, on_token, on_wait)
    }
}

/// A model that fails every call, so the engine persists `Failed` and
/// returns `Err` — the second terminal branch a worker must return.
#[derive(Clone)]
struct Failing(String);

impl ModelAdapter for Failing {
    fn stream_response(
        &mut self,
        _request: &ModelRequest,
        _cancel: &CancelToken,
        _on_token: &mut dyn FnMut(&str),
        _on_wait: &mut dyn FnMut(u64),
    ) -> workspace_engine::Result<ModelRun> {
        Err(workspace_engine::ClientError::InvalidInput(self.0.clone()))
    }
}

/// Holds the turn's first model call until opened, so a test can act
/// while a turn is certainly running.
#[derive(Clone)]
struct Gated {
    inner: Scripted,
    first: Arc<AtomicBool>,
    gate: Arc<(Mutex<(bool, bool)>, Condvar)>,
}

impl Gated {
    fn new(inner: Scripted) -> Self {
        Self {
            inner,
            first: Arc::new(AtomicBool::new(true)),
            gate: Arc::new((Mutex::new((false, false)), Condvar::new())),
        }
    }

    fn wait_entered(&self) {
        let (lock, changed) = &*self.gate;
        let (state, timeout) = changed
            .wait_timeout_while(lock.lock().unwrap(), Duration::from_secs(30), |state| {
                !state.0
            })
            .unwrap();
        assert!(
            state.0 && !timeout.timed_out(),
            "the turn never called the model"
        );
    }

    fn open(&self) {
        let (lock, changed) = &*self.gate;
        lock.lock().unwrap().1 = true;
        changed.notify_all();
    }
}

impl ModelAdapter for Gated {
    fn stream_response(
        &mut self,
        request: &ModelRequest,
        cancel: &CancelToken,
        on_token: &mut dyn FnMut(&str),
        on_wait: &mut dyn FnMut(u64),
    ) -> workspace_engine::Result<ModelRun> {
        if self.first.swap(false, Ordering::SeqCst) {
            let (lock, changed) = &*self.gate;
            let mut state = lock.lock().unwrap();
            state.0 = true;
            changed.notify_all();
            while !state.1 {
                state = changed.wait(state).unwrap();
            }
        }
        self.inner
            .stream_response(request, cancel, on_token, on_wait)
    }
}

fn hosted<A>(repo: &Path, adapter: A) -> (Arc<WorkerHost>, String)
where
    A: ModelAdapter + Clone + Send + 'static,
{
    let session = session_for(repo, true);
    let host = host_in_process(
        &session,
        isolated_data_dir(),
        Box::new(move |_config: &Config| Ok(Box::new(adapter.clone()) as Box<dyn ModelAdapter>)),
    )
    .unwrap();
    (host, session)
}

/// A worker whose record names a parent, so a terminal turn has a
/// session to return its result to (spec 100).
fn hosted_with_parent<A>(repo: &Path, adapter: A) -> (Arc<WorkerHost>, String, String)
where
    A: ModelAdapter + Clone + Send + 'static,
{
    let store = store();
    let parent = store
        .create_session(
            &workspace_engine::hash::repository_id_for_root(repo),
            "Planner",
        )
        .unwrap()
        .id;
    let session = store
        .create_session(
            &workspace_engine::hash::repository_id_for_root(repo),
            "Worker",
        )
        .unwrap()
        .id;
    store
        .record_worker_spawned(
            &session,
            &WorkerRecord {
                parent_session_id: Some(parent.clone()),
                parent_task_id: None,
                inherited_protected_paths: Vec::new(),
                source_repository: repo.to_path_buf(),
                worktree_path: repo.to_path_buf(),
                branch: "damaian/fixture".to_string(),
                base_commit: "HEAD".to_string(),
                included_uncommitted: false,
                git_commands: Vec::new(),
            },
        )
        .unwrap();
    store.record_worker_child(&parent, &session).unwrap();
    let host = host_in_process(
        &session,
        isolated_data_dir(),
        Box::new(move |_config: &Config| Ok(Box::new(adapter.clone()) as Box<dyn ModelAdapter>)),
    )
    .unwrap();
    (host, session, parent)
}

fn send(host: &WorkerHost, command: ControlCommand) -> serde_json::Value {
    host.dispatch(command).0
}

fn message(text: &str) -> ControlCommand {
    ControlCommand::Message {
        text: text.to_string(),
    }
}

fn wait_for(host: &WorkerHost, done: impl Fn(&str) -> bool) -> serde_json::Value {
    let deadline = std::time::Instant::now() + Duration::from_secs(60);
    loop {
        let snapshot = host.snapshot();
        if done(snapshot["state"].as_str().unwrap()) {
            return snapshot;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the worker never settled: {snapshot}"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// Idle or waiting for approval: the turn thread has finished its job.
fn settle(host: &WorkerHost) -> serde_json::Value {
    wait_for(host, |state| !matches!(state, "running" | "paused"))
}

fn session_event_types(session: &str, after: u64) -> Vec<String> {
    let log = isolated_data_dir()
        .join("sessions")
        .join(format!("{session}.jsonl"));
    fs::read_to_string(log)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str::<serde_json::Value>(line).unwrap())
        .filter(|event| event["seq"].as_u64().unwrap() > after)
        .map(|event| event["eventType"].as_str().unwrap().to_string())
        .collect()
}

/// This session's audit events, in order. The audit log is shared by
/// every test in the binary, so it is filtered by the session and its
/// tasks.
fn audit_event_types(session: &str) -> Vec<String> {
    let tasks: Vec<String> = store()
        .read_tasks(session)
        .unwrap()
        .into_iter()
        .map(|task| task.id)
        .collect();
    fs::read_to_string(isolated_data_dir().join("audit").join("events.jsonl"))
        .unwrap()
        .lines()
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
        .filter(|event| {
            event["sessionId"] == session
                || event["taskId"]
                    .as_str()
                    .is_some_and(|task| tasks.iter().any(|id| id == task))
        })
        .map(|event| event["eventType"].as_str().unwrap().to_string())
        .collect()
}

fn progress(session: &str) -> Vec<serde_json::Value> {
    read_progress_after(isolated_data_dir(), session, 0).unwrap()
}

fn payloads(lines: &[serde_json::Value], kind: &str) -> Vec<serde_json::Value> {
    lines
        .iter()
        .filter(|line| line["kind"] == kind)
        .map(|line| line["payload"].clone())
        .collect()
}

/// Requirement 4, the load-bearing test of spec 57: one scripted turn
/// through the shell's `run_chat_request` and through the worker host
/// writes the same session-log events and the same audit events, in the
/// same order. Ids and timestamps differ and are not compared.
#[test]
fn a_turn_through_the_worker_host_matches_the_shell_host() {
    isolated_data_dir();
    let prompt = "Where does this run?";
    let script = || calls_then("run_command", PWD, "It runs in the fixture.");

    let shell_repo = fixture("sh");
    let shell_session = session_for(&shell_repo, false);
    let shell_after = store().latest_event_seq(&shell_session).unwrap();
    let form = HashMap::from([
        ("repo".to_string(), shell_repo.to_string_lossy().to_string()),
        ("prompt".to_string(), prompt.to_string()),
        ("session_id".to_string(), shell_session.clone()),
    ]);
    let (events, _discard) = std::sync::mpsc::channel();
    let shell_adapter = script();
    let shell_result = run_chat_request_with(&form, &CancelToken::new(), &events, &mut |_| {
        Ok(Box::new(shell_adapter.clone()))
    })
    .unwrap();
    assert_eq!(shell_result.task.status, TaskStatus::Complete);

    let worker_repo = fixture("wk");
    let (host, worker_session) = hosted(&worker_repo, script());
    let worker_after = store().latest_event_seq(&worker_session).unwrap();
    assert_eq!(send(&host, message(prompt))["ok"], true);
    assert_eq!(settle(&host)["state"], "idle");

    let shell_audit = audit_event_types(&shell_session);
    assert!(
        shell_audit.iter().any(|event| event == "command_proposed"),
        "the scripted turn must run its command, or this compares too little: {shell_audit:?}"
    );
    assert_eq!(
        session_event_types(&worker_session, worker_after),
        session_event_types(&shell_session, shell_after),
        "session-log events differ between the hosts"
    );
    assert_eq!(
        audit_event_types(&worker_session),
        shell_audit,
        "audit events differ between the hosts"
    );
}

/// Spec 100: a worker with a parent returns one redacted result when its
/// task completes, and a worker without a parent returns none.
#[test]
fn a_terminal_worker_task_returns_one_result_to_its_parent() {
    isolated_data_dir();
    let (host, worker_session, parent) =
        hosted_with_parent(&fixture("ret"), calls_then("run_command", PWD, "All done."));
    assert_eq!(send(&host, message("Do the task."))["ok"], true);
    assert_eq!(settle(&host)["state"], "idle");

    let returns = store().pending_worker_returns(&parent).unwrap();
    assert_eq!(returns.len(), 1, "{returns:?}");
    assert_eq!(returns[0].worker_session_id, worker_session);
    assert_eq!(returns[0].parent_session_id, parent);
    assert_eq!(returns[0].branch, "damaian/fixture");
    assert_eq!(returns[0].task_status, "complete");
    assert!(
        returns[0].response.contains("All done."),
        "{:?}",
        returns[0].response
    );
    // Re-reading the progress file and reloading the parent must not
    // append a second return.
    assert_eq!(store().pending_worker_returns(&parent).unwrap().len(), 1);

    let (host, parentless) = hosted(&fixture("ret-none"), calls_then("x", "{}", "Done."));
    assert_eq!(send(&host, message("Do the task."))["ok"], true);
    assert_eq!(settle(&host)["state"], "idle");
    assert!(
        store()
            .pending_worker_returns(&parentless)
            .unwrap()
            .is_empty(),
        "a parentless worker has nowhere to return"
    );
    assert!(
        !session_event_types(&parentless, 0)
            .iter()
            .any(|event| event == "worker_returned"),
        "a parentless worker must not write a return"
    );
}

/// Spec 100 repair: a model failure makes the engine persist a
/// terminal `Failed` task and return `Err`. That task must still return
/// exactly one redacted result, and the parentless case must not.
#[test]
fn a_failed_worker_task_returns_one_redacted_failure_to_its_parent() {
    isolated_data_dir();
    let secret = "ghp_abcdefghijklmnopqrstuvwxyz0123456789";
    let (host, worker_session, parent) = hosted_with_parent(
        &fixture("ret-failed"),
        Failing(format!("the model failed with {secret}")),
    );
    assert_eq!(send(&host, message("Do the task."))["ok"], true);
    assert_eq!(settle(&host)["state"], "idle");

    let returns = store().pending_worker_returns(&parent).unwrap();
    assert_eq!(returns.len(), 1, "{returns:?}");
    assert_eq!(returns[0].worker_session_id, worker_session);
    assert_eq!(returns[0].task_status, "failed");
    assert!(
        returns[0].response.contains("the model failed"),
        "{:?}",
        returns[0].response
    );
    assert!(
        returns[0].response.contains("[REDACTED_"),
        "{:?}",
        returns[0].response
    );
    assert!(!returns[0].response.contains(secret));

    let raw = fs::read_to_string(
        isolated_data_dir()
            .join("sessions")
            .join(format!("{parent}.jsonl")),
    )
    .unwrap();
    assert_eq!(raw.matches("\"eventType\":\"worker_returned\"").count(), 1);
    assert!(!raw.contains(secret), "the secret reached the parent log");
    // Reloading the parent must not duplicate the failed return.
    assert_eq!(store().pending_worker_returns(&parent).unwrap().len(), 1);

    let (host, parentless) = hosted(
        &fixture("ret-failed-none"),
        Failing("the model failed".to_string()),
    );
    assert_eq!(send(&host, message("Do the task."))["ok"], true);
    assert_eq!(settle(&host)["state"], "idle");
    assert!(
        store()
            .pending_worker_returns(&parentless)
            .unwrap()
            .is_empty(),
        "a parentless worker has nowhere to return"
    );
    assert!(
        !session_event_types(&parentless, 0)
            .iter()
            .any(|event| event == "worker_returned"),
        "a parentless worker must not write a return"
    );
}

/// Spec 100 requirement 2: a task waiting on a decision returns nothing
/// until the decision leads to a terminal outcome.
#[test]
fn a_waiting_worker_returns_only_after_the_decision_completes() {
    isolated_data_dir();
    let (host, _session, parent) = hosted_with_parent(
        &fixture("ret-wait"),
        calls_then("run_command", TOUCH, "Done."),
    );
    assert_eq!(send(&host, message("Mark it."))["ok"], true);
    let waiting = settle(&host);
    assert_eq!(waiting["state"], "waiting_for_approval", "{waiting}");
    assert!(
        store().pending_worker_returns(&parent).unwrap().is_empty(),
        "a waiting task must not return"
    );

    let proposal_id = waiting["proposalId"].as_str().unwrap().to_string();
    assert_eq!(
        send(
            &host,
            ControlCommand::Approve {
                proposal_id,
                budget: None,
            }
        )["ok"],
        true
    );
    assert_eq!(settle(&host)["state"], "idle");
    let returns = store().pending_worker_returns(&parent).unwrap();
    assert_eq!(returns.len(), 1, "{returns:?}");
    assert_eq!(returns[0].task_status, "complete");
}

/// The engine half of "same turn": the worker's engine gets the
/// shell's chat integrations (`chat_engine`). They change nothing a
/// turn records unless an integration is configured, so the
/// comparison above cannot see them; with a browser MCP server
/// configured, the runner they install is what offers the model
/// `inspect_web_page`. Its own data directory, because the server is
/// set in user config.
#[test]
fn a_worker_turn_gets_the_shells_chat_integrations() {
    let data_dir = std::env::temp_dir().join(format!(
        "dwt-{}-integ-{}",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = fs::remove_dir_all(&data_dir);
    fs::create_dir_all(data_dir.join("config")).unwrap();
    // Never started: the turn only lists its tools, which it offers
    // natively only to a provider configured for it.
    fs::write(
        data_dir.join("config").join("user.conf"),
        "model_provider.openai.supports_native_tools=true\n\
                 mcp_server.browser.transport=stdio\n\
                 mcp_server.browser.command=/usr/bin/false\n\
                 mcp_server.browser.enabled=true\n",
    )
    .unwrap();
    let repo = fixture("integ");
    let store = SessionStore::new(&data_dir);
    let session = store
        .create_session(
            &workspace_engine::hash::repository_id_for_root(&repo),
            "fixture",
        )
        .unwrap();
    store
        .record_worker_spawned(
            &session.id,
            &WorkerRecord {
                parent_session_id: None,
                parent_task_id: None,
                inherited_protected_paths: Vec::new(),
                source_repository: repo.clone(),
                worktree_path: repo.clone(),
                branch: "damaian/fixture".to_string(),
                base_commit: "HEAD".to_string(),
                included_uncommitted: false,
                git_commands: Vec::new(),
            },
        )
        .unwrap();
    let script = Scripted::new(MockModelAdapter::new("Nothing to inspect."));
    let recorded = script.clone();
    let host = host_in_process(
        &session.id,
        &data_dir,
        Box::new(move |_config: &Config| Ok(Box::new(script.clone()) as Box<dyn ModelAdapter>)),
    )
    .unwrap();
    assert_eq!(send(&host, message("Look at the page."))["ok"], true);
    assert_eq!(settle(&host)["state"], "idle");

    let requests = &recorded.0.lock().unwrap().requests;
    let offered: Vec<String> = requests[0]
        .tools
        .iter()
        .flatten()
        .map(|tool| tool.name.clone())
        .collect();
    assert!(
        offered.iter().any(|name| name == "inspect_web_page"),
        "the worker's engine was built without the shell's integrations: {offered:?}"
    );
}

/// §5.5: `phase` carries round and maxRounds, `tool` the tool label,
/// `text` is batched, and `status` marks each transition.
#[test]
fn progress_lines_mirror_the_turn() {
    let answer =
        "A long answer, streamed in many small pieces, so batching has something to batch.";
    let (host, session) = hosted(&fixture("prog"), calls_then("run_command", PWD, answer));
    assert_eq!(send(&host, message("Where?"))["ok"], true);
    assert_eq!(settle(&host)["state"], "idle");

    let lines = progress(&session);
    let statuses = payloads(&lines, "status");
    assert_eq!(
        statuses.first().unwrap()["state"],
        "running",
        "{statuses:?}"
    );
    let last = statuses.last().unwrap();
    assert_eq!(last["state"], "idle", "{statuses:?}");
    assert_eq!(last["taskStatus"], "complete", "{statuses:?}");

    let phases = payloads(&lines, "phase");
    assert!(!phases.is_empty());
    for phase in &phases {
        assert!(
            phase["round"].is_u64()
                && phase["maxRounds"].is_u64()
                && phase["explorationRound"].is_u64()
                && phase["maxExplorationRounds"].is_u64(),
            "phase must carry both the action and exploration counts: {phase}"
        );
    }
    let tool_phases: Vec<_> = phases
        .iter()
        .filter(|phase| phase["phase"] == "tool")
        .collect();
    let tools = payloads(&lines, "tool");
    assert!(!tools.is_empty(), "{lines:?}");
    assert_eq!(
        tools
            .iter()
            .map(|tool| tool["label"].clone())
            .collect::<Vec<_>>(),
        tool_phases
            .iter()
            .map(|phase| phase["label"].clone())
            .collect::<Vec<_>>()
    );
    // Spec 107 requirement 3: a command that runs without asking is
    // named as running once it starts, after the call that proposed it.
    let labels: Vec<&str> = tool_phases
        .iter()
        .filter_map(|phase| phase["label"].as_str())
        .collect();
    let proposing = labels
        .iter()
        .position(|label| label.starts_with("Proposing `"));
    let running = labels
        .iter()
        .position(|label| label.starts_with("Running `"));
    assert!(
        matches!((proposing, running), (Some(p), Some(r)) if p < r),
        "{labels:?}"
    );

    let texts = payloads(&lines, "text");
    let streamed: String = texts
        .iter()
        .map(|text| text["text"].as_str().unwrap())
        .collect();
    assert_eq!(streamed, answer);
    // The mock streams 24-byte tokens; one line per token is no batch.
    assert!(
        texts.len() < answer.len().div_ceil(24),
        "text was not batched: {texts:?}"
    );
}

/// §5.4: a turn waiting for approval has left its loop, so notes,
/// pause and stop do not apply, and no second turn starts over it.
#[test]
fn a_note_while_waiting_for_approval_is_refused() {
    let (host, _session) = hosted(&fixture("note"), calls_then("run_command", TOUCH, "Done."));
    assert_eq!(send(&host, message("Mark it."))["ok"], true);
    assert_eq!(settle(&host)["state"], "waiting_for_approval");

    for command in [
        ControlCommand::Note {
            text: "also this".to_string(),
        },
        ControlCommand::Pause,
        ControlCommand::Stop,
        message("another"),
    ] {
        let answer = send(&host, command.clone());
        assert_eq!(answer["ok"], false, "{command:?}");
        assert!(
            answer["error"].as_str().unwrap().contains("approval"),
            "{command:?}: {answer}"
        );
    }
    assert_eq!(host.snapshot()["state"], "waiting_for_approval");
}

#[test]
fn message_is_refused_while_a_turn_runs() {
    let gated = Gated::new(Scripted::new(MockModelAdapter::new("First.")));
    let (host, _session) = hosted(&fixture("busy"), gated.clone());
    assert_eq!(send(&host, message("one"))["ok"], true);
    gated.wait_entered();

    let refused = send(&host, message("two"));
    assert_eq!(refused["ok"], false);
    assert!(
        refused["error"]
            .as_str()
            .unwrap()
            .contains("already running"),
        "{refused}"
    );
    assert_eq!(host.snapshot()["state"], "running");

    gated.open();
    assert_eq!(settle(&host)["state"], "idle");
    assert_eq!(
        send(&host, message("two"))["ok"],
        true,
        "idle takes the next message"
    );
    assert_eq!(settle(&host)["state"], "idle");
}

/// §5.6: the turn stops on the snapshot as in the shell, and `approve`
/// resumes it in this process, through the same resume, once.
#[test]
fn a_command_approval_continues_in_process() {
    let repo = fixture("appr");
    let (host, session) = hosted(&repo, calls_then("run_command", TOUCH, "Done."));
    assert_eq!(send(&host, message("Mark it."))["ok"], true);
    let waiting = settle(&host);
    assert_eq!(waiting["state"], "waiting_for_approval", "{waiting}");
    let proposal_id = waiting["proposalId"].as_str().unwrap().to_string();
    let snapshot = isolated_data_dir()
        .join("chat")
        .join("pending")
        .join(format!("{proposal_id}.json"));
    assert!(
        snapshot.is_file(),
        "the pending snapshot is written as in the shell"
    );

    let approvals = payloads(&progress(&session), "approval");
    assert_eq!(approvals.len(), 1, "{approvals:?}");
    assert_eq!(approvals[0]["kind"], "command");
    assert_eq!(approvals[0]["proposal"]["proposalId"], proposal_id.as_str());
    assert_eq!(approvals[0]["proposal"]["command"], "touch approved.txt");

    let wrong = send(
        &host,
        ControlCommand::Approve {
            proposal_id: "chatcmd_not_this_one".to_string(),
            budget: None,
        },
    );
    assert_eq!(wrong["ok"], false, "{wrong}");
    assert!(!repo.join("approved.txt").exists());

    assert_eq!(
        send(
            &host,
            ControlCommand::Approve {
                proposal_id: proposal_id.clone(),
                budget: None,
            }
        )["ok"],
        true
    );
    let done = settle(&host);
    assert_eq!(done["state"], "idle", "{done}");
    assert_eq!(
        done["pid"],
        std::process::id(),
        "the same process resumed it"
    );
    assert!(!snapshot.exists(), "the resume took the snapshot");
    assert!(
        repo.join("approved.txt").is_file(),
        "the approved command ran"
    );
    // The turn resumed rather than starting again. The second task is
    // the verification the command's new file made due (spec 85),
    // run in the worker as the end of the same resume.
    let tasks = store().read_tasks(&session).unwrap();
    assert_eq!(tasks.len(), 2, "{tasks:?}");
    assert_eq!(tasks[0].user_prompt, "Mark it.");
    // Spec 101: the command's new file made a verification due, so the
    // answered task is incomplete until the verification that follows
    // closes the chain.
    assert_eq!(tasks[0].status, TaskStatus::Incomplete);
    assert_eq!(
        store()
            .read_completion_report(&session, &tasks[0].id)
            .unwrap()
            .expect("the settlement carries its report")
            .reason_codes(),
        vec![workspace_engine::CompletionReasonCode::VerificationDue]
    );
    assert_eq!(tasks[1].status, TaskStatus::Complete);
    assert!(
        store()
            .read_verification_events(&session, &[])
            .unwrap()
            .iter()
            .any(|event| matches!(
                event,
                workspace_engine::VerificationEvent::Due { paths, .. }
                    if paths == &vec!["approved.txt".to_string()]
            )),
        "the command's file made verification due"
    );
    assert_eq!(
        send(
            &host,
            ControlCommand::Approve {
                proposal_id,
                budget: None,
            }
        )["ok"],
        false,
        "a decision resumes once"
    );
}

/// Spec 103 R17: a worker in its pre-plan phase crosses the ordinary
/// round budget in an internal segment instead of pausing for
/// **Continue**. The in-process host still runs the resumed work as the
/// same task, and no round-budget card is written. The explicit
/// round-budget resume path remains for a worker whose plan already
/// exists but whose task budget was not approved.
#[test]
fn a_worker_crosses_the_preplan_round_budget_without_pausing() {
    let repo = fixture("rb");
    // One more response than the ordinary action-round budget: the
    // boundary starts the next pre-plan segment and the last response
    // answers.
    let budget = Config::default().agent_max_tool_rounds;
    let responses: Vec<String> = (0..budget)
        .map(|_| read_only_round())
        .chain(std::iter::once("Done looking.".to_string()))
        .collect();
    let script = Scripted::new(MockModelAdapter::new_sequence(responses));
    let (host, session) = hosted(&repo, script);

    assert_eq!(send(&host, message("Look at the files."))["ok"], true);
    let done = settle(&host);
    assert_eq!(
        done["state"], "idle",
        "the pre-plan phase must not wait for a round-budget decision: {done}"
    );

    let approvals = payloads(&progress(&session), "approval");
    assert!(
        !approvals
            .iter()
            .any(|approval| approval["kind"] == "round_budget"),
        "a worker must not be offered generic Continue before a plan: {approvals:?}"
    );

    let tasks = store().read_tasks(&session).unwrap();
    assert_eq!(tasks.len(), 1, "one turn, continued internally");
    assert_eq!(tasks[0].status, TaskStatus::Complete);
}

/// §5.6: a patch ends the turn; the worker stays alive and idle, and
/// the shell's own `/api/apply-patch` applies it with the worktree as
/// `repo`.
#[test]
fn a_patch_proposal_leaves_the_worker_idle_and_alive() {
    let repo = fixture("ptch");
    let (host, session) = hosted(
        &repo,
        calls_then(
            "propose_patch",
            r#"{"summary":"Add a file","files":[{"path":"new.txt","content":"hello\n"}]}"#,
            "Proposed.",
        ),
    );
    assert_eq!(send(&host, message("Add a file."))["ok"], true);
    assert_eq!(settle(&host)["state"], "idle");

    let approvals = payloads(&progress(&session), "approval");
    assert_eq!(approvals.len(), 1, "{approvals:?}");
    assert_eq!(approvals[0]["kind"], "patch");
    let patch_id = approvals[0]["proposal"]["patchId"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(!repo.join("new.txt").exists(), "a proposal applies nothing");

    let (port, token) = serve_for_test();
    let response = post_form_for_test(
        port,
        &token,
        "/api/apply-patch",
        &format!(
            "repo={}&patch_id={patch_id}",
            percent_encode_for_test(&repo.to_string_lossy())
        ),
    );
    assert!(response.starts_with("HTTP/1.1 200"), "{response}");
    assert_eq!(fs::read_to_string(repo.join("new.txt")).unwrap(), "hello\n");

    assert_eq!(
        send(&host, message("Thanks."))["ok"],
        true,
        "the worker is still alive"
    );
    assert_eq!(settle(&host)["state"], "idle");
}

/// Spec 80: after the shell applies a worker's patch, `verify` runs the
/// verification in the worker itself, and the patch card's approval is
/// superseded by the new job.
#[test]
fn verify_runs_the_due_verification_in_the_worker() {
    let repo = fixture("vrfy");
    let (host, session) = hosted(
        &repo,
        calls_then(
            "propose_patch",
            r#"{"summary":"Add a file","files":[{"path":"new.txt","content":"hello\n"}]}"#,
            "Proposed.",
        ),
    );
    assert_eq!(
        send(&host, ControlCommand::Verify)["ok"],
        true,
        "an idle worker accepts verify"
    );
    assert_eq!(settle(&host)["state"], "idle");
    assert!(
        payloads(&progress(&session), "status")
            .iter()
            .any(|status| status["error"]
                .as_str()
                .is_some_and(|error| error.contains("No verification is due"))),
        "with nothing applied there is nothing to verify"
    );

    assert_eq!(send(&host, message("Add a file."))["ok"], true);
    assert_eq!(settle(&host)["state"], "idle");
    let patch_id = payloads(&progress(&session), "approval")[0]["proposal"]["patchId"]
        .as_str()
        .unwrap()
        .to_string();
    let (port, token) = serve_for_test();
    let response = post_form_for_test(
        port,
        &token,
        "/api/apply-patch",
        &format!(
            "repo={}&patch_id={patch_id}",
            percent_encode_for_test(&repo.to_string_lossy())
        ),
    );
    assert!(response.starts_with("HTTP/1.1 200"), "{response}");

    assert_eq!(send(&host, ControlCommand::Verify)["ok"], true);
    assert_eq!(settle(&host)["state"], "idle");
    let events = store().read_verification_events(&session, &[]).unwrap();
    assert!(
        events
            .iter()
            .any(|event| matches!(event, workspace_engine::VerificationEvent::Finished { .. })),
        "the worker ran the verification: {events:?}"
    );
    let statuses = payloads(&progress(&session), "status");
    assert_eq!(
        statuses
            .iter()
            .filter(|status| status["state"] == "running")
            .count(),
        3,
        "each job, verify included, is a new running job: {statuses:?}"
    );
}

/// Spec 96: a worker accepts the explicit continuation, and its engine
/// refuses when the last verification did not reach the limit. The
/// refusal is recorded as the job's error and the worker stays idle.
#[test]
fn a_worker_accepts_continue_repair_and_the_engine_gates_it() {
    let (host, session) = hosted(&fixture("cont"), calls_then("x", "{}", "Done."));
    assert_eq!(
        send(&host, ControlCommand::ContinueRepair)["ok"],
        true,
        "an idle worker accepts continue-repair"
    );
    assert_eq!(settle(&host)["state"], "idle");
    let statuses = payloads(&progress(&session), "status");
    assert!(
        statuses
            .first()
            .is_some_and(|status| status["state"] == "running"),
        "the job starts as running: {statuses:?}"
    );
    assert!(
        statuses.iter().any(|status| status["error"]
            .as_str()
            .is_some_and(|error| error.contains("did not reach the repair limit"))),
        "the engine's own refusal is recorded: {statuses:?}"
    );
}

/// Spec 101 Task 3: the worker host carries the exact task id from the
/// rendered control. A stale click for an older incomplete task is
/// refused and must not continue the newer incomplete task that
/// happens to be latest when the command runs.
#[test]
fn a_worker_continues_only_the_exact_task_its_control_named() {
    let (host, session) = hosted(&fixture("ctask"), calls_then("x", "{}", "Done."));
    let store = store();
    let incomplete = |prompt: &str| {
        let task = store.create_task(&session, prompt, "mock", "mock").unwrap();
        store
            .update_task_status_with_completion(
                &task,
                TaskStatus::Incomplete,
                &CompletionGateReport {
                    task_id: task.id.clone(),
                    eligible: false,
                    reasons: vec![CompletionReason {
                        code: CompletionReasonCode::OpenPlanSteps,
                        explanation: "1 planned step is still open.".to_string(),
                    }],
                    waivers: Vec::new(),
                    accepted_failures: Vec::new(),
                },
            )
            .unwrap();
        task
    };
    let older = incomplete("Older incomplete task");
    let newer = incomplete("Newer incomplete task");

    assert_eq!(
        send(
            &host,
            ControlCommand::ContinueTask {
                task_id: older.id.clone(),
                note: Some("keep going".to_string()),
            }
        )["ok"],
        true,
        "an idle worker accepts continue-task"
    );
    assert_eq!(settle(&host)["state"], "idle");
    let statuses = payloads(&progress(&session), "status");
    assert!(
        statuses.iter().any(|status| status["error"]
            .as_str()
            .is_some_and(|error| error.contains("newest incomplete task"))),
        "the stale target is refused: {statuses:?}"
    );
    assert_eq!(
        store.read_tasks(&session).unwrap().len(),
        2,
        "the stale click creates no task for the newer one"
    );

    assert_eq!(
        send(
            &host,
            ControlCommand::ContinueTask {
                task_id: newer.id.clone(),
                note: Some("finish it".to_string()),
            }
        )["ok"],
        true
    );
    assert_eq!(settle(&host)["state"], "idle");
    let tasks = store.read_tasks(&session).unwrap();
    assert_eq!(tasks.len(), 3, "{tasks:?}");
    assert_ne!(tasks[2].id, older.id);
    assert_ne!(tasks[2].id, newer.id);
    assert!(
        tasks[2]
            .user_prompt
            .contains("1 planned step is still open"),
        "the continuation prompt came from the named task's report: {}",
        tasks[2].user_prompt
    );
}

#[test]
fn a_check_card_is_answered_with_checks_not_approve() {
    let (host, _session) = hosted(&fixture("chks"), calls_then("x", "{}", "Done."));
    assert_eq!(
        send(
            &host,
            ControlCommand::Checks {
                proposal_id: "checkbatch_none".to_string(),
                decisions: "[]".to_string(),
            }
        )["ok"],
        false,
        "no check card is pending"
    );
}

/// `pause` holds the turn at the next round boundary, a note queued
/// there reaches the conversation, and `resume` continues the turn.
#[test]
fn pause_holds_the_turn_and_a_note_reaches_it() {
    let gated = Gated::new(calls_then("run_command", PWD, "Done."));
    let (host, session) = hosted(&fixture("paus"), gated.clone());
    assert_eq!(send(&host, message("Where?"))["ok"], true);
    gated.wait_entered();
    assert_eq!(send(&host, ControlCommand::Pause)["ok"], true);
    assert_eq!(
        host.snapshot()["state"],
        "running",
        "a pause takes effect at the boundary, not mid-call"
    );
    gated.open();
    wait_for(&host, |state| state == "paused");
    assert_eq!(
        send(
            &host,
            ControlCommand::Note {
                text: "Mind the README.".to_string()
            }
        )["ok"],
        true
    );
    assert_eq!(
        send(&host, ControlCommand::Resume { note: None })["ok"],
        true
    );
    assert_eq!(settle(&host)["state"], "idle");

    let messages = store().read_messages(&session).unwrap();
    assert!(
        messages
            .iter()
            .any(|message| message.role == "user" && message.content == "Mind the README."),
        "the note is a user message in the session"
    );
    let lines = progress(&session);
    let states: Vec<_> = payloads(&lines, "status")
        .iter()
        .map(|status| status["state"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(
        states,
        ["running", "paused", "running", "idle"],
        "{lines:?}"
    );
    assert_eq!(payloads(&lines, "note").len(), 1, "{lines:?}");
}

/// A note accepted while the round runs, in a round that then stops
/// for approval, reaches the resumed turn: the approval continues the
/// same turn (§5.6), so the note joins it at its next boundary like
/// any other (§5.4). Task 10 discovery 3.
#[test]
fn a_note_queued_before_an_approval_reaches_the_resumed_turn() {
    let script = calls_then("run_command", TOUCH, "Done.");
    let gated = Gated::new(script.clone());
    let (host, session) = hosted(&fixture("nappr"), gated.clone());
    assert_eq!(send(&host, message("Mark it."))["ok"], true);
    gated.wait_entered();
    assert_eq!(
        send(
            &host,
            ControlCommand::Note {
                text: "Mind the README.".to_string()
            }
        )["ok"],
        true,
        "a note is accepted while the round runs"
    );
    gated.open();
    let waiting = settle(&host);
    assert_eq!(waiting["state"], "waiting_for_approval", "{waiting}");
    let proposal_id = waiting["proposalId"].as_str().unwrap().to_string();
    assert_eq!(
        send(
            &host,
            ControlCommand::Approve {
                proposal_id,
                budget: None,
            }
        )["ok"],
        true
    );
    assert_eq!(settle(&host)["state"], "idle");

    let messages = store().read_messages(&session).unwrap();
    assert!(
        messages
            .iter()
            .any(|message| message.role == "user" && message.content == "Mind the README."),
        "the note was dropped at the approval: {messages:?}"
    );
    let requests = &script.0.lock().unwrap().requests;
    assert!(
        requests
            .last()
            .unwrap()
            .messages
            .iter()
            .any(|message| message.content.contains("Mind the README.")),
        "the resumed turn's next model request carries the note"
    );
    assert_eq!(payloads(&progress(&session), "note").len(), 1);
}

#[test]
fn stop_ends_the_running_turn_cancelled() {
    let gated = Gated::new(calls_then("run_command", PWD, "Done."));
    let (host, session) = hosted(&fixture("stop"), gated.clone());
    assert_eq!(send(&host, message("Where?"))["ok"], true);
    gated.wait_entered();
    assert_eq!(send(&host, ControlCommand::Stop)["ok"], true);
    gated.open();
    assert_eq!(settle(&host)["state"], "idle");
    let statuses = payloads(&progress(&session), "status");
    assert_eq!(
        statuses.last().unwrap()["taskStatus"],
        "cancelled",
        "{statuses:?}"
    );
    // The next turn has its own cancel token.
    assert_eq!(send(&host, message("Again."))["ok"], true);
    let statuses = {
        settle(&host);
        payloads(&progress(&session), "status")
    };
    assert_eq!(
        statuses.last().unwrap()["taskStatus"],
        "complete",
        "{statuses:?}"
    );
}

/// Spec 100: consuming a pending return starts one ordinary parent
/// turn with the delimited result, records which task handled it, and
/// refuses a second consume. It merges, applies and approves nothing.
#[test]
fn continuing_a_worker_result_starts_one_parent_turn_and_refuses_a_second() {
    isolated_data_dir();
    let repo = fixture("continue-result");
    let parent = store()
        .create_session(
            &workspace_engine::hash::repository_id_for_root(&repo),
            "Planner",
        )
        .unwrap()
        .id;
    let returned = WorkerReturn::new(
        &parent,
        "ses_worker",
        "task_worker",
        "damaian/worker",
        "/code/app/.damaian/worktrees/worker",
        "complete",
        "The worker finished the parser fix.",
    );
    assert!(
        store()
            .record_worker_return(&returned, &SecretScanner::default())
            .unwrap()
    );

    let form = HashMap::from([
        ("repo".to_string(), repo.to_string_lossy().to_string()),
        ("session_id".to_string(), parent.clone()),
        ("return_id".to_string(), returned.return_id.clone()),
    ]);
    let (events, _discard) = std::sync::mpsc::channel();
    let adapter = MockModelAdapter::new("Parent acknowledges the result.");
    let result =
        run_continue_worker_result_request_with(&form, &CancelToken::new(), &events, &mut |_| {
            Ok(Box::new(adapter.clone()))
        })
        .unwrap();
    assert_eq!(result.response, "Parent acknowledges the result.");
    assert_eq!(result.task.status, TaskStatus::Complete);

    let messages = store().read_messages(&parent).unwrap();
    let prompt = messages
        .iter()
        .find(|message| message.role == "user")
        .expect("the continuation is a user message");
    assert!(prompt.content.contains("<<<WORKER_RESULT>>>"), "{prompt:?}");
    assert!(
        prompt
            .content
            .contains("The worker finished the parser fix."),
        "{prompt:?}"
    );
    assert!(prompt.content.contains("not an approval"), "{prompt:?}");
    assert_eq!(
        prompt.content,
        worker_return_continuation_prompt(&returned),
        "the stored turn is the prompt the card displays"
    );
    assert!(store().pending_worker_returns(&parent).unwrap().is_empty());
    let tasks = store().read_tasks(&parent).unwrap();
    assert_eq!(tasks.len(), 1, "{tasks:?}");
    assert_eq!(tasks[0].id, result.task.id);
    let raw = fs::read_to_string(
        isolated_data_dir()
            .join("sessions")
            .join(format!("{parent}.jsonl")),
    )
    .unwrap();
    assert_eq!(
        raw.matches("\"eventType\":\"task_created\"").count(),
        0,
        "the continuation is created by the consumed event, not task_created"
    );
    assert_eq!(
        raw.matches("\"eventType\":\"worker_return_consumed\"")
            .count(),
        1
    );
    assert!(
        raw.contains(&format!("\"taskId\":\"{}\"", result.task.id)),
        "the consumed marker records the parent task"
    );

    let (events, _discard) = std::sync::mpsc::channel();
    let refused =
        run_continue_worker_result_request_with(&form, &CancelToken::new(), &events, &mut |_| {
            Ok(Box::new(MockModelAdapter::new("unused")))
        })
        .unwrap_err();
    assert!(refused.contains("already have been used"), "{refused}");
}

/// Spec 101 Task 3: the ordinary route passes the target and optional
/// note to the engine, which builds the prompt and refuses a second
/// click.
#[test]
fn continuing_an_incomplete_task_through_the_route_carries_plan_and_reasons() {
    isolated_data_dir();
    let repo = fixture("continue-task-route");
    let session = store()
        .create_session(
            &workspace_engine::hash::repository_id_for_root(&repo),
            "Planner",
        )
        .unwrap()
        .id;
    let task = store()
        .create_task(&session, "Finish the parser", "mock", "mock")
        .unwrap();
    let mut plan = TaskPlan::new(task.id.clone(), 0);
    plan.steps.push(PlanStep {
        id: "finish".to_string(),
        title: "Finish the parser".to_string(),
        detail: None,
        status: StepStatus::InProgress,
        depends_on: Vec::new(),
        started_at_ms: None,
        completed_at_ms: None,
        evidence: Vec::new(),
        check: None,
        build: None,
    });
    store().create_plan(&task, &plan).unwrap();
    store()
        .update_task_status_with_completion(
            &task,
            TaskStatus::Incomplete,
            &CompletionGateReport {
                task_id: task.id.clone(),
                eligible: false,
                reasons: vec![CompletionReason {
                    code: CompletionReasonCode::OpenPlanSteps,
                    explanation: "1 planned step is still open: \"Finish the parser\".".to_string(),
                }],
                waivers: Vec::new(),
                accepted_failures: Vec::new(),
            },
        )
        .unwrap();

    let form = HashMap::from([
        ("repo".to_string(), repo.to_string_lossy().to_string()),
        ("session_id".to_string(), session.clone()),
        ("task_id".to_string(), task.id.clone()),
        ("note".to_string(), "Keep the tokenizer".to_string()),
    ]);
    let (events, _discard) = std::sync::mpsc::channel();
    let adapter = MockModelAdapter::new("I will continue.");
    let result = run_continue_task_request_with(&form, &CancelToken::new(), &events, &mut |_| {
        Ok(Box::new(adapter.clone()))
    })
    .unwrap();
    assert_ne!(result.task.id, task.id);
    assert!(
        result.task.user_prompt.contains("Finish the parser"),
        "{}",
        result.task.user_prompt
    );
    assert!(
        result.task.user_prompt.contains("Keep the tokenizer"),
        "{}",
        result.task.user_prompt
    );
    assert_eq!(
        store().read_tasks(&session).unwrap().len(),
        2,
        "the continuation is one new task"
    );

    let (events, _discard) = std::sync::mpsc::channel();
    let refused = run_continue_task_request_with(&form, &CancelToken::new(), &events, &mut |_| {
        Ok(Box::new(MockModelAdapter::new("unused")))
    })
    .unwrap_err();
    assert!(refused.contains("newest incomplete task"), "{refused}");
    assert_eq!(
        store().read_tasks(&session).unwrap().len(),
        2,
        "a stale click creates no task"
    );
}

/// Spec 100 repair: the route must reject a parent session from another
/// repository before it builds an adapter or consumes the return.
#[test]
fn a_wrong_repository_continuation_leaves_the_return_pending() {
    isolated_data_dir();
    let repo = fixture("continue-right-repo");
    let other = fixture("continue-wrong-repo");
    let parent = store()
        .create_session(
            &workspace_engine::hash::repository_id_for_root(&other),
            "Planner",
        )
        .unwrap()
        .id;
    let returned = WorkerReturn::new(
        &parent,
        "ses_worker",
        "task_worker",
        "damaian/worker",
        "/code/app/.damaian/worktrees/worker",
        "complete",
        "The worker finished.",
    );
    assert!(
        store()
            .record_worker_return(&returned, &SecretScanner::default())
            .unwrap()
    );
    let form = HashMap::from([
        ("repo".to_string(), repo.to_string_lossy().to_string()),
        ("session_id".to_string(), parent.clone()),
        ("return_id".to_string(), returned.return_id.clone()),
    ]);
    let calls = Arc::new(AtomicU64::new(0));
    let adapter_calls = calls.clone();
    let (events, _discard) = std::sync::mpsc::channel();
    let error =
        run_continue_worker_result_request_with(&form, &CancelToken::new(), &events, &mut |_| {
            adapter_calls.fetch_add(1, Ordering::SeqCst);
            Ok(Box::new(MockModelAdapter::new("unused")))
        })
        .unwrap_err();
    assert!(
        error.contains("different repository"),
        "the route must name the mismatch: {error}"
    );
    assert_eq!(
        calls.load(Ordering::SeqCst),
        0,
        "validation must happen before adapter construction"
    );
    assert_eq!(
        store().pending_worker_returns(&parent).unwrap().len(),
        1,
        "the return must stay pending"
    );
    assert!(
        store().read_tasks(&parent).unwrap().is_empty(),
        "no parent task may be created"
    );
}

/// Spec 100 repair: a model failure after the atomic consume leaves the
/// return consumed and one failed parent task, not a runnable card. A
/// second consume and a fresh store must both refuse without another
/// model call.
#[test]
fn a_failed_continuation_keeps_the_return_consumed_with_one_failed_task() {
    isolated_data_dir();
    let repo = fixture("continue-failure");
    let parent = store()
        .create_session(
            &workspace_engine::hash::repository_id_for_root(&repo),
            "Planner",
        )
        .unwrap()
        .id;
    let returned = WorkerReturn::new(
        &parent,
        "ses_worker",
        "task_worker",
        "damaian/worker",
        "/code/app/.damaian/worktrees/worker",
        "complete",
        "The worker finished the parser fix.",
    );
    assert!(
        store()
            .record_worker_return(&returned, &SecretScanner::default())
            .unwrap()
    );
    let form = HashMap::from([
        ("repo".to_string(), repo.to_string_lossy().to_string()),
        ("session_id".to_string(), parent.clone()),
        ("return_id".to_string(), returned.return_id.clone()),
    ]);

    let calls = Arc::new(AtomicU64::new(0));
    let first_calls = calls.clone();
    let (events, _discard) = std::sync::mpsc::channel();
    let failed =
        run_continue_worker_result_request_with(&form, &CancelToken::new(), &events, &mut |_| {
            first_calls.fetch_add(1, Ordering::SeqCst);
            Ok(Box::new(Failing("the provider refused".to_string())))
        })
        .unwrap_err();
    assert!(failed.contains("provider refused"), "{failed}");
    assert_eq!(calls.load(Ordering::SeqCst), 1);

    assert!(
        store().pending_worker_returns(&parent).unwrap().is_empty(),
        "the return is consumed even when the turn fails"
    );
    let tasks = store().read_tasks(&parent).unwrap();
    assert_eq!(tasks.len(), 1, "{tasks:?}");
    assert_eq!(tasks[0].status, TaskStatus::Failed);
    let raw = fs::read_to_string(
        isolated_data_dir()
            .join("sessions")
            .join(format!("{parent}.jsonl")),
    )
    .unwrap();
    assert_eq!(
        raw.matches("\"eventType\":\"task_created\"").count(),
        0,
        "the continuation is created by the consumed event, not task_created"
    );
    assert_eq!(
        raw.matches("\"eventType\":\"worker_return_consumed\"")
            .count(),
        1
    );

    let second_calls = calls.clone();
    let (events, _discard) = std::sync::mpsc::channel();
    let refused =
        run_continue_worker_result_request_with(&form, &CancelToken::new(), &events, &mut |_| {
            second_calls.fetch_add(1, Ordering::SeqCst);
            Ok(Box::new(MockModelAdapter::new("unused")))
        })
        .unwrap_err();
    assert!(refused.contains("already have been used"), "{refused}");
    assert_eq!(
        calls.load(Ordering::SeqCst),
        1,
        "a refused consume must not call the model again"
    );
    assert_eq!(store().read_tasks(&parent).unwrap().len(), 1);

    // A restarted process sees the same durable state.
    let restarted = SessionStore::new(isolated_data_dir());
    assert!(
        restarted
            .pending_worker_returns(&parent)
            .unwrap()
            .is_empty()
    );
    assert!(
        restarted
            .consume_worker_return_and_create_task(
                &parent,
                &returned.return_id,
                "continuation",
                "provider",
                "model",
            )
            .unwrap()
            .is_none()
    );
    assert_eq!(restarted.read_tasks(&parent).unwrap().len(), 1);
}
