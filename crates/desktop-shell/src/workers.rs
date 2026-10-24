//! The shell-side supervisor for spec 57's workers: spawn, lifetime, control
//! and listing, per `docs/specs/57_worker_sessions/proposal.md` §5.1 steps
//! 7–9, §5.3 and §5.7.
//!
//! The shell spawns each worker as its own executable re-entered with
//! `--worker` (`crate::worker`), registers it with #46 as
//! [`ProcessKind::Worker`] owned by the shell, writes the one handover line and
//! then holds the worker's stdin open for its whole life: dropping it is the
//! lifeline signal (§5.3 step 1). Every exchange with a worker is one short
//! request on its control socket; the shell never holds a socket open.
//!
//! The API key goes onto the worker's stdin and nowhere else. Nothing here puts
//! it in the environment, argv, a file, a log line, an audit field or an error.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::net::UnixStream;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::{Mutex, MutexGuard, OnceLock, PoisonError};
use std::time::{Duration, Instant};

use serde_json::{Map, Value, json};
use workspace_engine::worker_progress::read_progress_after;
use workspace_engine::worktree::{
    RemoveOutcome, create_worker_worktree, remove_worker_worktree, source_changes,
};
use workspace_engine::{
    AuditLog, Config, ProcessIdentity, ProcessKind, ProcessRegistry, RegistrationHandle,
    SecretScanner, Session, SessionMode, SessionStore, TaskStatus, WorkerRecord,
};

/// How long the supervisor waits at each step. Only tests change these.
#[derive(Debug, Clone, Copy)]
pub struct WorkerTimeouts {
    /// For a new worker's socket to answer before its first message is sent.
    pub ready: Duration,
    /// One request and its answer on a worker's socket.
    pub exchange: Duration,
    /// For a stopped turn to end before `shutdown` is sent (§5.3: 10 s).
    pub stop_to_shutdown: Duration,
    /// For the process to exit after `shutdown` before `SIGTERM` (§5.3: 5 s).
    pub shutdown_to_sigterm: Duration,
    /// Between checks while waiting.
    pub poll: Duration,
}

impl Default for WorkerTimeouts {
    fn default() -> Self {
        Self {
            ready: Duration::from_secs(30),
            exchange: Duration::from_secs(2),
            stop_to_shutdown: Duration::from_secs(10),
            shutdown_to_sigterm: Duration::from_secs(5),
            poll: Duration::from_millis(100),
        }
    }
}

/// What `POST /api/workers` asks for.
#[derive(Debug, Clone)]
pub struct SpawnRequest {
    pub repo: PathBuf,
    pub prompt: String,
    pub include_uncommitted: bool,
    pub parent_session_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpawnedWorker {
    pub session_id: String,
    pub title: String,
    pub branch: String,
    pub worktree: PathBuf,
}

/// The longest answer read from a worker. `status` is a few hundred bytes; the
/// bound only stops a misbehaving worker from making the shell buffer forever.
const MAX_ANSWER_BYTES: u64 = 1024 * 1024;
/// A worker session's title: the prompt's first non-blank line, this long.
const MAX_TITLE_CHARS: usize = 80;

/// One worker this shell started and has not yet seen exit.
///
/// No `Debug`: it holds the control token.
struct Supervised {
    child: Child,
    /// The lifeline. `None` once the shell has let go of it.
    stdin: Option<ChildStdin>,
    _registration: RegistrationHandle,
    identity: ProcessIdentity,
    token: String,
    socket: PathBuf,
    worktree: PathBuf,
    audit: AuditLog,
}

/// What a socket exchange or a signal needs, copied out so that no lock is
/// held while talking to a worker.
#[derive(Clone)]
struct Endpoint {
    session_id: String,
    socket: PathBuf,
    token: String,
    identity: ProcessIdentity,
    audit: AuditLog,
}

/// The workers this shell supervises, keyed by session id.
pub struct Supervisor {
    exe: PathBuf,
    timeouts: WorkerTimeouts,
    workers: Mutex<HashMap<String, Supervised>>,
}

impl Supervisor {
    /// `exe` is what is started with `--worker`: `current_exe()` in the app.
    pub fn new(exe: PathBuf, timeouts: WorkerTimeouts) -> Self {
        Self {
            exe,
            timeouts,
            workers: Mutex::new(HashMap::new()),
        }
    }

    /// §5.1 steps 1–9, in order. Steps 1–6 are `create_worker_worktree`,
    /// which refuses before changing anything; the session, the records and
    /// the audit event come next, and the process last, so a worker never runs
    /// without the record that names its worktree.
    pub fn spawn(
        &self,
        config: &Config,
        request: SpawnRequest,
        api_key: &str,
    ) -> Result<SpawnedWorker, String> {
        if request.prompt.trim().is_empty() {
            return Err("A worker needs a prompt.".to_string());
        }
        let store = SessionStore::new(&config.data_dir);
        if let Some(parent) = &request.parent_session_id {
            if store
                .read_session(parent)
                .map_err(|error| error.to_string())?
                .is_none()
            {
                return Err(format!("Unknown session: {parent}"));
            }
            // §5.1: workers do not spawn workers.
            if store
                .worker_record(parent)
                .map_err(|error| error.to_string())?
                .is_some()
            {
                return Err("A worker cannot start workers.".to_string());
            }
        }
        // The socket path's length depends only on the data directory, so a
        // directory too long for `sun_path` is refused here, before the
        // worktree, its branch and the records exist rather than after.
        let data_dir = std::path::absolute(&config.data_dir).map_err(|error| error.to_string())?;
        crate::worker_socket_path(&data_dir, "socket-length-check")?;
        let audit = audit_log(config);
        let created = match create_worker_worktree(
            &request.repo,
            &request.prompt,
            request.include_uncommitted,
        ) {
            Ok(created) => created,
            Err(error) => {
                // A failure after `worktree add` leaves the worktree and its
                // branch behind (Task 4), so the attempt is on record even
                // though the commands it ran are not returned.
                let _ = audit.record(
                    "worker_spawn_failed",
                    &[
                        ("actor", "desktop_user".to_string()),
                        ("sourceRepository", request.repo.display().to_string()),
                        ("error", error.to_string()),
                    ],
                );
                return Err(error.to_string());
            }
        };
        let source = std::fs::canonicalize(&request.repo).map_err(|error| error.to_string())?;

        // 7.
        let title = worker_title(&request.prompt);
        let session = store
            .create_session(
                &workspace_engine::hash::repository_id_for_root(&created.path),
                &title,
            )
            .map_err(|error| error.to_string())?;
        store
            .set_session_mode(&session.id, SessionMode::Code, "user")
            .map_err(|error| error.to_string())?;
        let record = WorkerRecord {
            parent_session_id: request.parent_session_id.clone(),
            source_repository: source,
            worktree_path: created.path.clone(),
            branch: created.branch.clone(),
            base_commit: created.base_commit.clone(),
            included_uncommitted: request.include_uncommitted,
            git_commands: created.git_commands.clone(),
        };
        store
            .record_worker_spawned(&session.id, &record)
            .map_err(|error| error.to_string())?;
        if let Some(parent) = &record.parent_session_id {
            store
                .record_worker_child(parent, &session.id)
                .map_err(|error| error.to_string())?;
        }

        // 8.
        audit
            .record(
                "worker_spawned",
                &[
                    ("actor", "desktop_user".to_string()),
                    ("sessionId", session.id.clone()),
                    (
                        "parentSessionId",
                        record.parent_session_id.clone().unwrap_or_default(),
                    ),
                    (
                        "sourceRepository",
                        record.source_repository.display().to_string(),
                    ),
                    ("worktreePath", record.worktree_path.display().to_string()),
                    ("branch", record.branch.clone()),
                    ("baseCommit", record.base_commit.clone()),
                    (
                        "includedUncommitted",
                        record.included_uncommitted.to_string(),
                    ),
                    (
                        "gitCommands",
                        serde_json::to_string(&record.git_commands)
                            .map_err(|error| error.to_string())?,
                    ),
                ],
            )
            .map_err(|error| error.to_string())?;

        // 9.
        self.start(
            config,
            &session.id,
            &record.worktree_path,
            api_key,
            Some(request.prompt),
        )?;
        Ok(SpawnedWorker {
            session_id: session.id,
            title,
            branch: record.branch,
            worktree: record.worktree_path,
        })
    }

    /// *Restart worker* (§5.7): a new process for the session's existing
    /// worktree. Refused while a live worker holds that worktree (§5.1, one
    /// worker per worktree).
    pub fn restart(&self, config: &Config, session_id: &str, api_key: &str) -> Result<(), String> {
        let record = SessionStore::new(&config.data_dir)
            .worker_record(session_id)
            .map_err(|error| error.to_string())?
            .ok_or_else(|| "This session is not a worker.".to_string())?;
        if self.worktree_is_held(config, session_id, &record.worktree_path) {
            return Err(WORKTREE_HELD.to_string());
        }
        if !record.worktree_path.is_dir() {
            return Err("This worker's worktree was removed.".to_string());
        }
        self.start(config, session_id, &record.worktree_path, api_key, None)?;
        audit_log(config)
            .record(
                "worker_restarted",
                &[
                    ("actor", "desktop_user".to_string()),
                    ("sessionId", session_id.to_string()),
                    ("worktreePath", record.worktree_path.display().to_string()),
                ],
            )
            .map_err(|error| error.to_string())?;
        Ok(())
    }

    /// One request on the worker's socket. `arguments` are the command's own
    /// fields in the socket's names (`text`, `note`, `proposalId`).
    pub fn control(
        &self,
        session_id: &str,
        command: &str,
        arguments: Map<String, Value>,
    ) -> Result<Value, String> {
        let endpoint = self
            .endpoint(session_id)
            .ok_or_else(|| NOT_RUNNING.to_string())?;
        exchange(
            &endpoint.socket,
            &endpoint.token,
            command,
            arguments,
            self.timeouts.exchange,
        )
        .ok_or_else(|| NO_ANSWER.to_string())
    }

    /// §5.3's stop escalation. Answers after the first exchanges, and waits
    /// for the turn, `shutdown` and `SIGTERM` on a thread of its own, so the
    /// shell's single request thread is never held for the 10 + 5 seconds.
    pub fn stop(&self, session_id: &str) -> Result<Value, String> {
        let endpoint = self
            .endpoint(session_id)
            .ok_or_else(|| NOT_RUNNING.to_string())?;
        let mut target = SocketTarget {
            endpoint,
            exchange: self.timeouts.exchange,
        };
        let (answer, next) = begin_stop(&mut target);
        if next != StopNext::Done {
            let timeouts = self.timeouts;
            std::thread::Builder::new()
                .name("damaian-worker-stop".to_string())
                .spawn(move || {
                    finish_stop(&mut target, next, &timeouts);
                })
                .map_err(|error| format!("could not start the stop escalation: {error}"))?;
        }
        Ok(answer)
    }

    /// Whether this shell holds a worker for `session_id` that has not exited.
    pub fn is_alive(&self, session_id: &str) -> bool {
        let mut workers = self.lock();
        reap(&mut workers);
        workers.contains_key(session_id)
    }

    /// The worker's pid, while it is alive.
    pub fn pid(&self, session_id: &str) -> Option<u32> {
        let mut workers = self.lock();
        reap(&mut workers);
        workers.get(session_id).map(|worker| worker.identity.pid)
    }

    /// Sends `shutdown`, lets go of the lifeline, and `SIGTERM`s the worker by
    /// identity if it has not exited within `shutdown_to_sigterm`. Returns
    /// false when this shell holds no live worker for the session.
    pub fn shutdown(&self, session_id: &str) -> bool {
        let Some(endpoint) = self.endpoint(session_id) else {
            return false;
        };
        let _ = exchange(
            &endpoint.socket,
            &endpoint.token,
            "shutdown",
            Map::new(),
            self.timeouts.exchange,
        );
        if let Some(worker) = self.lock().get_mut(session_id) {
            worker.stdin = None;
        }
        let timeouts = self.timeouts;
        let watchdog = std::thread::Builder::new()
            .name("damaian-worker-shutdown".to_string())
            .spawn(move || {
                if !wait_for_exit(&endpoint.identity, &timeouts, timeouts.shutdown_to_sigterm) {
                    terminate(&endpoint);
                }
            });
        if watchdog.is_err() {
            eprintln!("damaian: could not watch worker {session_id} shut down");
        }
        true
    }

    /// Asks every worker to shut down, lets go of every lifeline, waits up to
    /// `shutdown_to_sigterm`, then `SIGTERM`s any that remain by registry
    /// identity (§5.3 step 4).
    pub fn shutdown_all(&self) {
        let endpoints = self.endpoints();
        for endpoint in &endpoints {
            let _ = exchange(
                &endpoint.socket,
                &endpoint.token,
                "shutdown",
                Map::new(),
                self.timeouts.exchange,
            );
        }
        for worker in self.lock().values_mut() {
            worker.stdin = None;
        }
        let deadline = Instant::now() + self.timeouts.shutdown_to_sigterm;
        loop {
            let mut workers = self.lock();
            reap(&mut workers);
            if workers.is_empty() || Instant::now() >= deadline {
                break;
            }
            drop(workers);
            std::thread::sleep(self.timeouts.poll);
        }
        for endpoint in self.endpoints() {
            terminate(&endpoint);
        }
        // A signalled worker sweeps its own children before it exits; give it
        // the same grace again, then let go of whatever is left.
        let deadline = Instant::now() + self.timeouts.shutdown_to_sigterm;
        loop {
            let mut workers = self.lock();
            reap(&mut workers);
            if workers.is_empty() || Instant::now() >= deadline {
                workers.clear();
                break;
            }
            drop(workers);
            std::thread::sleep(self.timeouts.poll);
        }
    }

    /// Starts `exe --worker`, registers it, hands over and keeps the lifeline.
    fn start(
        &self,
        config: &Config,
        session_id: &str,
        worktree: &Path,
        api_key: &str,
        first_message: Option<String>,
    ) -> Result<(), String> {
        // The worker refuses a relative data directory; the shell's may be
        // relative when `DAMAIAN_DATA_DIR` was set that way.
        let data_dir = std::path::absolute(&config.data_dir).map_err(|error| error.to_string())?;
        let socket = crate::worker_socket_path(&data_dir, session_id)?;
        let token = control_token()?;
        let registry = ProcessRegistry::open(&data_dir).map_err(|error| error.to_string())?;
        let mut child = Command::new(&self.exe)
            .arg(crate::WORKER_FLAG)
            .arg("--session")
            .arg(session_id)
            .arg("--data-dir")
            .arg(&data_dir)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::inherit())
            .process_group(0)
            .spawn()
            .map_err(|error| format!("could not start the worker: {error}"))?;
        let Some(mut stdin) = child.stdin.take() else {
            abandon(child);
            return Err("could not open the worker's stdin".to_string());
        };
        let registration = match registry.register(ProcessKind::Worker, session_id, child.id()) {
            Ok(registration) => registration,
            Err(error) => {
                drop(stdin);
                abandon(child);
                return Err(format!("could not register the worker: {error}"));
            }
        };
        let Some(identity) = ProcessIdentity::of(child.id()) else {
            drop(stdin);
            abandon(child);
            return Err("the worker exited as it started".to_string());
        };
        let handover = json!({"apiKey": api_key, "token": token}).to_string();
        if stdin
            .write_all(handover.as_bytes())
            .and_then(|()| stdin.write_all(b"\n"))
            .and_then(|()| stdin.flush())
            .is_err()
        {
            drop(stdin);
            abandon(child);
            return Err("could not hand over to the worker: it exited".to_string());
        }
        drop(handover);

        let audit = audit_log(config);
        if let Some(text) = first_message {
            let endpoint = Endpoint {
                session_id: session_id.to_string(),
                socket: socket.clone(),
                token: token.clone(),
                identity,
                audit: audit.clone(),
            };
            let timeouts = self.timeouts;
            std::thread::Builder::new()
                .name("damaian-worker-first-message".to_string())
                .spawn(move || deliver_first_message(&endpoint, text, &timeouts))
                .map_err(|error| format!("could not start the worker's first message: {error}"))?;
        }
        self.lock().insert(
            session_id.to_string(),
            Supervised {
                child,
                stdin: Some(stdin),
                _registration: registration,
                identity,
                token,
                socket,
                worktree: worktree.to_path_buf(),
                audit,
            },
        );
        Ok(())
    }

    fn lock(&self) -> MutexGuard<'_, HashMap<String, Supervised>> {
        self.workers.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn endpoint(&self, session_id: &str) -> Option<Endpoint> {
        let mut workers = self.lock();
        reap(&mut workers);
        workers
            .get(session_id)
            .map(|worker| endpoint_of(session_id, worker))
    }

    fn endpoints(&self) -> Vec<Endpoint> {
        let mut workers = self.lock();
        reap(&mut workers);
        workers
            .iter()
            .map(|(session_id, worker)| endpoint_of(session_id, worker))
            .collect()
    }

    /// One worker per worktree: one this shell holds, or another process
    /// answering on the session's socket.
    fn worktree_is_held(&self, config: &Config, session_id: &str, worktree: &Path) -> bool {
        let held_here = {
            let mut workers = self.lock();
            reap(&mut workers);
            workers.contains_key(session_id)
                || workers.values().any(|worker| worker.worktree == worktree)
        };
        held_here
            || crate::worker_socket_path(&config.data_dir, session_id)
                .is_ok_and(|socket| UnixStream::connect(socket).is_ok())
    }
}

const NOT_RUNNING: &str = "This worker is not running.";
const NO_ANSWER: &str = "The worker did not answer.";
const WORKTREE_HELD: &str = "A live worker already holds this worktree.";

fn endpoint_of(session_id: &str, worker: &Supervised) -> Endpoint {
    Endpoint {
        session_id: session_id.to_string(),
        socket: worker.socket.clone(),
        token: worker.token.clone(),
        identity: worker.identity,
        audit: worker.audit.clone(),
    }
}

/// Drops every worker that has exited. Dropping its entry closes the lifeline
/// and removes its registry entry.
fn reap(workers: &mut HashMap<String, Supervised>) {
    workers.retain(|_, worker| matches!(worker.child.try_wait(), Ok(None)));
}

/// A child that failed part way through starting: its stdin is already
/// closed, so it ends by its lifeline; this only reaps it.
fn abandon(mut child: Child) {
    let _ = std::thread::Builder::new()
        .name("damaian-worker-reap".to_string())
        .spawn(move || {
            let _ = child.wait();
        });
}

/// The audit log `ProcessRegistry::open_with_audit` builds, so the worker
/// events land beside every other event.
fn audit_log(config: &Config) -> AuditLog {
    AuditLog::with_retention(
        &config.data_dir,
        config.audit_enabled,
        config.audit_retention_days,
        SecretScanner::new(config.secret_patterns.clone()),
    )
}

/// 32 bytes from `/dev/urandom`, as hex.
fn control_token() -> Result<String, String> {
    let mut bytes = [0_u8; 32];
    std::fs::File::open("/dev/urandom")
        .and_then(|mut random| random.read_exact(&mut bytes))
        .map_err(|_| "could not read /dev/urandom for the worker's token".to_string())?;
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

/// The prompt's first non-blank line, which is also where its slug comes from.
fn worker_title(prompt: &str) -> String {
    prompt
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("Worker")
        .chars()
        .take(MAX_TITLE_CHARS)
        .collect()
}

/// One request and its one-line answer. `None` covers every failure: no
/// socket, a closed connection, a timeout, an answer that is not JSON.
fn exchange(
    socket: &Path,
    token: &str,
    command: &str,
    mut arguments: Map<String, Value>,
    timeout: Duration,
) -> Option<Value> {
    let stream = UnixStream::connect(socket).ok()?;
    stream.set_read_timeout(Some(timeout)).ok()?;
    stream.set_write_timeout(Some(timeout)).ok()?;
    arguments.insert("token".to_string(), Value::from(token));
    arguments.insert("command".to_string(), Value::from(command));
    let mut writer = stream.try_clone().ok()?;
    writer
        .write_all(format!("{}\n", Value::Object(arguments)).as_bytes())
        .ok()?;
    let mut line = String::new();
    BufReader::new(stream)
        .take(MAX_ANSWER_BYTES)
        .read_line(&mut line)
        .ok()?;
    serde_json::from_str(&line).ok()
}

/// §5.1 step 9's prompt, sent once the new worker serves its socket. Off the
/// request thread, so a slow worker start never holds the shell.
fn deliver_first_message(endpoint: &Endpoint, text: String, timeouts: &WorkerTimeouts) {
    let deadline = Instant::now() + timeouts.ready;
    let mut arguments = Map::new();
    arguments.insert("text".to_string(), Value::from(text));
    loop {
        if ProcessIdentity::of(endpoint.identity.pid) != Some(endpoint.identity) {
            eprintln!(
                "damaian: worker {} exited before its prompt was sent",
                endpoint.session_id
            );
            return;
        }
        if UnixStream::connect(&endpoint.socket).is_ok() {
            let answer = exchange(
                &endpoint.socket,
                &endpoint.token,
                "message",
                arguments,
                timeouts.exchange,
            );
            if answer.as_ref().is_none_or(|answer| answer["ok"] != true) {
                // The worker's refusals are fixed strings; the prompt is not
                // echoed.
                eprintln!(
                    "damaian: worker {} did not take its prompt: {}",
                    endpoint.session_id,
                    answer
                        .as_ref()
                        .and_then(|answer| answer["error"].as_str())
                        .unwrap_or("no answer")
                );
            }
            return;
        }
        if Instant::now() >= deadline {
            eprintln!(
                "damaian: worker {} did not open its control socket in time; its prompt was not sent",
                endpoint.session_id
            );
            return;
        }
        std::thread::sleep(timeouts.poll);
    }
}

/// Whether the process `identity` names is gone within `within`.
fn wait_for_exit(identity: &ProcessIdentity, timeouts: &WorkerTimeouts, within: Duration) -> bool {
    let deadline = Instant::now() + within;
    loop {
        if ProcessIdentity::of(identity.pid) != Some(*identity) {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(timeouts.poll);
    }
}

/// `SIGTERM` by registry identity, never by name, and audited. The worker's
/// own handler then sweeps its children (§5.3 step 3).
fn terminate(endpoint: &Endpoint) {
    if endpoint.identity.send_sigterm() {
        let _ = endpoint.audit.record(
            "worker_terminated",
            &[
                ("actor", "system".to_string()),
                ("sessionId", endpoint.session_id.clone()),
                ("pid", endpoint.identity.pid.to_string()),
                ("startTimeUs", endpoint.identity.start_time_us.to_string()),
            ],
        );
    }
}

static SUPERVISOR: OnceLock<Supervisor> = OnceLock::new();

/// The process-wide supervisor the shell's routes use.
pub(crate) fn global() -> &'static Supervisor {
    SUPERVISOR.get_or_init(|| {
        let exe = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("damaian-desktop"));
        Supervisor::new(exe, WorkerTimeouts::default())
    })
}

/// For the desktop app's `RunEvent::Exit`: ends every worker this process
/// started. A no-op when none was.
pub fn shutdown_all_workers() {
    if let Some(supervisor) = SUPERVISOR.get() {
        supervisor.shutdown_all();
    }
}

/// A refused or failed worker route: the HTTP status and a message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Refusal {
    pub status: u16,
    pub message: String,
}

impl Refusal {
    fn new(status: u16, message: impl Into<String>) -> Self {
        Self {
            status,
            message: message.into(),
        }
    }
}

/// The parsed parts of a worker route's request.
pub(crate) struct WorkerRequest<'a> {
    pub method: &'a str,
    pub path: &'a str,
    pub query: &'a HashMap<String, String>,
    pub form: &'a HashMap<String, String>,
}

/// Every worker route (tasks.md, Interfaces). `api_key` resolves the model key
/// for a spawn or restart; the shell passes `resolve_shell_model_api_key`,
/// which allows an empty key for a loopback runtime.
///
/// With `enable_worker_sessions` off, everything refuses except the list,
/// which keeps running workers reachable (§5.0), and the setting's own read.
pub(crate) fn route(
    supervisor: &Supervisor,
    config: &Config,
    api_key: &dyn Fn(&Config) -> Result<String, String>,
    request: &WorkerRequest<'_>,
) -> Result<String, Refusal> {
    match (request.method, request.path) {
        ("GET", "/api/workers-enabled") => {
            return Ok(json!({"enabled": config.enable_worker_sessions}).to_string());
        }
        ("GET", "/api/workers") => return list_workers(supervisor, config, request.query),
        _ => {}
    }
    if !config.enable_worker_sessions {
        return Err(Refusal::new(
            403,
            "Worker sessions are turned off. Turn them on in Settings.",
        ));
    }
    match (request.method, request.path) {
        ("GET", "/api/worker-source-status") => {
            let repo = required(request.query, "repo")?;
            let prompt = request.query.get("prompt").cloned().unwrap_or_default();
            let changes = source_changes(Path::new(&repo), &prompt)
                .map_err(|error| Refusal::new(400, error.to_string()))?;
            Ok(json!({"paths": changes.paths, "mentioned": changes.mentioned}).to_string())
        }
        ("POST", "/api/workers") => {
            let spawn = SpawnRequest {
                repo: PathBuf::from(required(request.form, "repo")?),
                prompt: required(request.form, "prompt")?,
                include_uncommitted: is_true(request.form, "include_uncommitted"),
                parent_session_id: request
                    .form
                    .get("parent_session_id")
                    .map(|value| value.trim().to_string())
                    .filter(|value| !value.is_empty()),
            };
            let key = api_key(config).map_err(|error| Refusal::new(400, error))?;
            let spawned = supervisor
                .spawn(config, spawn, &key)
                .map_err(|error| Refusal::new(400, error))?;
            Ok(json!({
                "sessionId": spawned.session_id,
                "title": spawned.title,
                "branch": spawned.branch,
                "worktree": spawned.worktree.display().to_string(),
            })
            .to_string())
        }
        ("GET", "/api/worker-progress") => {
            let session = session_id(request.query, "session")?;
            let after = match request.query.get("after") {
                None => 0,
                Some(value) => value
                    .parse::<u64>()
                    .map_err(|_| Refusal::new(400, "after must be a sequence number"))?,
            };
            let events = read_progress_after(&config.data_dir, &session, after)
                .map_err(|error| Refusal::new(500, error.to_string()))?;
            Ok(json!({"events": events, "alive": supervisor.is_alive(&session)}).to_string())
        }
        ("POST", "/api/worker-control") => control_route(supervisor, request.form),
        ("POST", "/api/worker-restart") => {
            let session = session_id(request.form, "session_id")?;
            let record = worker_record_of(config, &session)?;
            if supervisor.worktree_is_held(config, &session, &record.worktree_path) {
                return Err(Refusal::new(409, WORKTREE_HELD));
            }
            let key = api_key(config).map_err(|error| Refusal::new(400, error))?;
            supervisor
                .restart(config, &session, &key)
                .map_err(|error| Refusal::new(500, error))?;
            Ok(json!({"status": "started"}).to_string())
        }
        ("POST", "/api/worker-remove-worktree") => {
            let session = session_id(request.form, "session_id")?;
            let record = worker_record_of(config, &session)?;
            // Finished workers only (§5.7).
            if supervisor.worktree_is_held(config, &session, &record.worktree_path) {
                return Err(Refusal::new(
                    409,
                    "Stop the worker before removing its worktree.",
                ));
            }
            let confirmed = is_true(request.form, "confirmed_dirty");
            let outcome =
                remove_worker_worktree(&record.source_repository, &record.worktree_path, confirmed)
                    .map_err(|error| Refusal::new(400, error.to_string()))?;
            match outcome {
                RemoveOutcome::Removed => {
                    audit_log(config)
                        .record(
                            "worker_worktree_removed",
                            &[
                                ("actor", "desktop_user".to_string()),
                                ("sessionId", session.clone()),
                                ("worktreePath", record.worktree_path.display().to_string()),
                                ("branch", record.branch.clone()),
                                ("confirmedDirty", confirmed.to_string()),
                            ],
                        )
                        .map_err(|error| Refusal::new(500, error.to_string()))?;
                    Ok(json!({"status": "removed"}).to_string())
                }
                RemoveOutcome::Dirty { paths } => {
                    Ok(json!({"status": "dirty", "paths": paths}).to_string())
                }
            }
        }
        _ => Err(Refusal::new(404, "not found")),
    }
}

/// C7: form fields in, the socket's own names out. Only the fields a command
/// takes are forwarded, so the worker never sees an argument it would refuse.
fn control_route(
    supervisor: &Supervisor,
    form: &HashMap<String, String>,
) -> Result<String, Refusal> {
    let session = session_id(form, "session_id")?;
    let command = required(form, "command")?;
    let mut arguments = Map::new();
    let mut forward = |form_name: &str, socket_name: &str, needed: bool| match form
        .get(form_name)
        .filter(|value| !value.is_empty())
    {
        Some(value) => {
            arguments.insert(socket_name.to_string(), Value::from(value.clone()));
            Ok(())
        }
        None if needed => Err(Refusal::new(
            400,
            format!("missing form field: {form_name}"),
        )),
        None => Ok(()),
    };
    match command.as_str() {
        "message" | "note" => forward("text", "text", true)?,
        "resume" => forward("note", "note", false)?,
        "approve" | "reject" => forward("proposal_id", "proposalId", true)?,
        "pause" | "status" => {}
        "stop" => {
            let answer = supervisor
                .stop(&session)
                .map_err(|error| Refusal::new(409, error))?;
            return Ok(answer.to_string());
        }
        "shutdown" => {
            if !supervisor.shutdown(&session) {
                return Err(Refusal::new(409, NOT_RUNNING));
            }
            return Ok(json!({"ok": true}).to_string());
        }
        _ => {
            return Err(Refusal::new(
                400,
                format!("Unknown worker command: {command}"),
            ));
        }
    }
    if !supervisor.is_alive(&session) {
        return Err(Refusal::new(409, NOT_RUNNING));
    }
    supervisor
        .control(&session, &command, arguments)
        .map(|answer| answer.to_string())
        .map_err(|error| Refusal::new(500, error))
}

fn required(fields: &HashMap<String, String>, name: &str) -> Result<String, Refusal> {
    fields
        .get(name)
        .filter(|value| !value.trim().is_empty())
        .cloned()
        .ok_or_else(|| Refusal::new(400, format!("missing field: {name}")))
}

/// A session id, checked before any path is built from it.
fn session_id(fields: &HashMap<String, String>, name: &str) -> Result<String, Refusal> {
    let session = required(fields, name)?;
    crate::worker::validate_session_id(&session).map_err(|error| Refusal::new(400, error))?;
    Ok(session)
}

fn is_true(fields: &HashMap<String, String>, name: &str) -> bool {
    fields.get(name).map(String::as_str) == Some("true")
}

fn worker_record_of(config: &Config, session: &str) -> Result<WorkerRecord, Refusal> {
    SessionStore::new(&config.data_dir)
        .worker_record(session)
        .map_err(|error| Refusal::new(500, error.to_string()))?
        .ok_or_else(|| Refusal::new(400, "This session is not a worker."))
}

/// The board's groups, in the order §5.7 shows them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Group {
    NeedsYou,
    Running,
    Finished,
}

impl Group {
    fn as_str(self) -> &'static str {
        match self {
            Self::NeedsYou => "needs_you",
            Self::Running => "running",
            Self::Finished => "finished",
        }
    }
}

/// `GET /api/workers?repo=`: every worker whose source is `repo`, from its
/// record, its progress file and whether this shell still holds it.
fn list_workers(
    supervisor: &Supervisor,
    config: &Config,
    query: &HashMap<String, String>,
) -> Result<String, Refusal> {
    let repo = required(query, "repo")?;
    let source = std::fs::canonicalize(&repo)
        .map_err(|error| Refusal::new(400, format!("{repo}: {error}")))?;
    let store = SessionStore::new(&config.data_dir);
    let sessions = store
        .worker_sessions_for_source(&source)
        .map_err(|error| Refusal::new(500, error.to_string()))?;
    let mut rows = Vec::new();
    for session in &sessions {
        let Some(record) = store
            .worker_record(&session.id)
            .map_err(|error| Refusal::new(500, error.to_string()))?
        else {
            continue;
        };
        rows.push(worker_row(supervisor, config, &store, session, &record));
    }
    // Newest first within each group.
    rows.sort_by(|(group_a, created_a, _), (group_b, created_b, _)| {
        group_a.cmp(group_b).then(created_b.cmp(created_a))
    });
    let rows: Vec<Value> = rows.into_iter().map(|(_, _, row)| row).collect();
    Ok(json!({"workers": rows}).to_string())
}

fn worker_row(
    supervisor: &Supervisor,
    config: &Config,
    store: &SessionStore,
    session: &Session,
    record: &WorkerRecord,
) -> (Group, u128, Value) {
    let alive = supervisor.is_alive(&session.id);
    let events = read_progress_after(&config.data_dir, &session.id, 0).unwrap_or_default();
    let last = |kind: &str| {
        events
            .iter()
            .rev()
            .find(|event| event["kind"] == kind)
            .map(|event| event["payload"].clone())
    };
    let status = last("status").unwrap_or(Value::Null);
    let phase = last("phase").unwrap_or(Value::Null);
    let reported = status["state"].as_str().unwrap_or("idle").to_string();
    let mut task_status = status["taskStatus"]
        .as_str()
        .map(str::to_string)
        .or_else(|| {
            store
                .read_tasks(&session.id)
                .ok()
                .and_then(|tasks| tasks.last().map(|task| task.status.as_str().to_string()))
        });
    let finished_task = task_status
        .as_deref()
        .and_then(TaskStatus::parse)
        .is_some_and(|status| status.is_terminal());
    let group = match (alive, reported.as_str()) {
        (true, "waiting_for_approval" | "paused") => Group::NeedsYou,
        (true, "running") => Group::Running,
        // Idle before its first turn has finished: starting, not done.
        (true, "idle") if !finished_task => Group::Running,
        _ => Group::Finished,
    };
    // A process that ended part way through a turn left it unfinished (§5.3).
    if !alive && task_status.is_some() && !finished_task {
        task_status = Some(TaskStatus::Interrupted.as_str().to_string());
    }
    let state = if alive { reported.as_str() } else { "exited" };
    let doing = if let Some(error) = status["error"].as_str() {
        Some(error.to_string())
    } else {
        match state {
            "waiting_for_approval" => Some("Waiting for approval".to_string()),
            "paused" => Some("Paused".to_string()),
            "running" => phase["label"].as_str().map(str::to_string),
            _ => None,
        }
    };
    let parent_title = record
        .parent_session_id
        .as_deref()
        .and_then(|parent| store.read_session(parent).ok().flatten())
        .map(|parent| parent.title);
    let (files_changed, commits_ahead) = if group == Group::Finished {
        (
            git_count(
                &record.worktree_path,
                &["diff", "--name-only", &record.base_commit],
            ),
            git_count(
                &record.worktree_path,
                &[
                    "rev-list",
                    "--count",
                    &format!("{}..HEAD", record.base_commit),
                ],
            ),
        )
    } else {
        (None, None)
    };
    let row = json!({
        "sessionId": session.id,
        "title": session.title,
        "parentSessionId": record.parent_session_id,
        "parentTitle": parent_title,
        "branch": record.branch,
        "worktree": record.worktree_path.display().to_string(),
        "alive": alive,
        "state": state,
        "taskStatus": task_status,
        "group": group.as_str(),
        "round": phase["round"],
        "maxRounds": phase["maxRounds"],
        // Additive for spec 59 §5.5: a row written before the exploration
        // allowance landed has no such fields, and both are `null` then.
        "explorationRound": phase["explorationRound"],
        "maxExplorationRounds": phase["maxExplorationRounds"],
        "doing": doing,
        "createdAtMs": session.created_at_ms as u64,
        "updatedAtMs": session.updated_at_ms as u64,
        "filesChanged": files_changed,
        "commitsAhead": commits_ahead,
    });
    (group, session.created_at_ms, row)
}

/// One `git -C <worktree>` call, read as a count: `rev-list --count` prints
/// one, and `diff --name-only` prints one path per line. `None` when the
/// worktree is gone or git fails.
fn git_count(worktree: &Path, args: &[&str]) -> Option<u64> {
    if !worktree.is_dir() {
        return None;
    }
    let output = Command::new("git")
        .arg("--no-optional-locks")
        .arg("-C")
        .arg(worktree)
        .args(args)
        .stdin(Stdio::null())
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout);
    if args.first() == Some(&"rev-list") {
        text.trim().parse().ok()
    } else {
        Some(text.lines().filter(|line| !line.is_empty()).count() as u64)
    }
}

/// §5.3 step 5: deleting a session sends `shutdown` to the workers it
/// started, and to its own worker if it is one. Read before the log goes,
/// because the children are recorded in it. Worktrees and the workers' own
/// session logs are kept.
pub(crate) fn workers_to_stop_for_deleted_session(
    store: &SessionStore,
    session_id: &str,
) -> Vec<String> {
    let mut workers = store.worker_children(session_id).unwrap_or_default();
    if store.worker_record(session_id).ok().flatten().is_some() {
        workers.push(session_id.to_string());
    }
    workers
}

/// What a stop escalation talks to. The real one is a worker's socket and its
/// registry identity; tests inject a fake.
pub(crate) trait StopTarget {
    /// One request; `None` when the worker did not answer.
    fn send(&mut self, command: &str) -> Option<Value>;
    fn has_exited(&mut self) -> bool;
    /// `SIGTERM` by registry identity.
    fn terminate(&mut self);
}

struct SocketTarget {
    endpoint: Endpoint,
    exchange: Duration,
}

impl StopTarget for SocketTarget {
    fn send(&mut self, command: &str) -> Option<Value> {
        exchange(
            &self.endpoint.socket,
            &self.endpoint.token,
            command,
            Map::new(),
            self.exchange,
        )
    }

    fn has_exited(&mut self) -> bool {
        ProcessIdentity::of(self.endpoint.identity.pid) != Some(self.endpoint.identity)
    }

    fn terminate(&mut self) {
        terminate(&self.endpoint);
    }
}

/// Where a stop goes after its first, synchronous exchanges.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum StopNext {
    /// No turn to stop: nothing follows.
    Done,
    /// `stop` was accepted; wait for the turn to end.
    AwaitTurnEnd,
    /// Straight to `shutdown`: waiting for approval refuses `stop` (Task 6),
    /// or the worker did not answer.
    Shutdown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum StopOutcome {
    Done,
    TurnEnded,
    ExitedAfterShutdown,
    Terminated,
}

fn no_answer() -> Value {
    json!({"ok": false, "error": NO_ANSWER})
}

/// The synchronous half of a stop: `status`, then `stop` when a turn runs.
/// Returns the answer the route relays and what follows.
pub(crate) fn begin_stop(target: &mut dyn StopTarget) -> (Value, StopNext) {
    let Some(status) = target.send("status") else {
        return (no_answer(), StopNext::Shutdown);
    };
    match status["state"].as_str() {
        // `stop` would be refused; Task 6's progress row says to go straight
        // to `shutdown`.
        Some("waiting_for_approval") => (
            json!({"ok": true, "escalation": "shutdown"}),
            StopNext::Shutdown,
        ),
        Some("running" | "paused") => match target.send("stop") {
            Some(answer) if answer["ok"] == true => (answer, StopNext::AwaitTurnEnd),
            // The turn moved on between the two requests: to an approval,
            // which `stop` cannot end, or to its end, which needs nothing.
            Some(answer) => {
                let waiting = target
                    .send("status")
                    .is_some_and(|status| status["state"] == "waiting_for_approval");
                (
                    answer,
                    if waiting {
                        StopNext::Shutdown
                    } else {
                        StopNext::Done
                    },
                )
            }
            None => (no_answer(), StopNext::Shutdown),
        },
        // Idle or already shutting down: the worker's own refusal says so.
        _ => (
            target.send("stop").unwrap_or_else(no_answer),
            StopNext::Done,
        ),
    }
}

/// The waiting half of §5.3's stop escalation, run off the request thread:
/// the turn gets `stop_to_shutdown` to end, then `shutdown`, then the process
/// gets `shutdown_to_sigterm` to exit before `SIGTERM`.
pub(crate) fn finish_stop(
    target: &mut dyn StopTarget,
    next: StopNext,
    timeouts: &WorkerTimeouts,
) -> StopOutcome {
    match next {
        StopNext::Done => return StopOutcome::Done,
        StopNext::Shutdown => {}
        StopNext::AwaitTurnEnd => {
            let deadline = Instant::now() + timeouts.stop_to_shutdown;
            loop {
                if target.has_exited() {
                    return StopOutcome::TurnEnded;
                }
                let ended = target.send("status").is_some_and(|status| {
                    matches!(status["state"].as_str(), Some("idle" | "shutting_down"))
                });
                if ended {
                    return StopOutcome::TurnEnded;
                }
                if Instant::now() >= deadline {
                    break;
                }
                std::thread::sleep(timeouts.poll);
            }
        }
    }
    target.send("shutdown");
    let deadline = Instant::now() + timeouts.shutdown_to_sigterm;
    loop {
        if target.has_exited() {
            return StopOutcome::ExitedAfterShutdown;
        }
        if Instant::now() >= deadline {
            break;
        }
        std::thread::sleep(timeouts.poll);
    }
    target.terminate();
    StopOutcome::Terminated
}
