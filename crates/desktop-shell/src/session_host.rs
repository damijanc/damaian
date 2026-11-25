//! The `--session-host` process: one session's work in a process of its own
//! (spec 110 D1, D1a, D6).
//!
//! The shell starts this executable re-entered with [`SESSION_HOST_FLAG`]
//! through the shared host plumbing (`crate::host`) and forwards each of the
//! session's work requests to it whole. The host runs **the same route
//! handler** the shell would (`crate::serve_session_work`), so approvals and
//! resumes are the shell's own code, not a translation, and writes the HTTP
//! response, JSON or SSE, back over the connection for the shell to copy to
//! the window. Besides `request`, a connection may ask `status`, `cancel`,
//! `note` or `shutdown`, one line each: that is how the shell's lane reaches
//! the work running here (D3, D7).
//!
//! The host never reads the Keychain. The shell resolves each key a request
//! needs and writes it on the host's stdin, after the handover line, as
//! `{"account": …, "key": …}`; [`keys_from_shell_only`] makes every key
//! lookup in this process use those and nothing else.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, IsTerminal, Read, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use serde_json::{Map, Value, json};
use workspace_engine::{Config, ProcessRegistry};

use crate::host::{
    Handover, HostKind, ShutdownSignal, bind_host_socket, read_handover, refuse_terminal_stdin,
    tokens_match, validate_session_id, watch_lifeline,
};
use crate::http::Request;

/// The first argument that selects session-host mode, in both `main`s.
pub const SESSION_HOST_FLAG: &str = "--session-host";

/// The first line of a connection: a forwarded request carries its whole body,
/// so this is generous; it bounds what an authorized client can make the host
/// buffer.
const MAX_LINE_BYTES: u64 = 64 * 1024 * 1024;
/// How long a new connection may take to send its first line.
const FIRST_LINE_TIMEOUT: Duration = Duration::from_secs(5);
/// How long a host shutting down waits for its stopped work to end, so the
/// turn is recorded as stopped and is not taken for a crash (D6). The shell
/// waits five seconds before `SIGTERM`.
const STOP_GRACE: Duration = Duration::from_secs(3);

static KEYS_FROM_SHELL_ONLY: AtomicBool = AtomicBool::new(false);

/// True in a session host: key lookups use what the shell handed over on
/// stdin and never the Keychain (D1a, "keys only on stdin").
pub(crate) fn keys_from_shell_only() -> bool {
    KEYS_FROM_SHELL_ONLY.load(Ordering::Relaxed)
}

/// One key the shell hands a host on its stdin.
pub(crate) fn key_line(account: &str, key: &str) -> String {
    json!({"account": account, "key": key}).to_string()
}

/// Session-host entry point for both `main`s. `args` is the whole argv,
/// program name included. Returns the process exit code.
pub fn run_session_host_from_args(args: Vec<String>) -> i32 {
    match run_session_host(&args) {
        Ok(()) => 0,
        Err(error) => {
            eprintln!("damaian session host: {error}");
            1
        }
    }
}

/// The worker's order (spec 57 §5.3): nothing is served until the handover is
/// read and the lifeline and the signal handler are both in place.
fn run_session_host(args: &[String]) -> Result<(), String> {
    let (id, data_dir) = parse_args(args)?;
    refuse_terminal_stdin(std::io::stdin().is_terminal())?;
    // The handover's key is unused: a session's requests name their own
    // provider, so every key comes on a later line, as the request needs it.
    let Handover { api_key: _, token } = read_handover(&mut std::io::stdin().lock())?;
    KEYS_FROM_SHELL_ONLY.store(true, Ordering::Relaxed);
    // The routes this host runs load their config the way the shell does, so
    // it must arrive at the shell's data directory; anything else would read
    // and write another instance's state.
    let config = Config::load_for_repository(None).map_err(|error| error.to_string())?;
    if std::path::absolute(&config.data_dir).ok().as_deref() != Some(data_dir.as_path()) {
        return Err("the session host's data directory is not the shell's".to_string());
    }
    let (registry, audit) =
        ProcessRegistry::open_with_audit(&config).map_err(|error| error.to_string())?;

    let shutdown = Arc::new(ShutdownSignal::default());
    let lifeline = Arc::clone(&shutdown);
    std::thread::Builder::new()
        .name("damaian-host-lifeline".to_string())
        .spawn(move || {
            watch_lifeline(std::io::stdin().lock(), accept_key_line);
            lifeline.trigger();
        })
        .map_err(|error| format!("could not start the lifeline thread: {error}"))?;
    registry
        .clone()
        .install_shutdown_handler(audit.clone())
        .map_err(|error| error.to_string())?;

    let (listener, socket_path) = bind_host_socket(&data_dir, HostKind::Session, &id)?;
    let served = Arc::clone(&shutdown);
    std::thread::Builder::new()
        .name("damaian-host-accept".to_string())
        .spawn(move || {
            for stream in listener.incoming() {
                let Ok(stream) = stream else { continue };
                let (token, shutdown) = (token.clone(), Arc::clone(&served));
                if let Err(error) =
                    crate::spawn_with_shell_stack("damaian-host-connection", move || {
                        serve_host_connection(stream, &token, &shutdown);
                    })
                {
                    eprintln!("damaian session host: start a connection thread: {error}");
                }
            }
        })
        .map_err(|error| format!("could not start the accept thread: {error}"))?;

    shutdown.wait();
    // Stopped, not killed: each turn ends at its next safe point and records
    // itself as stopped, so quitting is never reported as a crash (D6).
    crate::lanes::cancel_all();
    let deadline = Instant::now() + STOP_GRACE;
    while !crate::lanes::working().is_empty() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(50));
    }
    let swept = registry.sweep_own(&audit);
    let _ = std::fs::remove_file(&socket_path);
    swept.map(|_| ()).map_err(|error| error.to_string())
}

fn parse_args(args: &[String]) -> Result<(String, PathBuf), String> {
    const USAGE: &str = "usage: --session-host --session <id> --data-dir <absolute dir>";
    let mut rest = args.iter().skip(1).map(String::as_str);
    if rest.next() != Some(SESSION_HOST_FLAG) {
        return Err(USAGE.to_string());
    }
    let (mut session_id, mut data_dir) = (None, None);
    while let Some(flag) = rest.next() {
        let slot = match flag {
            "--session" => &mut session_id,
            "--data-dir" => &mut data_dir,
            _ => return Err(USAGE.to_string()),
        };
        let value = rest.next().ok_or_else(|| USAGE.to_string())?;
        if slot.replace(value.to_string()).is_some() {
            return Err(USAGE.to_string());
        }
    }
    let (Some(session_id), Some(data_dir)) = (session_id, data_dir) else {
        return Err(USAGE.to_string());
    };
    validate_session_id(&session_id)?;
    let data_dir = PathBuf::from(data_dir);
    if !data_dir.is_absolute() {
        return Err(USAGE.to_string());
    }
    Ok((session_id, data_dir))
}

/// A key line from the shell. Anything else on stdin is ignored, and nothing
/// about a refused line is printed: it may hold a key.
fn accept_key_line(line: &[u8]) {
    let Ok(Value::Object(fields)) = serde_json::from_slice::<Value>(line) else {
        return;
    };
    if let (Some(Value::String(account)), Some(Value::String(key))) =
        (fields.get("account"), fields.get("key"))
    {
        crate::remember_model_api_key(account, key);
    }
}

/// The JSON line the shell sends for one forwarded request.
pub(crate) fn request_line(request: &Request) -> String {
    json!({
        "command": "request",
        "method": request.method,
        "path": request.path,
        "query": request.query,
        "headers": request.headers,
        "body": request.body,
    })
    .to_string()
}

fn request_from(fields: &mut Map<String, Value>) -> Option<Request> {
    let mut text = |name: &str| match fields.remove(name) {
        Some(Value::String(value)) => Some(value),
        _ => None,
    };
    let (method, path, body) = (text("method")?, text("path")?, text("body")?);
    let mut map = |name: &str| -> Option<HashMap<String, String>> {
        serde_json::from_value(fields.remove(name)?).ok()
    };
    Some(Request {
        method,
        path,
        query: map("query")?,
        headers: map("headers")?,
        body,
    })
}

/// One connection: a token-checked first line, then either one forwarded
/// request, whose response is written raw until the handler returns, or one
/// short command and its one-line answer.
fn serve_host_connection(stream: UnixStream, token: &str, shutdown: &ShutdownSignal) {
    if stream.set_read_timeout(Some(FIRST_LINE_TIMEOUT)).is_err() {
        return;
    }
    let Ok(reader) = stream.try_clone() else {
        return;
    };
    let mut line = Vec::new();
    match BufReader::new(reader)
        .take(MAX_LINE_BYTES + 1)
        .read_until(b'\n', &mut line)
    {
        Ok(_) if line.last() == Some(&b'\n') && line.len() as u64 <= MAX_LINE_BYTES => {}
        _ => return,
    }
    let Ok(Value::Object(mut fields)) = serde_json::from_slice::<Value>(&line) else {
        return;
    };
    match fields.remove("token") {
        Some(Value::String(offered)) if tokens_match(&offered, token) => {}
        // Unauthorized: closed unanswered.
        _ => return,
    }
    let command = match fields.remove("command") {
        Some(Value::String(command)) => command,
        _ => return,
    };
    let mut stream = stream;
    let session = match fields.remove("sessionId") {
        Some(Value::String(session)) => session,
        _ => String::new(),
    };
    let answer = match command.as_str() {
        "request" => {
            // A turn streams for as long as it runs.
            let _ = stream.set_read_timeout(None);
            match request_from(&mut fields) {
                Some(request) => crate::serve_forwarded_work(&mut stream, &request),
                None => {
                    let _ = crate::http::write_basic_response(
                        &mut stream,
                        400,
                        "application/json",
                        &crate::json::json_error("malformed forwarded request"),
                    );
                }
            }
            return;
        }
        "status" => {
            let working = crate::lanes::working()
                .into_iter()
                .map(|row| {
                    json!({
                        "sessionId": row.session_id,
                        "label": row.label,
                        "cancellable": row.cancellable,
                    })
                })
                .collect::<Vec<_>>();
            json!({"ok": true, "working": working})
        }
        "cancel" => match crate::lanes::cancel(&session) {
            Ok(()) => json!({"ok": true}),
            Err(crate::lanes::CancelRefusal::Idle) => json!({"ok": false, "refusal": "idle"}),
            Err(crate::lanes::CancelRefusal::NotCancellable) => {
                json!({"ok": false, "refusal": "not_cancellable"})
            }
        },
        "note" => {
            let text = match fields.remove("text") {
                Some(Value::String(text)) if text.len() <= crate::worker::MAX_NOTE_BYTES => text,
                _ => return,
            };
            json!({"ok": crate::lanes::queue_note(&session, text)})
        }
        "shutdown" => {
            shutdown.trigger();
            json!({"ok": true})
        }
        _ => json!({"ok": false, "error": "unknown command"}),
    };
    let _ = stream
        .write_all(format!("{answer}\n").as_bytes())
        .and_then(|()| stream.flush());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_forwarded_request_survives_its_line_whole() {
        let request = Request {
            method: "POST".to_string(),
            path: "/api/ask-stream".to_string(),
            query: HashMap::from([("a".to_string(), "b c".to_string())]),
            headers: HashMap::from([("origin".to_string(), "tauri://localhost".to_string())]),
            body: "repo=%2Fx&prompt=line%0Abreak".to_string(),
        };
        let line = request_line(&request);
        assert!(!line.contains('\n'), "one line on the socket");
        let Value::Object(mut fields) = serde_json::from_str(&line).unwrap() else {
            panic!("not an object");
        };
        assert_eq!(fields.remove("command"), Some(Value::from("request")));
        let back = request_from(&mut fields).unwrap();
        assert_eq!(back.method, request.method);
        assert_eq!(back.path, request.path);
        assert_eq!(back.query, request.query);
        assert_eq!(back.headers, request.headers);
        assert_eq!(back.body, request.body);
    }

    #[test]
    fn the_host_takes_only_well_formed_arguments() {
        let args = |list: &[&str]| {
            std::iter::once("damaian")
                .chain(list.iter().copied())
                .map(str::to_string)
                .collect::<Vec<_>>()
        };
        assert!(
            parse_args(&args(&[
                SESSION_HOST_FLAG,
                "--session",
                "session_1",
                "--data-dir",
                "/d"
            ]))
            .is_ok()
        );
        for bad in [
            &[SESSION_HOST_FLAG, "--session", "session_1"][..],
            &[SESSION_HOST_FLAG, "--session", "../x", "--data-dir", "/d"],
            &[
                SESSION_HOST_FLAG,
                "--session",
                "s",
                "--data-dir",
                "relative",
            ],
            &["--worker", "--session", "s", "--data-dir", "/d"],
        ] {
            assert!(parse_args(&args(bad)).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn a_key_line_reaches_the_key_cache_and_nothing_else_does() {
        let account = format!("host-key-test-{}", std::process::id());
        accept_key_line(b"not json");
        accept_key_line(br#"{"account": 7}"#);
        assert_eq!(crate::cached_model_api_key(&account), None);
        accept_key_line(key_line(&account, "k-123").as_bytes());
        assert_eq!(
            crate::cached_model_api_key(&account).as_deref(),
            Some("k-123")
        );
        crate::forget_model_api_key(&account);
    }
}
