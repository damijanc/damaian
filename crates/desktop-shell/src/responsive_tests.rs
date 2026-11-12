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
    let gate = gates().lock().unwrap().get(&config.model_name)?.clone();
    Some(Box::new(GatedAdapter { gate }))
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
