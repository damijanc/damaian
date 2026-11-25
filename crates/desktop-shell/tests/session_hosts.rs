//! The real-process half of spec 110 Task 1b: the shell, served in this test
//! process, runs each session's work in a `--session-host` process of the real
//! `damaian-desktop-shell` binary, against a local OpenAI-compatible endpoint
//! this test serves and holds until it releases the turns.
//!
//! Every test here starts real processes and binds loopback ports, so each is
//! `#[ignore]`d, per `AGENTS.md`. Run them with:
//!
//! ```text
//! cargo nextest run -p desktop-shell --test session_hosts --run-ignored only
//! ```
//!
//! Nothing here kills by name: the one host this test kills is found by the
//! pid the shell reports for its session and checked against its start time.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use desktop_shell::ShellOptions;
use workspace_engine::{AuditLog, Config, ProcessIdentity, SecretScanner, SessionStore};

/// A model endpoint that answers "Done." to every request once released.
struct Model {
    port: u16,
    arrived: Arc<AtomicUsize>,
    open: Arc<AtomicBool>,
}

impl Model {
    fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let arrived = Arc::new(AtomicUsize::new(0));
        let open = Arc::new(AtomicBool::new(false));
        let (counter, gate) = (Arc::clone(&arrived), Arc::clone(&open));
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let (counter, gate) = (Arc::clone(&counter), Arc::clone(&gate));
                std::thread::spawn(move || answer(stream, &counter, &gate));
            }
        });
        Model {
            port,
            arrived,
            open,
        }
    }

    fn hold(&self) {
        self.open.store(false, Ordering::SeqCst);
    }

    fn release(&self) {
        self.open.store(true, Ordering::SeqCst);
    }
}

fn answer(stream: TcpStream, arrived: &AtomicUsize, open: &AtomicBool) {
    let mut reader = BufReader::new(stream.try_clone().unwrap());
    let mut writer = stream;
    let mut length = 0;
    let mut continue_expected = false;
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).unwrap_or(0) == 0 {
            return;
        }
        let lower = line.to_ascii_lowercase();
        if let Some(value) = lower.strip_prefix("content-length:") {
            length = value.trim().parse().unwrap_or(0);
        }
        if lower.starts_with("expect:") && lower.contains("100-continue") {
            continue_expected = true;
        }
        if line == "\r\n" {
            break;
        }
    }
    if continue_expected {
        let _ = writer.write_all(b"HTTP/1.1 100 Continue\r\n\r\n");
    }
    let mut body = vec![0_u8; length];
    let _ = reader.read_exact(&mut body);
    arrived.fetch_add(1, Ordering::SeqCst);
    while !open.load(Ordering::SeqCst) {
        std::thread::sleep(Duration::from_millis(20));
    }
    let (chunk, last) = if wants_tracker_call(&body) {
        let id = arrived.load(Ordering::SeqCst);
        (
            format!(
                r#"{{"id":"c1","object":"chat.completion.chunk","created":1,"model":"fake","choices":[{{"index":0,"delta":{{"role":"assistant","tool_calls":[{{"index":0,"id":"call_{id}","type":"function","function":{{"name":"mcp__tracker__ping","arguments":"{{}}"}}}}]}},"finish_reason":null}}]}}"#
            ),
            r#"{"id":"c1","object":"chat.completion.chunk","created":1,"model":"fake","choices":[{"index":0,"delta":{},"finish_reason":"tool_calls"}]}"#,
        )
    } else {
        (
            r#"{"id":"c1","object":"chat.completion.chunk","created":1,"model":"fake","choices":[{"index":0,"delta":{"role":"assistant","content":"Done."},"finish_reason":null}]}"#.to_string(),
            r#"{"id":"c1","object":"chat.completion.chunk","created":1,"model":"fake","choices":[{"index":0,"delta":{},"finish_reason":"stop"}]}"#,
        )
    };
    let body = format!("data: {chunk}\n\ndata: {last}\n\ndata: [DONE]\n\n");
    let _ = writer.write_all(
        format!(
            "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
            body.len()
        )
        .as_bytes(),
    );
}

/// The prompt that makes the model call `mcp__tracker__ping` once per turn
/// (spec 33 Task 7's host-level health test); every other prompt gets "Done.".
const TRACKER_PROMPT: &str = "PING-THE-TRACKER once.";

/// Whether this request is a tracker turn whose call has not been answered
/// yet: more user prompts asking for the call than tool results so far.
fn wants_tracker_call(body: &[u8]) -> bool {
    let Ok(request) = serde_json::from_slice::<serde_json::Value>(body) else {
        return false;
    };
    let messages = request["messages"].as_array().cloned().unwrap_or_default();
    let count = |role: &str, marked: bool| {
        messages
            .iter()
            .filter(|message| message["role"] == role)
            .filter(|message| {
                !marked
                    || message["content"]
                        .as_str()
                        .is_some_and(|text| text.contains(TRACKER_PROMPT))
            })
            .count()
    };
    count("user", true) > count("tool", false)
}

struct Shell {
    port: u16,
    token: String,
    data_dir: PathBuf,
}

impl Shell {
    fn request(&self, path: &str, fields: &[(&str, &str)]) -> TcpStream {
        let body = fields
            .iter()
            .map(|(key, value)| format!("{key}={}", encode(value)))
            .collect::<Vec<_>>()
            .join("&");
        let method = if matches!(
            path,
            "/api/recovery" | "/api/sessions" | "/api/working-sessions" | "/api/mcp-health"
        ) {
            "GET"
        } else {
            "POST"
        };
        let target = if method == "GET" {
            format!("{path}?{body}")
        } else {
            path.to_string()
        };
        let body = if method == "GET" { String::new() } else { body };
        let request = format!(
            "{method} {target} HTTP/1.1\r\nHost: 127.0.0.1\r\ncontent-type: application/x-www-form-urlencoded\r\nx-damaian-api-token: {}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
            self.token,
            body.len()
        );
        let mut stream = TcpStream::connect(("127.0.0.1", self.port)).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(120)))
            .unwrap();
        stream.write_all(request.as_bytes()).unwrap();
        stream
    }

    fn call(&self, path: &str, fields: &[(&str, &str)]) -> (u16, serde_json::Value, Duration) {
        let started = Instant::now();
        let mut response = String::new();
        self.request(path, fields)
            .read_to_string(&mut response)
            .unwrap();
        let status = response
            .split_whitespace()
            .nth(1)
            .and_then(|code| code.parse().ok())
            .unwrap_or(0);
        let body = response.split_once("\r\n\r\n").map_or("", |(_, body)| body);
        (
            status,
            serde_json::from_str(body).unwrap_or(serde_json::Value::Null),
            started.elapsed(),
        )
    }

    fn create_session(&self, repo: &Path, title: &str) -> String {
        let (status, body, _) = self.call(
            "/api/session-create",
            &[("repo", repo.to_str().unwrap()), ("title", title)],
        );
        assert_eq!(status, 200, "{body}");
        body["session"]["id"].as_str().unwrap().to_string()
    }

    fn start_turn(&self, repo: &Path, session_id: &str) -> std::thread::JoinHandle<String> {
        let stream = self.request(
            "/api/ask-stream",
            &[
                ("repo", repo.to_str().unwrap()),
                ("session_id", session_id),
                ("prompt", "Say done."),
            ],
        );
        std::thread::spawn(move || {
            let mut text = String::new();
            let _ = BufReader::new(stream).read_to_string(&mut text);
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

fn wait_until(what: &str, mut condition: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(60);
    while !condition() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// One shell and one model endpoint for the whole test binary: the data
/// directory is an environment variable every host inherits, and hosting is
/// switched on once per process.
fn shell_and_model() -> &'static (Shell, Model) {
    static SHARED: OnceLock<(Shell, Model)> = OnceLock::new();
    SHARED.get_or_init(|| {
        let model = Model::start();
        // Short: a host's socket path must fit in 104 bytes.
        let data_dir = std::env::temp_dir().join(format!("dsh-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&data_dir);
        std::fs::create_dir_all(data_dir.join("config")).unwrap();
        std::fs::write(
            data_dir.join("config").join("user.conf"),
            format!(
                "model_base_url=http://127.0.0.1:{}\nmodel_name=fake\nmodel_api_key_env=DAMAIAN_SESSION_HOST_TEST_KEY\n",
                model.port
            ),
        )
        .unwrap();
        // SAFETY: set once, before this process starts any thread that reads
        // the environment; every test in this binary goes through here first.
        unsafe {
            std::env::set_var("DAMAIAN_DATA_DIR", &data_dir);
            std::env::remove_var("DAMAIAN_MOCK_MODEL_RESPONSE");
        }
        desktop_shell::enable_session_hosts(PathBuf::from(env!(
            "CARGO_BIN_EXE_damaian-desktop-shell"
        )));
        let options = ShellOptions::new(0, None);
        let token = options.api_token.clone();
        let (sender, receiver) = std::sync::mpsc::channel();
        desktop_shell::spawn_with_shell_stack("test-shell", move || {
            desktop_shell::run_server_with_ready(options, |port| {
                sender.send(port).unwrap();
            })
        })
        .unwrap();
        let port = receiver.recv_timeout(Duration::from_secs(30)).unwrap();
        (
            Shell {
                port,
                token,
                data_dir,
            },
            model,
        )
    })
}

/// The tests share one shell and one model gate, so they take turns.
fn exclusive() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: Mutex<()> = Mutex::new(());
    LOCK.lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn temp_repo(name: &str) -> PathBuf {
    let repo = std::env::temp_dir().join(format!("dsh-repo-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&repo);
    std::fs::create_dir_all(&repo).unwrap();
    std::fs::write(repo.join("README.md"), "# Host\n").unwrap();
    std::fs::canonicalize(repo).unwrap()
}

fn live_pid(session_id: &str) -> u32 {
    let pid = desktop_shell::session_host_pid(session_id)
        .unwrap_or_else(|| panic!("no host for {session_id}"));
    assert_ne!(
        pid,
        std::process::id(),
        "the work must not run in the shell"
    );
    pid
}

fn alive(pid: u32) -> bool {
    ProcessIdentity::of(pid).is_some()
}

/// Two sessions in one project work at the same time, each in its own host
/// process; killing one host leaves the other and the shell running, and the
/// killed session's turn is offered for recovery as a crash (spec 17).
#[test]
#[ignore = "starts real host processes and binds loopback ports; see the module docs"]
fn two_sessions_work_at_once_in_two_hosts_and_a_killed_host_recovers_as_a_crash() {
    let _turn = exclusive();
    let (shell, model) = shell_and_model();
    // The launch sweep runs once per shell; a later crash joins its snapshot.
    let (status, _, _) = shell.call("/api/recovery", &[]);
    assert_eq!(status, 200);

    let repo = temp_repo("two");
    let first = shell.create_session(&repo, "First");
    let second = shell.create_session(&repo, "Second");

    model.hold();
    let before = model.arrived.load(Ordering::SeqCst);
    let one = shell.start_turn(&repo, &first);
    let two = shell.start_turn(&repo, &second);
    wait_until("both turns to reach the model", || {
        model.arrived.load(Ordering::SeqCst) >= before + 2
    });
    let (first_pid, second_pid) = (live_pid(&first), live_pid(&second));
    assert_ne!(first_pid, second_pid, "one host per session");

    // The shell still answers while both work, and lists both as working.
    let (status, working, elapsed) = shell.call("/api/working-sessions", &[]);
    assert_eq!(status, 200, "{working}");
    assert!(elapsed < Duration::from_secs(2), "{elapsed:?}");
    let listed = working.to_string();
    assert!(
        listed.contains(&first) && listed.contains(&second),
        "{listed}"
    );
    // A second turn for a working session is refused at once.
    let (status, refused, _) = shell.call(
        "/api/ask-stream",
        &[
            ("repo", repo.to_str().unwrap()),
            ("session_id", &first),
            ("prompt", "Again."),
        ],
    );
    assert_eq!(status, 409, "{refused}");

    model.release();
    let (one, two) = (one.join().unwrap(), two.join().unwrap());
    assert!(one.contains("event: done"), "{one}");
    assert!(two.contains("event: done"), "{two}");
    assert!(one.contains("Done."), "{one}");

    // Now kill one host mid-turn.
    model.hold();
    let before = model.arrived.load(Ordering::SeqCst);
    let one = shell.start_turn(&repo, &first);
    let two = shell.start_turn(&repo, &second);
    wait_until("both turns to reach the model again", || {
        model.arrived.load(Ordering::SeqCst) >= before + 2
    });
    let victim = ProcessIdentity::of(live_pid(&first)).unwrap();
    assert_eq!(
        victim.pid, first_pid,
        "the session's host is kept between turns"
    );
    // SAFETY: a plain kill(2) of the pid this test just confirmed is the
    // host the shell started for `first`.
    assert_eq!(unsafe { libc_kill(victim.pid as i32, 9) }, 0);
    let one = one.join().unwrap();
    assert!(
        one.contains("event: error") && one.contains("stopped unexpectedly"),
        "{one}"
    );
    assert!(alive(second_pid), "the other host is untouched");
    let (status, _, elapsed) = shell.call("/api/sessions", &[("repo", repo.to_str().unwrap())]);
    assert_eq!(status, 200);
    assert!(elapsed < Duration::from_secs(2), "{elapsed:?}");

    model.release();
    let two = two.join().unwrap();
    assert!(two.contains("event: done"), "{two}");

    let (status, recovery, _) = shell.call("/api/recovery", &[]);
    assert_eq!(status, 200, "{recovery}");
    let recovered = recovery.to_string();
    assert!(
        recovered.contains(&first),
        "the killed session's turn is offered for recovery: {recovered}"
    );
    assert!(
        !recovered.contains(&second),
        "the other session finished normally: {recovered}"
    );

    // A new request for the killed session starts a new host.
    let (status, _, _) = shell.call(
        "/api/compact",
        &[("repo", repo.to_str().unwrap()), ("session_id", &first)],
    );
    assert_ne!(status, 0);
    assert_ne!(live_pid(&first), first_pid);
}

/// Quitting ends every host, and a turn running at the time ends as stopped,
/// not as a crash (D6).
#[test]
#[ignore = "starts real host processes and binds loopback ports; see the module docs"]
fn quitting_ends_every_host_and_is_not_a_crash() {
    let _turn = exclusive();
    let (shell, model) = shell_and_model();
    let repo = temp_repo("quit");
    let session = shell.create_session(&repo, "Quit");

    model.hold();
    let before = model.arrived.load(Ordering::SeqCst);
    let turn = shell.start_turn(&repo, &session);
    wait_until("the turn to reach the model", || {
        model.arrived.load(Ordering::SeqCst) > before
    });
    let pid = live_pid(&session);

    desktop_shell::shutdown_all_hosts();
    assert!(!alive(pid), "the host ended with the quit");
    let streamed = turn.join().unwrap();
    model.release();
    assert!(
        streamed.contains("event: done"),
        "the turn ended as stopped: {streamed}"
    );

    let store = SessionStore::new(&shell.data_dir);
    let config = Config::load_for_repository(None).unwrap();
    let audit = AuditLog::with_retention(
        &config.data_dir,
        config.audit_enabled,
        config.audit_retention_days,
        SecretScanner::new(config.secret_patterns.clone()),
    );
    let crashed = workspace_engine::classify_session(&store, &audit, &session).unwrap();
    assert!(
        crashed.is_empty(),
        "a quit is not a crash: {crashed:?}; stream: {streamed}"
    );
}

/// The first message of a new session names no session: its host is started
/// for the request and bound to the session the turn creates, and the lane's
/// label, notes and cancel then reach the work in that host (D3, D7).
#[test]
#[ignore = "starts real host processes and binds loopback ports; see the module docs"]
fn a_new_sessions_first_turn_binds_its_host_and_the_lane_reaches_it() {
    let _turn = exclusive();
    let (shell, model) = shell_and_model();
    let repo = temp_repo("new");

    model.hold();
    let before = model.arrived.load(Ordering::SeqCst);
    let stream = shell.request(
        "/api/ask-stream",
        &[("repo", repo.to_str().unwrap()), ("prompt", "Say done.")],
    );
    let turn = std::thread::spawn(move || {
        let mut text = String::new();
        let _ = BufReader::new(stream).read_to_string(&mut text);
        text
    });
    wait_until("the turn to reach the model", || {
        model.arrived.load(Ordering::SeqCst) > before
    });
    let mut session = String::new();
    wait_until("the new session to be listed as working", || {
        let (_, working, _) = shell.call("/api/working-sessions", &[]);
        let rows = working["sessions"].as_array().cloned().unwrap_or_default();
        session = rows
            .iter()
            .filter_map(|row| row["sessionId"].as_str())
            .find(|id| desktop_shell::session_host_pid(id).is_some())
            .unwrap_or_default()
            .to_string();
        !session.is_empty()
    });
    let pid = live_pid(&session);

    let (status, noted, _) = shell.call(
        "/api/session-note",
        &[
            ("session_id", &session),
            ("text", "Also mention the README."),
        ],
    );
    assert_eq!(
        status, 200,
        "a note reaches the host's running turn: {noted}"
    );
    let (status, cancelled, _) =
        shell.call("/api/session-work-cancel", &[("session_id", &session)]);
    assert_eq!(status, 200, "{cancelled}");
    let streamed = turn.join().unwrap();
    model.release();
    assert!(streamed.contains("event: session"), "{streamed}");
    assert!(streamed.contains("event: done"), "{streamed}");
    assert_eq!(
        desktop_shell::session_host_pid(&session),
        Some(pid),
        "the host stays bound to its session"
    );
}

/// Spec 33 Task 7: MCP health lives in the process whose turn contacted the
/// server — here, the session's host — so `/api/mcp-health` must gather it
/// from there; a server that fails repeatedly in that host is `Failed`, and
/// the manual retry on `/api/mcp-test` reaches the host, clears the latch
/// there, and lets the session's next turn contact the server again.
#[test]
#[ignore = "starts real host processes and binds loopback ports; see the module docs"]
fn mcp_health_is_gathered_from_the_session_host_and_a_retry_clears_its_latch() {
    let _turn = exclusive();
    let (shell, model) = shell_and_model();
    model.release();
    let repo = temp_repo("mcp");
    let script = repo.join("tracker.sh");
    std::fs::write(&script, "#!/bin/sh\nexit 1\n").unwrap();
    let user_conf = shell.data_dir.join("config").join("user.conf");
    let shared = std::fs::read_to_string(&user_conf).unwrap();
    std::fs::write(
        &user_conf,
        format!(
            "{shared}mcp_enabled=true\n\
             mcp_server.tracker.enabled=true\n\
             mcp_server.tracker.transport=stdio\n\
             mcp_server.tracker.command=/bin/sh\n\
             mcp_server.tracker.args={}\n\
             mcp_server.tracker.require_approval=false\n",
            script.display()
        ),
    )
    .unwrap();
    let repo_arg = repo.to_str().unwrap();
    let session = shell.create_session(&repo, "MCP health");
    let turn = |prompt: &str| {
        let mut text = String::new();
        shell
            .request(
                "/api/ask-stream",
                &[
                    ("repo", repo_arg),
                    ("session_id", &session),
                    ("prompt", prompt),
                ],
            )
            .read_to_string(&mut text)
            .unwrap();
        assert!(text.contains("event: done"), "{text}");
        text
    };
    let tracker = |shell: &Shell| {
        let (status, body, _) = shell.call("/api/mcp-health", &[("repo", repo_arg)]);
        assert_eq!(status, 200, "{body}");
        body["servers"]
            .as_array()
            .unwrap()
            .iter()
            .find(|server| server["id"] == "tracker")
            .cloned()
            .unwrap_or_else(|| panic!("tracker is listed: {body}"))
    };

    assert_eq!(tracker(shell)["health"], "unknown");
    // Two contacts fail in the first turn (the listing, then the call), the
    // third in the second turn, which latches `Failed` — all in the host.
    turn(TRACKER_PROMPT);
    live_pid(&session);
    let seen = tracker(shell);
    assert_eq!(seen["health"], "unreachable", "{seen}");
    assert_eq!(seen["sessionId"], session.as_str(), "observed by the host");
    turn(TRACKER_PROMPT);
    let seen = tracker(shell);
    assert_eq!(seen["health"], "failed", "{seen}");
    assert!(seen["reason"].as_str().is_some(), "{seen}");

    // The server is fixed; the retry clears the host's latch and contacts it.
    std::fs::write(
        &script,
        r#"#!/bin/sh
while IFS= read -r line; do
  id=$(printf '%s' "$line" | sed 's/.*"id":\([0-9]*\).*/\1/')
  case "$line" in
    *'"method":"initialize"'*)
      printf '{"jsonrpc":"2.0","id":%s,"result":{"protocolVersion":"2025-06-18","capabilities":{},"serverInfo":{"name":"fake","version":"0"}}}\n' "$id" ;;
    *'"method":"tools/list"'*)
      printf '{"jsonrpc":"2.0","id":%s,"result":{"tools":[{"name":"ping","description":"Ping.","inputSchema":{"type":"object"}}]}}\n' "$id" ;;
    *'"method":"tools/call"'*)
      printf '{"jsonrpc":"2.0","id":%s,"result":{"content":[{"type":"text","text":"pong"}]}}\n' "$id" ;;
  esac
done
"#,
    )
    .unwrap();
    let (status, retried, _) = shell.call(
        "/api/mcp-test",
        &[("retry", "true"), ("repo", repo_arg), ("id", "tracker")],
    );
    assert_eq!(status, 200, "{retried}");
    assert_eq!(retried["ok"], true, "{retried}");
    assert_eq!(retried["health"], "ready", "{retried}");
    assert_eq!(
        retried["wasFailedIn"], 1,
        "the host had it latched: {retried}"
    );

    // The session's next turn contacts the server again, from its host.
    turn(TRACKER_PROMPT);
    let seen = tracker(shell);
    assert_eq!(seen["health"], "ready", "{seen}");
    assert_eq!(seen["sessionId"], session.as_str(), "{seen}");

    std::fs::write(&user_conf, shared).unwrap();
}

unsafe extern "C" {
    #[link_name = "kill"]
    fn libc_kill(pid: i32, signal: i32) -> i32;
}
