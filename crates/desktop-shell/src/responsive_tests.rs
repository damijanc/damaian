//! Spec 110 Task 1: the shell answers the UI while sessions work.
//!
//! Each test serves the real `serve_listener` (a thread per connection) on an
//! ephemeral port, with the binary's isolated data directory, and runs turns
//! against a model that waits until the test opens its gate. The gate is keyed
//! by the model name the turn asks for (the `model` form field), unique per
//! test, so tests running side by side never share one.

use super::*;
use std::collections::HashMap;
use std::io::{BufRead, BufReader};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::{Duration, Instant};
use workspace_engine::{ModelRequest, ModelRun};

/// One repository's gate: turns there wait at the model until it opens.
#[derive(Default)]
pub(super) struct Gate {
    open: AtomicBool,
    started: AtomicUsize,
}

fn gates() -> &'static Mutex<HashMap<String, Arc<Gate>>> {
    static GATES: OnceLock<Mutex<HashMap<String, Arc<Gate>>>> = OnceLock::new();
    GATES.get_or_init(|| Mutex::new(HashMap::new()))
}

/// A new gate and the model name that selects it.
fn new_gate(name: &str) -> (String, Arc<Gate>) {
    let model = format!(
        "gated-{name}-{}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );
    let gate = Arc::new(Gate::default());
    gates().lock().unwrap().insert(model.clone(), gate.clone());
    (model, gate)
}

/// `chat_model_adapter`'s test hook: a turn that asks for a gated model gets
/// one that waits for its gate, then answers "Done.".
pub(super) fn gated_adapter_for(config: &Config) -> Option<Box<dyn ModelAdapter>> {
    if let Some(script) = scripts().lock().unwrap().get(&config.model_name).cloned() {
        return Some(Box::new(ScriptedAdapter { script }));
    }
    let gate = gates().lock().unwrap().get(&config.model_name)?.clone();
    Some(Box::new(GatedAdapter { gate }))
}

/// A model that answers a fixed sequence across a turn and its resumes, each
/// call waiting until the test releases it, and keeps every request it got.
pub(super) struct Script {
    steps: Vec<(String, Vec<workspace_engine::ToolCall>)>,
    started: AtomicUsize,
    released: AtomicUsize,
    requests: Mutex<Vec<ModelRequest>>,
}

impl Script {
    fn release(&self) {
        self.released.fetch_add(1, Ordering::SeqCst);
    }

    fn started(&self) -> usize {
        self.started.load(Ordering::SeqCst)
    }

    /// Whether request `index` carried a user message saying exactly `text`.
    fn request_has_user_message(&self, index: usize, text: &str) -> bool {
        self.requests
            .lock()
            .unwrap()
            .get(index)
            .is_some_and(|request| {
                request
                    .messages
                    .iter()
                    .any(|message| message.role == "user" && message.content == text)
            })
    }
}

fn scripts() -> &'static Mutex<HashMap<String, Arc<Script>>> {
    static SCRIPTS: OnceLock<Mutex<HashMap<String, Arc<Script>>>> = OnceLock::new();
    SCRIPTS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn tool_call(name: &str, arguments_json: &str) -> workspace_engine::ToolCall {
    workspace_engine::ToolCall {
        id: format!("call_{name}"),
        name: name.to_string(),
        arguments_json: arguments_json.to_string(),
    }
}

/// One tool call, then a final answer.
fn new_script(name: &str, tool: &str, arguments_json: &str, answer: &str) -> (String, Arc<Script>) {
    let (model, _) = new_gate(name);
    let script = Arc::new(Script {
        steps: vec![
            (String::new(), vec![tool_call(tool, arguments_json)]),
            (answer.to_string(), Vec::new()),
        ],
        started: AtomicUsize::new(0),
        released: AtomicUsize::new(0),
        requests: Mutex::new(Vec::new()),
    });
    scripts()
        .lock()
        .unwrap()
        .insert(model.clone(), script.clone());
    (model, script)
}

struct ScriptedAdapter {
    script: Arc<Script>,
}

impl ModelAdapter for ScriptedAdapter {
    fn stream_response(
        &mut self,
        request: &ModelRequest,
        cancel: &CancelToken,
        on_token: &mut dyn FnMut(&str),
        on_wait: &mut dyn FnMut(u64),
    ) -> workspace_engine::Result<ModelRun> {
        let index = self.script.started.fetch_add(1, Ordering::SeqCst);
        self.script.requests.lock().unwrap().push(request.clone());
        let deadline = Instant::now() + Duration::from_secs(60);
        while self.script.released.load(Ordering::SeqCst) <= index {
            if cancel.is_cancelled() || Instant::now() > deadline {
                return Err(workspace_engine::ClientError::InvalidInput(
                    "the scripted model was cancelled".to_string(),
                ));
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        let (response, calls) = self
            .script
            .steps
            .get(index)
            .cloned()
            .unwrap_or_else(|| ("Done.".to_string(), Vec::new()));
        workspace_engine::MockModelAdapter::new_sequence_with_tool_calls(
            vec![response],
            vec![calls],
        )
        .stream_response(request, cancel, on_token, on_wait)
    }
}

struct GatedAdapter {
    gate: Arc<Gate>,
}

impl ModelAdapter for GatedAdapter {
    fn stream_response(
        &mut self,
        request: &ModelRequest,
        cancel: &CancelToken,
        on_token: &mut dyn FnMut(&str),
        on_wait: &mut dyn FnMut(u64),
    ) -> workspace_engine::Result<ModelRun> {
        self.gate.started.fetch_add(1, Ordering::SeqCst);
        let deadline = Instant::now() + Duration::from_secs(60);
        while !self.gate.open.load(Ordering::SeqCst) {
            if cancel.is_cancelled() || Instant::now() > deadline {
                return Err(workspace_engine::ClientError::InvalidInput(
                    "the waiting model was cancelled".to_string(),
                ));
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        workspace_engine::MockModelAdapter::new("Done.")
            .stream_response(request, cancel, on_token, on_wait)
    }
}

fn wait_until(what: &str, mut condition: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(30);
    while !condition() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(10));
    }
}

struct Shell {
    port: u16,
    token: String,
}

impl Shell {
    fn start() -> Self {
        super::tests::isolated_data_dir();
        let options = ShellOptions::new(0, None);
        let token = options.api_token.clone();
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind test listener");
        let port = listener.local_addr().expect("local addr").port();
        spawn_with_shell_stack("test-shell", move || serve_listener(listener, &options))
            .expect("spawn the test shell");
        Shell { port, token }
    }

    fn request(&self, method: &str, path: &str, fields: &[(&str, &str)]) -> TcpStream {
        let body = fields
            .iter()
            .map(|(key, value)| format!("{key}={}", encode(value)))
            .collect::<Vec<_>>()
            .join("&");
        let request = format!(
            "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1\r\ncontent-type: application/x-www-form-urlencoded\r\nx-damaian-api-token: {}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
            self.token,
            body.len()
        );
        let mut stream = TcpStream::connect(("127.0.0.1", self.port)).expect("connect");
        stream
            .set_read_timeout(Some(Duration::from_secs(60)))
            .unwrap();
        stream.write_all(request.as_bytes()).expect("write request");
        stream
    }

    /// Sends a request and returns its status and JSON body, and how long the
    /// answer took.
    fn call(
        &self,
        method: &str,
        path: &str,
        fields: &[(&str, &str)],
    ) -> (u16, serde_json::Value, Duration) {
        let started = Instant::now();
        let mut stream = self.request(method, path, fields);
        let mut response = String::new();
        stream.read_to_string(&mut response).expect("read response");
        let elapsed = started.elapsed();
        let status = response
            .split_whitespace()
            .nth(1)
            .and_then(|code| code.parse().ok())
            .unwrap_or(0);
        let body = response
            .split_once("\r\n\r\n")
            .map(|(_, body)| body)
            .unwrap_or("");
        let json = serde_json::from_str(body).unwrap_or(serde_json::Value::Null);
        (status, json, elapsed)
    }

    fn create_session(&self, repo: &Path, title: &str) -> String {
        let (status, body, _) = self.call(
            "POST",
            "/api/session-create",
            &[("repo", repo.to_str().unwrap()), ("title", title)],
        );
        assert_eq!(status, 200, "{body}");
        body["session"]["id"].as_str().unwrap().to_string()
    }

    /// Opens a turn stream on `path` and reads it on another thread.
    fn stream(&self, path: &str, fields: &[(&str, &str)]) -> std::thread::JoinHandle<String> {
        let stream = self.request("POST", path, fields);
        std::thread::spawn(move || {
            let mut text = String::new();
            for line in BufReader::new(stream).lines() {
                let Ok(line) = line else { break };
                text.push_str(&line);
                text.push('\n');
            }
            text
        })
    }

    fn note(&self, session_id: &str, text: &str) -> (u16, serde_json::Value) {
        let (status, body, elapsed) = self.call(
            "POST",
            "/api/session-note",
            &[("session_id", session_id), ("text", text)],
        );
        assert!(elapsed < PROMPT, "a note is answered at once: {elapsed:?}");
        (status, body)
    }

    /// Starts a turn and returns its open stream, read on another thread.
    fn start_turn(
        &self,
        repo: &Path,
        session_id: &str,
        model: &str,
    ) -> std::thread::JoinHandle<String> {
        let stream = self.request(
            "POST",
            "/api/ask-stream",
            &[
                ("repo", repo.to_str().unwrap()),
                ("session_id", session_id),
                ("prompt", "Say done."),
                ("model", model),
            ],
        );
        std::thread::spawn(move || {
            let mut text = String::new();
            for line in BufReader::new(stream).lines() {
                let Ok(line) = line else { break };
                text.push_str(&line);
                text.push('\n');
            }
            text
        })
    }
}

fn encode(value: &str) -> String {
    value
        .bytes()
        .map(|byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b'/' => {
                (byte as char).to_string()
            }
            other => format!("%{other:02X}"),
        })
        .collect()
}

fn temp_repo(name: &str) -> PathBuf {
    let repo = std::env::temp_dir().join(format!(
        "damaian-responsive-{name}-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(repo.join(".damaian")).unwrap();
    fs::write(repo.join("README.md"), "# Responsive\n").unwrap();
    repo
}

const PROMPT: Duration = Duration::from_secs(1);

/// Acceptance: with a turn waiting on the model, the sidebar, the session,
/// archive of another session, rename and settings all answer within a
/// second; the working session is listed; a second turn and archive or delete
/// of the working session are refused at once; after the turn everything is
/// allowed again.
#[test]
fn the_ui_answers_while_a_turn_waits_and_the_working_session_refuses_a_second_turn() {
    let shell = Shell::start();
    let repo = temp_repo("ui");
    let repo_arg = repo.to_str().unwrap().to_string();
    let (model, gate) = new_gate("ui");
    let working = shell.create_session(&repo, "Working");
    let other = shell.create_session(&repo, "Other");

    let turn = shell.start_turn(&repo, &working, &model);
    wait_until("the turn to reach the model", || {
        gate.started.load(Ordering::SeqCst) > 0
    });

    for (method, path, fields) in [
        (
            "GET",
            format!("/api/sessions?repo={}", encode(&repo_arg)),
            vec![],
        ),
        ("GET", format!("/api/session?session_id={working}"), vec![]),
        (
            "GET",
            format!("/api/config?repo={}", encode(&repo_arg)),
            vec![],
        ),
        (
            "POST",
            "/api/session-rename".to_string(),
            vec![("session_id", working.as_str()), ("title", "Renamed")],
        ),
        (
            "POST",
            "/api/session-archive".to_string(),
            vec![("session_id", other.as_str())],
        ),
    ] {
        let (status, body, elapsed) = shell.call(method, &path, &fields);
        assert_eq!(status, 200, "{method} {path}: {body}");
        assert!(elapsed < PROMPT, "{method} {path} took {elapsed:?}");
    }

    let (status, body, _) = shell.call("GET", "/api/working-sessions", &[]);
    assert_eq!(status, 200);
    let row = body["sessions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["sessionId"] == working.as_str())
        .unwrap_or_else(|| panic!("the working session is listed: {body}"))
        .clone();
    assert!(!row["label"].as_str().unwrap().is_empty());
    assert_eq!(row["cancellable"], true);

    let second = shell.call(
        "POST",
        "/api/ask-stream",
        &[
            ("repo", repo_arg.as_str()),
            ("session_id", working.as_str()),
            ("prompt", "Again."),
            ("model", model.as_str()),
        ],
    );
    assert_eq!(second.0, 409, "{}", second.1);
    assert_eq!(second.1["code"], "session_working");
    assert!(second.2 < PROMPT, "refused at once, not queued");

    for (path, extra) in [
        ("/api/session-archive", vec![]),
        ("/api/session-delete", vec![("repo", repo_arg.as_str())]),
    ] {
        let mut fields = vec![("session_id", working.as_str())];
        fields.extend(extra);
        let (status, body, _) = shell.call("POST", path, &fields);
        assert_eq!(status, 409, "{path}: {body}");
        assert!(
            body["error"].as_str().unwrap().contains("Stop it first"),
            "{path}: {body}"
        );
    }

    gate.open.store(true, Ordering::SeqCst);
    let streamed = turn.join().unwrap();
    assert!(streamed.contains("event: done"), "{streamed}");
    wait_until("the lane to be released", || {
        lanes::working_label(&working).is_none()
    });
    let (status, body, _) = shell.call("POST", "/api/session-archive", &[("session_id", &working)]);
    assert_eq!(status, 200, "archive is allowed once the work ends: {body}");
}

/// Two sessions in one project work at the same time: both reach the model
/// before either is allowed to answer, and both finish. (Each in its own
/// process is spec 110 Task 1b.)
#[test]
fn two_sessions_in_one_project_work_at_the_same_time() {
    let shell = Shell::start();
    let repo = temp_repo("two");
    let (model, gate) = new_gate("two");
    let first = shell.create_session(&repo, "First");
    let second = shell.create_session(&repo, "Second");

    let first_turn = shell.start_turn(&repo, &first, &model);
    let second_turn = shell.start_turn(&repo, &second, &model);
    wait_until("both turns to reach the model", || {
        gate.started.load(Ordering::SeqCst) >= 2
    });
    assert!(lanes::working_label(&first).is_some());
    assert!(lanes::working_label(&second).is_some());

    gate.open.store(true, Ordering::SeqCst);
    for turn in [first_turn, second_turn] {
        let streamed = turn.join().unwrap();
        assert!(streamed.contains("event: done"), "{streamed}");
    }
}

/// Spec 110 D3: a UI request reaches running work and stops it mid-turn; the
/// session's lane is free afterwards.
#[test]
fn a_ui_request_cancels_a_working_sessions_turn() {
    let shell = Shell::start();
    let repo = temp_repo("cancel");
    let (model, gate) = new_gate("cancel");
    let session = shell.create_session(&repo, "Cancelled");

    let idle = shell.call(
        "POST",
        "/api/session-work-cancel",
        &[("session_id", &session)],
    );
    assert_eq!(idle.0, 409, "an idle session has nothing to cancel");

    let turn = shell.start_turn(&repo, &session, &model);
    wait_until("the turn to reach the model", || {
        gate.started.load(Ordering::SeqCst) > 0
    });
    let (status, body, elapsed) = shell.call(
        "POST",
        "/api/session-work-cancel",
        &[("session_id", &session)],
    );
    assert_eq!(status, 200, "{body}");
    assert!(elapsed < PROMPT);

    let streamed = turn.join().unwrap();
    // The gate never opened, so the model never answered: the turn ended
    // because the cancel reached it.
    assert!(!streamed.contains("Done."), "{streamed}");
    assert_eq!(gate.started.load(Ordering::SeqCst), 1);
    wait_until("the lane to be released", || {
        lanes::working_label(&session).is_none()
    });
}

/// A resume route names only its paused turn; the turn's snapshot says whose
/// lane it takes.
#[test]
fn a_resume_request_is_classified_by_its_paused_turns_session() {
    let data_dir = super::tests::isolated_data_dir();
    let pending = data_dir.join("chat").join("pending");
    fs::create_dir_all(&pending).unwrap();
    let proposal = format!(
        "proposal_responsive_{}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );
    fs::write(
        pending.join(format!("{proposal}.json")),
        r#"{"session":{"id":"session_from_snapshot"}}"#,
    )
    .unwrap();
    let request = |body: String| Request {
        method: "POST".to_string(),
        path: "/api/resume-plan-stream".to_string(),
        query: HashMap::new(),
        headers: HashMap::new(),
        body,
    };
    let route = routes::Route::ResumePlanStream;
    assert_eq!(
        route_class(route, &request(format!("proposal_id={proposal}"))),
        routes::RouteClass::SessionWork(Some("session_from_snapshot".to_string()))
    );
    assert_eq!(
        route_class(
            route,
            &request(format!("proposal_id={proposal}&session_id=named"))
        ),
        routes::RouteClass::SessionWork(Some("named".to_string())),
        "a named session wins"
    );
    assert_eq!(
        route_class(route, &request("proposal_id=..%2Fescape".to_string())),
        routes::RouteClass::SessionWork(None),
        "an id that is not plain is never read from disk"
    );
    assert_eq!(
        route_class(
            routes::Route::StartWorker,
            &request("parent_session_id=parent".to_string())
        ),
        routes::RouteClass::SessionWork(Some("parent".to_string()))
    );
    assert_eq!(
        route_class(routes::Route::Sessions, &request(String::new())),
        routes::RouteClass::Ui
    );
}

/// The data lines of every `event` in an SSE transcript, parsed.
fn sse_payloads(streamed: &str, event: &str) -> Vec<serde_json::Value> {
    let marker = format!("event: {event}\n");
    streamed
        .split("\n\n")
        .filter_map(|block| block.trim_start_matches('\n').strip_prefix(marker.as_str()))
        .filter_map(|rest| rest.strip_prefix("data: "))
        .filter_map(|data| serde_json::from_str(data).ok())
        .collect()
}

/// Everything the data directory holds for `session_id`'s log.
fn session_log(session_id: &str) -> String {
    fn walk(dir: &Path, session_id: &str, out: &mut String) {
        let Ok(entries) = fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                walk(&path, session_id, out);
            } else if path.to_string_lossy().contains(session_id) {
                out.push_str(&fs::read_to_string(&path).unwrap_or_default());
            }
        }
    }
    let mut out = String::new();
    walk(super::tests::isolated_data_dir(), session_id, &mut out);
    out
}

fn ask_fields<'a>(repo: &'a str, session_id: &'a str, model: &'a str) -> Vec<(&'a str, &'a str)> {
    vec![
        ("repo", repo),
        ("session_id", session_id),
        ("prompt", "Read the README."),
        ("model", model),
    ]
}

/// Spec 110 D7: a message sent while the model works reaches its next request
/// as the user's own message, the log records it with `steering_note_added`,
/// the window hears `note`, and the turn goes on to its answer.
#[test]
fn a_note_during_a_turn_reaches_the_next_request_and_the_log() {
    let shell = Shell::start();
    let repo = temp_repo("note");
    let repo_arg = repo.to_str().unwrap().to_string();
    let (model, script) = new_script("note", "read_file", r#"{"path":"README.md"}"#, "Read it.");
    let session = shell.create_session(&repo, "Noted");

    let turn = shell.stream("/api/ask-stream", &ask_fields(&repo_arg, &session, &model));
    wait_until("the first round to reach the model", || {
        script.started() == 1
    });
    let (status, body) = shell.note(&session, "Also check the licence.");
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["status"], "queued");
    script.release();
    wait_until("the second round to reach the model", || {
        script.started() == 2
    });
    script.release();

    let streamed = turn.join().unwrap();
    assert!(streamed.contains("event: done"), "{streamed}");
    assert!(
        script.request_has_user_message(1, "Also check the licence."),
        "the next request carries the note as a user message"
    );
    assert!(!script.request_has_user_message(0, "Also check the licence."));
    let notes = sse_payloads(&streamed, "note");
    assert_eq!(notes.len(), 1, "{streamed}");
    assert_eq!(notes[0]["text"], "Also check the licence.");
    let message_id = notes[0]["messageId"].as_str().unwrap();
    assert!(sse_payloads(&streamed, "notes_returned").is_empty());

    let log = session_log(&session);
    assert!(log.contains("steering_note_added"), "{log}");
    assert!(log.contains(message_id));
    assert!(log.contains("Also check the licence."));
}

/// A note that arrives during the turn's final round has no boundary left to
/// reach; it comes back before `done`, so the window sends it as the next turn.
#[test]
fn a_note_in_the_final_round_is_returned_for_the_next_turn() {
    let shell = Shell::start();
    let repo = temp_repo("final");
    let (model, gate) = new_gate("final");
    let session = shell.create_session(&repo, "Final");

    let turn = shell.start_turn(&repo, &session, &model);
    wait_until("the turn to reach the model", || {
        gate.started.load(Ordering::SeqCst) > 0
    });
    assert_eq!(shell.note(&session, "One more thing.").0, 200);
    gate.open.store(true, Ordering::SeqCst);

    let streamed = turn.join().unwrap();
    let returned = sse_payloads(&streamed, "notes_returned");
    assert_eq!(returned.len(), 1, "{streamed}");
    assert_eq!(returned[0]["notes"], serde_json::json!(["One more thing."]));
    let done = sse_payloads(&streamed, "done");
    assert_eq!(done[0]["cancelled"], false, "{streamed}");
    assert!(
        streamed.find("event: notes_returned") < streamed.find("event: done"),
        "returned before done, so the window has them when the turn ends"
    );
}

/// A note before a Stop is returned, and the window puts it back in the
/// composer: the user asked the work to stop.
#[test]
fn a_note_before_a_stop_is_returned() {
    let shell = Shell::start();
    let repo = temp_repo("stopnote");
    let (model, gate) = new_gate("stopnote");
    let session = shell.create_session(&repo, "Stopped");

    let turn = shell.start_turn(&repo, &session, &model);
    wait_until("the turn to reach the model", || {
        gate.started.load(Ordering::SeqCst) > 0
    });
    assert_eq!(shell.note(&session, "Wait, not that.").0, 200);
    let (status, _, _) = shell.call(
        "POST",
        "/api/session-work-cancel",
        &[("session_id", &session)],
    );
    assert_eq!(status, 200);

    let streamed = turn.join().unwrap();
    let returned = sse_payloads(&streamed, "notes_returned");
    assert_eq!(returned.len(), 1, "{streamed}");
    assert_eq!(returned[0]["notes"], serde_json::json!(["Wait, not that."]));
    assert!(!session_log(&session).contains("Wait, not that."));
}

/// A note queued while the round that asks for an approval runs is kept with
/// the paused turn, and the resumed turn delivers it at its first boundary.
/// While paused the session has no running turn, so a note is refused: it can
/// never stand in for the approval.
#[test]
fn a_note_before_an_approval_reaches_the_resumed_turn() {
    let shell = Shell::start();
    let repo = temp_repo("approval");
    let repo_arg = repo.to_str().unwrap().to_string();
    let (model, script) = new_script(
        "approval",
        "run_command",
        r#"{"command":"touch approved.txt","reason":"Mark the tree"}"#,
        "Marked.",
    );
    let session = shell.create_session(&repo, "Approval");

    let turn = shell.stream("/api/ask-stream", &ask_fields(&repo_arg, &session, &model));
    wait_until("the first round to reach the model", || {
        script.started() == 1
    });
    assert_eq!(shell.note(&session, "Use the fixture.").0, 200);
    script.release();
    let streamed = turn.join().unwrap();
    assert!(
        sse_payloads(&streamed, "notes_returned").is_empty(),
        "a paused turn keeps its notes: {streamed}"
    );
    let done = sse_payloads(&streamed, "done").remove(0);
    let proposal_id = done["commandProposal"]["proposalId"]
        .as_str()
        .or_else(|| done["commandProposal"]["id"].as_str())
        .unwrap_or_else(|| panic!("the turn pauses on the command: {done}"))
        .to_string();
    wait_until("the lane to be released", || {
        lanes::working_label(&session).is_none()
    });

    let (status, body) = shell.note(&session, "Approve it.");
    assert_eq!(status, 409, "{body}");
    assert_eq!(body["code"], "not_working");
    assert!(
        PausedTurns::new(super::tests::isolated_data_dir())
            .session_id(&proposal_id)
            .is_some(),
        "the approval is still pending"
    );
    assert!(!repo.join("approved.txt").exists(), "nothing was approved");

    script.release();
    let resumed = shell.stream(
        "/api/resume-command-stream",
        &[
            ("repo", repo_arg.as_str()),
            ("proposal_id", proposal_id.as_str()),
            ("approved", "true"),
            ("model", model.as_str()),
        ],
    );
    let streamed = resumed.join().unwrap();
    assert!(streamed.contains("event: done"), "{streamed}");
    assert!(
        script.request_has_user_message(1, "Use the fixture."),
        "the resumed turn's next request carries the note"
    );
    assert!(!script.request_has_user_message(1, "Approve it."));
    assert_eq!(sse_payloads(&streamed, "note").len(), 1, "{streamed}");
    assert!(session_log(&session).contains("steering_note_added"));
}

/// A turn paused on a question holds no running turn: a note is refused and
/// the question stays unanswered.
#[test]
fn a_note_never_answers_a_pending_question() {
    let shell = Shell::start();
    let repo = temp_repo("question");
    let repo_arg = repo.to_str().unwrap().to_string();
    let (model, script) = new_script(
        "question",
        "ask_user",
        r#"{"question":"Which file?","options":["README","LICENSE"],"allowFreeText":true}"#,
        "Fine.",
    );
    let session = shell.create_session(&repo, "Question");

    script.release();
    let streamed = shell
        .stream("/api/ask-stream", &ask_fields(&repo_arg, &session, &model))
        .join()
        .unwrap();
    let done = sse_payloads(&streamed, "done").remove(0);
    let question_id = done["questionProposal"]["questionId"]
        .as_str()
        .unwrap_or_else(|| panic!("the turn pauses on a question: {done}"))
        .to_string();
    wait_until("the lane to be released", || {
        lanes::working_label(&session).is_none()
    });

    let (status, body) = shell.note(&session, "README");
    assert_eq!(status, 409, "{body}");
    assert_eq!(body["code"], "not_working");
    assert!(
        PausedTurns::new(super::tests::isolated_data_dir())
            .session_id(&question_id)
            .is_some(),
        "the question is still pending"
    );
    assert_eq!(script.started(), 1, "no model call followed the note");
}

/// An idle session, and one that does not exist, have no turn to take a note.
#[test]
fn a_note_to_an_idle_session_is_not_working() {
    let shell = Shell::start();
    let repo = temp_repo("idle");
    let session = shell.create_session(&repo, "Idle");
    for target in [session.as_str(), "session_that_does_not_exist"] {
        let (status, body) = shell.note(target, "Hello?");
        assert_eq!(status, 409, "{body}");
        assert_eq!(body["code"], "not_working");
        assert!(!body["error"].as_str().unwrap().is_empty());
    }
    let (status, _) = shell.note(&session, "   ");
    assert_eq!(status, 400, "an empty note is refused");
}
