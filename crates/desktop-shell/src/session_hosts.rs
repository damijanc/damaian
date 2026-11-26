//! The shell side of spec 110's session hosts (D1, D1a, D6): one host process
//! per session, started on the session's first work request in the session's
//! own checkout and kept until the session is archived or deleted or the app
//! quits.
//!
//! The shell keeps every UI route. A session-work request is claimed on the
//! session's lane here, as before, then forwarded whole to the session's host,
//! whose response the shell copies to the window byte for byte. The lane is
//! marked remote, so its label, cancel and notes are asked of the host.
//!
//! A request that names no session yet (the first message of a new session)
//! goes to a host started for it under a placeholder id; the shell reads the
//! turn's `session` event as it copies the stream and binds the host to that
//! session then. A request whose session never appears gets a host for that
//! request only.
//!
//! Hosting is off unless a binary turns it on with [`enable_session_hosts`]:
//! the library's own tests run session work in the shell process, where their
//! scripted models live.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{Shutdown, TcpStream};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, PoisonError};
use std::time::{Duration, Instant};

use serde_json::{Map, Value};
use workspace_engine::{Config, PausedTurns, ProcessIdentity};

use crate::host::{HostKind, HostLaunch, StartedHost, exchange, host_audit_log, start_host};
use crate::http::{Request, parse_form, write_basic_response, write_sse_event};
use crate::lanes::{CancelRefusal, RemoteStatus, RemoteWork};
use crate::routes::Route;

/// How long a new host may take to serve its socket.
const READY_TIMEOUT: Duration = Duration::from_secs(30);
/// One short exchange with a host: status, cancel, note, shutdown.
const EXCHANGE_TIMEOUT: Duration = Duration::from_secs(2);
/// After `shutdown`, before `SIGTERM` (as a worker's, spec 57 §5.3).
const SHUTDOWN_TO_SIGTERM: Duration = Duration::from_secs(5);
/// How long the shell waits to see a host exit after its response broke off.
const EXIT_SETTLE: Duration = Duration::from_secs(2);
const POLL: Duration = Duration::from_millis(50);
/// How much of a stream's start is searched for the turn's `session` event.
const SNIFF_LIMIT: usize = 64 * 1024;

static EXE: OnceLock<PathBuf> = OnceLock::new();

/// Runs every session's work in a host process of its own, started from
/// `exe` with `--session-host`. Both `main`s call this with `current_exe()`.
pub fn enable_session_hosts(exe: PathBuf) {
    let _ = EXE.set(exe);
}

pub(crate) fn enabled() -> bool {
    EXE.get().is_some()
}

/// One host this shell started and has not seen exit.
///
/// No `Debug`: it holds the control token.
struct Host {
    started: StartedHost,
    /// The session the host works for; `None` while a new session's first
    /// turn has not named it yet.
    session: Option<String>,
}

#[derive(Clone)]
struct Endpoint {
    socket: PathBuf,
    token: String,
    identity: ProcessIdentity,
}

fn hosts() -> MutexGuard<'static, HashMap<String, Host>> {
    static HOSTS: OnceLock<Mutex<HashMap<String, Host>>> = OnceLock::new();
    HOSTS
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
}

fn endpoint_of(host: &Host) -> Endpoint {
    Endpoint {
        socket: host.started.socket.clone(),
        token: host.started.token.clone(),
        identity: host.started.identity,
    }
}

/// The pid of `session_id`'s host while it runs. For tests and diagnostics.
pub fn session_host_pid(session_id: &str) -> Option<u32> {
    let mut hosts = hosts();
    reap(&mut hosts);
    hosts
        .values()
        .find(|host| host.session.as_deref() == Some(session_id))
        .map(|host| host.started.identity.pid)
}

/// Drops every host that has exited, returning the sessions they served.
/// Dropping a host closes its lifeline and removes its registry entry.
fn reap(hosts: &mut HashMap<String, Host>) -> Vec<String> {
    let mut gone = Vec::new();
    hosts.retain(|_, host| {
        let alive = matches!(host.started.child.try_wait(), Ok(None));
        if !alive && let Some(session) = &host.session {
            gone.push(session.clone());
        }
        alive
    });
    gone
}

/// The live host under `key`, starting one when there is none.
fn ensure(key: &str, session: Option<&str>, checkout: Option<&Path>) -> Result<Endpoint, String> {
    let exe = EXE
        .get()
        .ok_or_else(|| "session hosts are not enabled".to_string())?;
    let endpoint = {
        let mut hosts = hosts();
        let gone = reap(&mut hosts);
        drop(hosts);
        for session in gone {
            host_died(&session);
        }
        let mut hosts = crate::session_hosts::hosts();
        if let Some(host) = hosts.get(key) {
            return Ok(endpoint_of(host));
        }
        // A host refuses a relative data directory, as a worker does.
        let data_dir =
            std::path::absolute(crate::effective_data_dir()?).map_err(|error| error.to_string())?;
        let started = start_host(HostLaunch {
            exe,
            kind: HostKind::Session,
            id: key,
            data_dir: &data_dir,
            current_dir: checkout,
            api_key: "",
        })?;
        let host = Host {
            started,
            session: session.map(str::to_string),
        };
        let endpoint = endpoint_of(&host);
        hosts.insert(key.to_string(), host);
        endpoint
    };
    wait_until_ready(key, &endpoint)?;
    Ok(endpoint)
}

fn wait_until_ready(key: &str, endpoint: &Endpoint) -> Result<(), String> {
    let deadline = Instant::now() + READY_TIMEOUT;
    loop {
        if UnixStream::connect(&endpoint.socket).is_ok() {
            return Ok(());
        }
        if ProcessIdentity::of(endpoint.identity.pid) != Some(endpoint.identity)
            || Instant::now() >= deadline
        {
            // Dropped, so the next request starts a fresh one.
            hosts().remove(key);
            return Err("The session's process did not start.".to_string());
        }
        std::thread::sleep(POLL);
    }
}

/// Writes each key line on the host's stdin, never on its socket (D1a).
fn hand_keys(key: &str, lines: &[String]) {
    if lines.is_empty() {
        return;
    }
    let mut hosts = hosts();
    let Some(stdin) = hosts
        .get_mut(key)
        .and_then(|host| host.started.stdin.as_mut())
    else {
        return;
    };
    for line in lines {
        if stdin
            .write_all(line.as_bytes())
            .and_then(|()| stdin.write_all(b"\n"))
            .and_then(|()| stdin.flush())
            .is_err()
        {
            return;
        }
    }
}

/// The keys `request` may use, resolved here and handed to the host: the
/// model key of the provider the request names, the fallback provider's when
/// the request accepts a fallback offer (spec 56), and the tokens of the MCP
/// servers configured for the repository. Environment references need no
/// handover: the host inherits the shell's environment. A key the shell cannot
/// resolve is not sent, and the host's turn fails on it as the shell's would.
fn keys_for(route: Route, form: &HashMap<String, String>) -> Vec<String> {
    // These change files or the log and never call a model, so they never
    // read a key; the shell does not read the Keychain for them either.
    if matches!(
        route,
        Route::ApplyPatch | Route::RollbackPatch | Route::RejectPatchFiles | Route::Rewind
    ) {
        return Vec::new();
    }
    let repo = form.get("repo").cloned().unwrap_or_default();
    let mut configs = Vec::new();
    if let Ok((config, _)) = crate::config_for_repo_with_model_options(&repo, form) {
        configs.push(config);
    }
    if route == Route::ResumeFallbackStream
        && form.get("approved").map(String::as_str) == Some("true")
        && let Some(proposal_id) = form.get("proposal_id")
        && let Ok(data_dir) = crate::effective_data_dir()
        && let Some(target) = PausedTurns::new(data_dir).provider_fallback_target(proposal_id)
        && let Ok((config, _)) = crate::config_for_repo_with_provider(&repo, Some(&target))
    {
        configs.push(config);
    }
    let mut lines = Vec::new();
    let mut add = |reference: &str, resolve: &dyn Fn(&str) -> Option<String>| {
        if let Some(account) = crate::keychain::account_from_reference(reference)
            && let Some(key) = resolve(reference)
        {
            let line = crate::session_host::key_line(account, &key);
            if !lines.contains(&line) {
                lines.push(line);
            }
        }
    };
    for config in &configs {
        add(&config.model_api_key_env, &|reference| {
            crate::resolve_model_api_key(reference).ok()
        });
        for server in &config.mcp_servers {
            add(&server.auth_token_env, &crate::resolve_mcp_token);
        }
    }
    lines
}

/// Asks every live host `command` and returns the answers that came; a host
/// that does not answer in time is left out. For the spec 33 Task 7 MCP
/// health exchange, which every session's host holds a part of.
pub(crate) fn ask_every_host(command: &str, arguments: Map<String, Value>) -> Vec<Value> {
    let endpoints = {
        let mut hosts = hosts();
        reap(&mut hosts);
        hosts.values().map(endpoint_of).collect::<Vec<_>>()
    };
    endpoints
        .iter()
        .filter_map(|endpoint| {
            exchange(
                &endpoint.socket,
                &endpoint.token,
                command,
                arguments.clone(),
                EXCHANGE_TIMEOUT,
            )
        })
        .collect()
}

/// A lane's way into its host (D3, D7).
struct RemoteHost {
    endpoint: Endpoint,
    session_id: String,
}

impl RemoteHost {
    fn ask(&self, command: &str, mut arguments: Map<String, Value>) -> Option<Value> {
        arguments.insert(
            "sessionId".to_string(),
            Value::from(self.session_id.clone()),
        );
        exchange(
            &self.endpoint.socket,
            &self.endpoint.token,
            command,
            arguments,
            EXCHANGE_TIMEOUT,
        )
    }
}

impl RemoteWork for RemoteHost {
    fn status(&self) -> Option<RemoteStatus> {
        let answer = self.ask("status", Map::new())?;
        answer["working"]
            .as_array()?
            .iter()
            .find(|row| row["sessionId"] == self.session_id.as_str())
            .map(|row| RemoteStatus {
                label: row["label"].as_str().unwrap_or_default().to_string(),
                cancellable: row["cancellable"] == true,
            })
    }

    fn cancel(&self) -> Result<(), CancelRefusal> {
        let answer = self.ask("cancel", Map::new()).ok_or(CancelRefusal::Idle)?;
        match (answer["ok"] == true, answer["refusal"].as_str()) {
            (true, _) => Ok(()),
            (false, Some("not_cancellable")) => Err(CancelRefusal::NotCancellable),
            _ => Err(CancelRefusal::Idle),
        }
    }

    fn note(&self, text: String) -> bool {
        let mut arguments = Map::new();
        arguments.insert("text".to_string(), Value::from(text));
        self.ask("note", arguments)
            .is_some_and(|answer| answer["ok"] == true)
    }
}

fn mark_lane_remote(endpoint: &Endpoint, session_id: &str) {
    let remote: Arc<dyn RemoteWork> = Arc::new(RemoteHost {
        endpoint: endpoint.clone(),
        session_id: session_id.to_string(),
    });
    crate::lanes::with_current(|lane| lane.set_remote(Arc::clone(&remote)));
}

/// A placeholder host id for work that names no session yet. Shaped like a
/// session id, so it names a socket and a registry entry the same way.
fn placeholder_id() -> String {
    let mut bytes = [0_u8; 8];
    getrandom::fill(&mut bytes).expect("secure random id generation failed");
    let hex: String = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
    format!("pending_{hex}")
}

/// Binds the host started under `key` to the session its turn created: the
/// table entry and the registry entry move to the session's id, so recovery's
/// live-host check (C12) finds it.
///
/// The registry entry is rewritten in place: it is named by the process's
/// identity, so registering the process again under the session's id failed
/// on it and left the placeholder (owner report 2026-11-26).
fn bind(key: &str, session_id: &str) {
    let mut hosts = hosts();
    let Some(mut host) = hosts.remove(key) else {
        return;
    };
    if let Err(error) = host.started.registration.reassign(session_id) {
        eprintln!("damaian: could not record session {session_id}'s host: {error}");
    }
    host.session = Some(session_id.to_string());
    hosts.insert(session_id.to_string(), host);
}

/// What the shell saw of one forwarded response while copying it.
#[derive(Default)]
struct Copied {
    bytes: usize,
    head: Vec<u8>,
    /// The last bytes seen, so an event split across two reads is still found.
    tail: Vec<u8>,
    ended_with_event: bool,
    session: Option<String>,
}

impl Copied {
    fn feed(&mut self, chunk: &[u8], sniff_session: bool) -> Option<String> {
        self.bytes += chunk.len();
        if self.head.len() < SNIFF_LIMIT {
            let room = SNIFF_LIMIT - self.head.len();
            self.head.extend_from_slice(&chunk[..chunk.len().min(room)]);
        }
        let mut window = std::mem::take(&mut self.tail);
        window.extend_from_slice(chunk);
        if contains(&window, b"\nevent: done\n") || contains(&window, b"\nevent: error\n") {
            self.ended_with_event = true;
        }
        let keep = window.len().saturating_sub(64);
        self.tail = window.split_off(keep);
        if sniff_session && self.session.is_none() {
            self.session = session_event(&self.head);
            return self.session.clone();
        }
        None
    }

    fn is_event_stream(&self) -> bool {
        contains(&self.head, b"text/event-stream")
    }

    /// Whether the response the host wrote is whole: a JSON response is one
    /// write, so any bytes are taken as whole; a stream ends with its
    /// terminal event.
    fn is_complete(&self) -> bool {
        self.bytes > 0 && (!self.is_event_stream() || self.ended_with_event)
    }
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack
        .windows(needle.len())
        .any(|window| window == needle)
}

/// The id in a stream's `session` event (`relay_turn_events`), once whole.
fn session_event(head: &[u8]) -> Option<String> {
    const MARK: &[u8] = b"event: session\ndata: ";
    let start = head.windows(MARK.len()).position(|window| window == MARK)? + MARK.len();
    let end = start + head[start..].iter().position(|byte| *byte == b'\n')?;
    let data: Value = serde_json::from_slice(&head[start..end]).ok()?;
    let id = data["sessionId"].as_str()?;
    crate::host::validate_session_id(id).ok()?;
    Some(id.to_string())
}

/// Forwards one session-work request to its session's host and copies the
/// response to `client`. The caller holds the session's lane when the request
/// names its session.
pub(crate) fn forward(
    client: &mut TcpStream,
    request: &Request,
    route: Route,
    session: Option<String>,
) -> Result<(), String> {
    let form = parse_form(&request.body);
    let checkout = form
        .get("repo")
        .map(PathBuf::from)
        .filter(|path| path.is_dir());
    let key = session.clone().unwrap_or_else(placeholder_id);
    let endpoint = ensure(&key, session.as_deref(), checkout.as_deref())?;
    hand_keys(&key, &keys_for(route, &form));
    if let Some(session_id) = &session {
        mark_lane_remote(&endpoint, session_id);
    }

    let mut host = UnixStream::connect(&endpoint.socket)
        .map_err(|_| "The session's process is not answering.".to_string())?;
    let mut line = serde_json::from_str::<Value>(&crate::session_host::request_line(request))
        .map_err(|error| error.to_string())?;
    line["token"] = Value::from(endpoint.token.clone());
    host.write_all(format!("{line}\n").as_bytes())
        .map_err(|_| "The session's process is not answering.".to_string())?;

    let mut copied = Copied::default();
    let mut bound = session.clone();
    let mut buffer = [0_u8; 16 * 1024];
    let mut client_gone = false;
    loop {
        let read = match host.read(&mut buffer) {
            Ok(0) | Err(_) => break,
            Ok(read) => read,
        };
        if let Some(session_id) = copied.feed(&buffer[..read], bound.is_none()) {
            // The new session's turn named it: claim its lane here too, so the
            // sidebar lists it and archive refuses it mid-turn.
            bind(&key, &session_id);
            crate::lanes::adopt_current(&session_id, route.work_label());
            mark_lane_remote(&endpoint, &session_id);
            bound = Some(session_id);
        }
        if client.write_all(&buffer[..read]).is_err() {
            // The window went away. Closing the host's connection is what its
            // stream notices, exactly as the shell's own relay notices a
            // departed window: the turn is cancelled at its next write.
            client_gone = true;
            let _ = host.shutdown(Shutdown::Both);
            break;
        }
    }
    drop(host);

    let host_key = bound.clone().unwrap_or(key);
    if !client_gone && !copied.is_complete() && host_exited(&host_key, &endpoint) {
        if let Some(session_id) = &bound {
            host_died(session_id);
        }
        let message = "This session's process stopped unexpectedly. Its work is offered for \
                       recovery when you reopen the app.";
        if copied.bytes == 0 {
            let _ = write_basic_response(
                client,
                500,
                "application/json",
                &crate::json::json_error(message),
            );
        } else if copied.is_event_stream() {
            let _ = write_sse_event(client, "error", &crate::json::json_error(message));
        }
    }
    if bound.is_none() {
        // A request that never named a session: its host served it alone.
        shutdown_host(&host_key);
    }
    Ok(())
}

/// Whether the host under `key` has exited, giving it a moment to finish
/// dying; an exited host is dropped from the table.
fn host_exited(key: &str, endpoint: &Endpoint) -> bool {
    let deadline = Instant::now() + EXIT_SETTLE;
    loop {
        let exited = {
            let mut hosts = hosts();
            match hosts.get_mut(key) {
                Some(host) => !matches!(host.started.child.try_wait(), Ok(None)),
                None => ProcessIdentity::of(endpoint.identity.pid) != Some(endpoint.identity),
            }
        };
        if exited {
            hosts().remove(key);
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(POLL);
    }
}

/// A host that died while the shell runs (D6): its children are swept like a
/// crashed instance's, and its session is classified as spec 17 classifies a
/// crash, so the work it was doing is offered for recovery.
fn host_died(session_id: &str) {
    if let Ok(config) = Config::load_for_repository(None) {
        let _ = crate::sweep_orphaned_processes(&config);
        let _ = host_audit_log(&config).record(
            "session_host_exited",
            &[
                ("actor", "system".to_string()),
                ("sessionId", session_id.to_string()),
            ],
        );
    }
    if let Err(error) = crate::recovery::recover_session(session_id) {
        eprintln!("damaian: recovery of session {session_id} after its process ended: {error}");
    }
}

/// Ends the host under `key`, if any: `shutdown`, the lifeline let go, and
/// `SIGTERM` by identity if it has not exited in time. Off the caller's
/// thread.
fn shutdown_host(key: &str) {
    let Some(mut host) = hosts().remove(key) else {
        return;
    };
    let _ = exchange(
        &host.started.socket,
        &host.started.token,
        "shutdown",
        Map::new(),
        EXCHANGE_TIMEOUT,
    );
    host.started.stdin = None;
    let spawned = std::thread::Builder::new()
        .name("damaian-host-shutdown".to_string())
        .spawn(move || {
            if !crate::host::wait_for_exit(&host.started.identity, POLL, SHUTDOWN_TO_SIGTERM) {
                host.started.identity.send_sigterm();
            }
            let _ = host.started.child.wait();
        });
    if spawned.is_err() {
        eprintln!("damaian: could not watch a session host shut down");
    }
}

/// Ends `session_id`'s host once the session is archived or deleted.
pub(crate) fn shutdown_session_host(session_id: &str) {
    let key = hosts()
        .iter()
        .find(|(_, host)| host.session.as_deref() == Some(session_id))
        .map(|(key, _)| key.clone());
    if let Some(key) = key {
        shutdown_host(&key);
    }
}

/// Quit (D6): asks every host to shut down, lets go of every lifeline, waits
/// up to five seconds, then `SIGTERM`s any that remain by identity. Each host
/// stops its work first, so a turn ends as stopped, not as a crash.
pub(crate) fn shutdown_all_session_hosts() {
    let endpoints = hosts().values().map(endpoint_of).collect::<Vec<_>>();
    for endpoint in &endpoints {
        let _ = exchange(
            &endpoint.socket,
            &endpoint.token,
            "shutdown",
            Map::new(),
            EXCHANGE_TIMEOUT,
        );
    }
    for host in hosts().values_mut() {
        host.started.stdin = None;
    }
    let deadline = Instant::now() + SHUTDOWN_TO_SIGTERM;
    loop {
        let mut hosts = hosts();
        reap(&mut hosts);
        if hosts.is_empty() || Instant::now() >= deadline {
            break;
        }
        drop(hosts);
        std::thread::sleep(POLL);
    }
    for endpoint in hosts().values().map(endpoint_of) {
        endpoint.identity.send_sigterm();
    }
    let deadline = Instant::now() + SHUTDOWN_TO_SIGTERM;
    loop {
        let mut hosts = hosts();
        reap(&mut hosts);
        if hosts.is_empty() || Instant::now() >= deadline {
            hosts.clear();
            break;
        }
        drop(hosts);
        std::thread::sleep(POLL);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_new_sessions_id_is_read_from_the_streams_session_event_even_when_split() {
        let stream = b"HTTP/1.1 200 OK\r\ncontent-type: text/event-stream; charset=utf-8\r\n\r\n\
                       event: session\ndata: {\"sessionId\":\"session_abc_1\"}\n\nevent: token\n";
        let mut copied = Copied::default();
        let (first, second) = stream.split_at(90);
        assert_eq!(copied.feed(first, true), None);
        assert_eq!(copied.feed(second, true).as_deref(), Some("session_abc_1"));
        assert!(copied.is_event_stream());
        assert!(!copied.is_complete(), "no terminal event yet");
        copied.feed(b"data: {}\n\nevent: do", false);
        copied.feed(b"ne\ndata: {}\n\n", false);
        assert!(copied.is_complete());
    }

    #[test]
    fn a_session_event_with_an_unsafe_id_is_not_taken() {
        assert_eq!(
            session_event(b"event: session\ndata: {\"sessionId\":\"../x\"}\n\n"),
            None
        );
        assert_eq!(session_event(b"event: session\ndata: {\"sessi"), None);
    }

    #[test]
    fn a_json_response_is_whole_once_any_of_it_arrived() {
        let mut copied = Copied::default();
        assert!(!copied.is_complete());
        copied.feed(
            b"HTTP/1.1 200 OK\r\ncontent-type: application/json\r\n\r\n{}",
            false,
        );
        assert!(copied.is_complete());
    }

    #[test]
    fn placeholder_ids_are_session_shaped_and_distinct() {
        let (one, two) = (placeholder_id(), placeholder_id());
        assert_ne!(one, two);
        crate::host::validate_session_id(&one).unwrap();
    }
}
