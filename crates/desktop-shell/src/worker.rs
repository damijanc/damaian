//! The `--worker` host: the same executable re-entered to serve one worker
//! session, per `docs/specs/57_worker_sessions/proposal.md` §5.2 and §5.3.
//!
//! It owns the stdin handover, the lifeline, the control socket and the
//! shutdown path, and hosts the session's turns on one turn thread. A turn runs
//! through the same `ChatOrchestrator` entry points and the same engine and
//! adapter construction as the shell's (`crate::chat_engine`,
//! `crate::chat_model_adapter`); only where progress goes and what drives
//! pause, notes and cancel differ (§5.2 "Same turn").
//!
//! The API key arrives on one stdin line and lives only in memory. Nothing here
//! writes it to the environment, argv, disk or any message: every error below
//! is a fixed string, so no input can be echoed through one.

use std::cell::RefCell;
use std::fs;
use std::io::{BufRead, BufReader, IsTerminal, Read, Write};
use std::os::unix::fs::{DirBuilderExt, FileTypeExt, PermissionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use serde_json::{Map, Value, json};
use workspace_engine::worker_progress::{ProgressKind, ProgressWriter};
use workspace_engine::{
    CancelToken, ChatTurnOptions, ChatTurnResult, Config, ModelAdapter, PhaseKind, ProcessRegistry,
    SecretScanner, TaskStatus, TurnProgress, TurnSink, TurnSteering,
};

/// The first argument that selects worker mode, in both `main`s.
pub const WORKER_FLAG: &str = "--worker";

/// Room for `{"apiKey": …, "token": …}` with any real provider key.
const MAX_HANDOVER_BYTES: u64 = 16 * 1024;
/// One request line. A `message` carries a whole prompt, so this is generous;
/// it exists so a client cannot make the worker buffer without limit.
const MAX_REQUEST_BYTES: u64 = 256 * 1024;
/// A steering note. The composer has no length limit (Task 2's discovery), so
/// the socket is where notes are bounded.
pub const MAX_NOTE_BYTES: usize = 16 * 1024;
const MAX_PROPOSAL_ID_BYTES: usize = 256;
const MAX_SESSION_ID_BYTES: usize = 128;
/// macOS's `sockaddr_un.sun_path`, including the terminating NUL.
const SUN_PATH_CAPACITY: usize = 104;
/// The control token is 32 random bytes, written as hex.
const TOKEN_HEX_LEN: usize = 64;
/// How long one control connection may sit idle. The shell's exchanges are
/// short, and the socket serves one connection at a time, so a client that
/// stops talking must not hold it.
const CONNECTION_IDLE_TIMEOUT: Duration = Duration::from_secs(5);

/// Worker-mode entry point for both `main`s. `args` is the whole argv,
/// program name included. Returns the process exit code.
pub fn run_worker_from_args(args: Vec<String>) -> i32 {
    match run_worker(&args) {
        Ok(()) => 0,
        Err(error) => {
            eprintln!("damaian worker: {error}");
            1
        }
    }
}

/// Proposal §5.3's order: nothing is served until the handover is read and
/// the lifeline and the signal handler are both in place.
fn run_worker(args: &[String]) -> Result<(), String> {
    let WorkerArgs {
        session_id,
        data_dir,
    } = parse_worker_args(args)?;
    refuse_terminal_stdin(std::io::stdin().is_terminal())?;
    let Handover { api_key, token } = read_handover(&mut std::io::stdin().lock())?;
    let config = load_worker_config(&data_dir, None)?;
    let (registry, audit) =
        ProcessRegistry::open_with_audit(&config).map_err(|error| error.to_string())?;

    let shutdown = Arc::new(ShutdownSignal::default());
    let lifeline = Arc::clone(&shutdown);
    std::thread::Builder::new()
        .name("damaian-worker-lifeline".to_string())
        .spawn(move || {
            watch_lifeline(std::io::stdin().lock());
            lifeline.trigger();
        })
        .map_err(|error| format!("could not start the lifeline thread: {error}"))?;
    registry
        .clone()
        .install_shutdown_handler(audit.clone())
        .map_err(|error| error.to_string())?;

    let (listener, socket_path) = bind_control_socket(&data_dir, &session_id)?;
    let host = Arc::new(WorkerHost::new(session_id.clone(), Arc::clone(&shutdown)));
    // The key is moved into the adapter builder and goes nowhere else: not
    // the environment of anything this worker runs, not a file, not a log.
    let driver = TurnDriver::new(
        &session_id,
        &data_dir,
        Box::new(move |config: &Config| crate::chat_model_adapter(config, || Ok(api_key.clone()))),
    )?;
    start_turn_thread(&host, driver)?;
    let served = Arc::clone(&host);
    crate::spawn_with_shell_stack("damaian-worker-control", move || {
        serve_control(listener, &token, &served);
    })
    .map_err(|error| format!("could not start the control thread: {error}"))?;

    shutdown.wait();
    host.set_state(WorkerState::ShuttingDown);
    host.cancel_turn();
    let swept = registry.sweep_own(&audit);
    let _ = fs::remove_file(&socket_path);
    swept.map(|_| ()).map_err(|error| error.to_string())
}

/// Blocks until stdin reaches EOF or fails. The shell holds the write end for
/// the worker's whole life, so this returns only when the shell let go of it
/// or died, `SIGKILL` included: the kernel closes the pipe either way.
fn watch_lifeline(mut stdin: impl Read) {
    let mut buffer = [0_u8; 512];
    loop {
        match stdin.read(&mut buffer) {
            Ok(0) => return,
            // Nothing is expected after the handover; anything is discarded.
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
            Err(_) => return,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct WorkerArgs {
    session_id: String,
    data_dir: PathBuf,
}

fn parse_worker_args(args: &[String]) -> Result<WorkerArgs, String> {
    const USAGE: &str = "usage: --worker --session <id> --data-dir <absolute dir>";
    let mut rest = args.iter().skip(1).map(String::as_str);
    if rest.next() != Some(WORKER_FLAG) {
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
    // A relative directory would resolve against whatever the cwd happens to
    // be, and the socket path with it.
    if !data_dir.is_absolute() {
        return Err(USAGE.to_string());
    }
    Ok(WorkerArgs {
        session_id,
        data_dir,
    })
}

/// Session ids are `create_id` output; anything else is not one of ours.
fn validate_session_id(session_id: &str) -> Result<(), String> {
    let well_formed = !session_id.is_empty()
        && session_id.len() <= MAX_SESSION_ID_BYTES
        && session_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-');
    if well_formed {
        Ok(())
    } else {
        Err("the session id is not a Damaian session id".to_string())
    }
}

/// Worker mode cannot be run by hand: without the handover there is no key and
/// no token, and a terminal on stdin means nobody is holding the lifeline.
fn refuse_terminal_stdin(is_terminal: bool) -> Result<(), String> {
    if is_terminal {
        return Err(
            "worker mode reads its handover from the shell and cannot run from a terminal"
                .to_string(),
        );
    }
    Ok(())
}

/// What the shell writes on the worker's stdin. `Debug` never prints either
/// value.
struct Handover {
    api_key: String,
    token: String,
}

impl std::fmt::Debug for Handover {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("Handover { .. }")
    }
}

fn read_handover(reader: &mut impl BufRead) -> Result<Handover, String> {
    let mut line = Vec::new();
    reader
        .take(MAX_HANDOVER_BYTES + 1)
        .read_until(b'\n', &mut line)
        .map_err(|_| "could not read the handover from stdin".to_string())?;
    if line.len() as u64 > MAX_HANDOVER_BYTES {
        return Err("the handover line is too long".to_string());
    }
    // A line without its newline means stdin closed part way: the shell that
    // started this worker is already gone.
    if line.pop() != Some(b'\n') {
        return Err("stdin closed before the handover line ended".to_string());
    }
    parse_handover(&line)
}

/// Every refusal is a fixed string. In particular serde's own messages are
/// never passed on, because some of them quote the input.
fn parse_handover(line: &[u8]) -> Result<Handover, String> {
    const MALFORMED: &str = "the handover line is not {\"apiKey\": …, \"token\": …}";
    let Ok(Value::Object(mut fields)) = serde_json::from_slice::<Value>(line) else {
        return Err(MALFORMED.to_string());
    };
    let (Some(Value::String(api_key)), Some(Value::String(token))) =
        (fields.remove("apiKey"), fields.remove("token"))
    else {
        return Err(MALFORMED.to_string());
    };
    if !fields.is_empty() {
        return Err("the handover line has an unexpected field".to_string());
    }
    if token.len() != TOKEN_HEX_LEN || !token.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err("the control token is not 64 hex characters".to_string());
    }
    Ok(Handover { api_key, token })
}

/// `<data_dir>/workers/<16 hex of sha256(session id)>.sock`.
///
/// Named by a hash because the session id does not fit: measured on
/// 2026-10-12, the id-named path is 107 bytes under the real Application
/// Support directory and 132 under a `/var/folders/…` test directory, against
/// `sun_path`'s 104. Refuses when even the hashed path does not fit.
pub fn worker_socket_path(data_dir: &Path, session_id: &str) -> Result<PathBuf, String> {
    let digest = workspace_engine::hash::sha256(session_id);
    let path = data_dir
        .join("workers")
        .join(format!("{}.sock", &digest[..16]));
    let length = path.as_os_str().len();
    if length >= SUN_PATH_CAPACITY {
        return Err(format!(
            "the worker socket path is {length} bytes, and macOS allows {} \
             (sun_path is {SUN_PATH_CAPACITY} bytes with its NUL); use a shorter \
             data directory",
            SUN_PATH_CAPACITY - 1
        ));
    }
    Ok(path)
}

/// Binds `0600` in a `0700` directory. The directory is private before the
/// socket exists, so nobody else can reach it between `bind` and `chmod`.
fn bind_control_socket(
    data_dir: &Path,
    session_id: &str,
) -> Result<(UnixListener, PathBuf), String> {
    let path = worker_socket_path(data_dir, session_id)?;
    let dir = path
        .parent()
        .ok_or_else(|| "the worker socket has no directory".to_string())?;
    fs::create_dir_all(data_dir)
        .map_err(|error| format!("could not create the data directory: {error}"))?;
    match fs::symlink_metadata(dir) {
        Ok(meta) if meta.file_type().is_dir() => {}
        Ok(_) => return Err(format!("{} is not a directory", dir.display())),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            fs::DirBuilder::new()
                .mode(0o700)
                .create(dir)
                .map_err(|error| format!("could not create {}: {error}", dir.display()))?;
        }
        Err(error) => return Err(format!("could not inspect {}: {error}", dir.display())),
    }
    fs::set_permissions(dir, fs::Permissions::from_mode(0o700))
        .map_err(|error| format!("could not make {} private: {error}", dir.display()))?;

    match fs::symlink_metadata(&path) {
        Ok(meta) if meta.file_type().is_socket() => {
            // A socket someone still answers on belongs to a live worker for
            // this session. One left by a crash refuses the connection.
            if UnixStream::connect(&path).is_ok() {
                return Err("a worker is already serving this session".to_string());
            }
            fs::remove_file(&path)
                .map_err(|error| format!("could not remove a stale worker socket: {error}"))?;
        }
        Ok(_) => return Err(format!("{} is not a socket", path.display())),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(format!("could not inspect {}: {error}", path.display())),
    }
    let listener = UnixListener::bind(&path)
        .map_err(|error| format!("could not bind the worker socket: {error}"))?;
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600))
        .map_err(|error| format!("could not make the worker socket private: {error}"))?;
    Ok((listener, path))
}

/// The worker's config comes from the data directory the shell named, never
/// from `Config::default_data_dir()`, so a worker can never read or write a
/// different instance's state.
///
/// With `repository`, its `.damaian/config.conf` is applied at repository
/// scope, exactly as `Config::load_for_repository` applies it for the shell's
/// turns: it may tighten, and cannot loosen (#34).
fn load_worker_config(data_dir: &Path, repository: Option<&Path>) -> Result<Config, String> {
    let base = Config {
        data_dir: data_dir.to_path_buf(),
        ..Config::default()
    };
    let user_path = base.user_config_path();
    let admin_path = base.admin_config_path();
    let repo_path = repository.map(Config::repository_config_path);
    let (mut config, _) = Config::load_scoped(
        base,
        Some(&user_path),
        repo_path.as_deref(),
        Some(&admin_path),
        repository,
    )
    .map_err(|error| error.to_string())?;
    // The shell passed its effective directory, which already reflects any
    // `data_dir` its own config named. Following that key again from here
    // would move the worker away from the shell it serves.
    config.data_dir = data_dir.to_path_buf();
    Ok(config)
}

/// A control request, validated against proposal §5.2's table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ControlCommand {
    Message { text: String },
    Pause,
    Resume { note: Option<String> },
    Note { text: String },
    Approve { proposal_id: String },
    Reject { proposal_id: String },
    Stop,
    Shutdown,
    Status,
}

#[derive(Debug, PartialEq, Eq)]
enum ParsedRequest {
    /// Missing, malformed or wrong token: the connection is closed unanswered.
    Unauthorized,
    /// Authorized, but not a valid command. The message is a fixed string.
    Invalid(&'static str),
    Command(ControlCommand),
}

fn parse_request(line: &[u8], token: &str) -> ParsedRequest {
    let Ok(Value::Object(mut fields)) = serde_json::from_slice::<Value>(line) else {
        return ParsedRequest::Unauthorized;
    };
    match fields.remove("token") {
        Some(Value::String(offered)) if tokens_match(&offered, token) => {}
        _ => return ParsedRequest::Unauthorized,
    }
    let Some(Value::String(command)) = fields.remove("command") else {
        return ParsedRequest::Invalid("missing command");
    };
    match parse_command(&command, fields) {
        Ok(command) => ParsedRequest::Command(command),
        Err(error) => ParsedRequest::Invalid(error),
    }
}

/// Proposal §5.2's table, with no field beyond the ones it names.
fn parse_command(
    command: &str,
    mut fields: Map<String, Value>,
) -> Result<ControlCommand, &'static str> {
    let mut take = |name: &str, limit: usize| -> Result<Option<String>, &'static str> {
        match fields.remove(name) {
            None => Ok(None),
            Some(Value::String(value)) if value.len() <= limit => Ok(Some(value)),
            Some(Value::String(_)) => Err("an argument is too long"),
            Some(_) => Err("an argument is not a string"),
        }
    };
    let required = |value: Option<String>| value.ok_or("a required argument is missing");
    let parsed = match command {
        "message" => ControlCommand::Message {
            text: required(take("text", MAX_REQUEST_BYTES as usize)?)?,
        },
        "pause" => ControlCommand::Pause,
        "resume" => ControlCommand::Resume {
            note: take("note", MAX_NOTE_BYTES)?,
        },
        "note" => ControlCommand::Note {
            text: required(take("text", MAX_NOTE_BYTES)?)?,
        },
        "approve" => ControlCommand::Approve {
            proposal_id: required(take("proposalId", MAX_PROPOSAL_ID_BYTES)?)?,
        },
        "reject" => ControlCommand::Reject {
            proposal_id: required(take("proposalId", MAX_PROPOSAL_ID_BYTES)?)?,
        },
        "stop" => ControlCommand::Stop,
        "shutdown" => ControlCommand::Shutdown,
        "status" => ControlCommand::Status,
        _ => return Err("unknown command"),
    };
    if !fields.is_empty() {
        return Err("unexpected argument");
    }
    Ok(parsed)
}

/// Compares every byte whatever the first mismatch, so the time an answer
/// takes says nothing about how much of a guess was right.
fn tokens_match(offered: &str, expected: &str) -> bool {
    offered.len() == expected.len()
        && offered
            .bytes()
            .zip(expected.bytes())
            .fold(0_u8, |difference, (a, b)| difference | (a ^ b))
            == 0
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum WorkerState {
    Idle,
    Running,
    /// Held at a round boundary (§5.4). Set when the turn reports it, not
    /// when `pause` is asked for: a pause waits for the next boundary.
    Paused,
    WaitingForApproval,
    ShuttingDown,
}

impl WorkerState {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::Running => "running",
            Self::Paused => "paused",
            Self::WaitingForApproval => "waiting_for_approval",
            Self::ShuttingDown => "shutting_down",
        }
    }
}

/// Set once, by the lifeline or by `shutdown`; the main thread waits on it.
#[derive(Debug, Default)]
struct ShutdownSignal {
    requested: Mutex<bool>,
    changed: Condvar,
}

impl ShutdownSignal {
    fn trigger(&self) {
        *self.lock() = true;
        self.changed.notify_all();
    }

    fn is_triggered(&self) -> bool {
        *self.lock()
    }

    fn wait(&self) {
        let mut requested = self.lock();
        while !*requested {
            requested = self
                .changed
                .wait(requested)
                .unwrap_or_else(|poison| poison.into_inner());
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, bool> {
        self.requested
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
    }
}

/// Which resume a pending approval takes (§5.6).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DecisionKind {
    Command,
    Plan,
}

/// The turn the host holds. The cancel token and the steering belong to the
/// turn in flight and are made fresh for each, so a `stop` that arrives late
/// cannot cancel the next turn.
struct TurnSlot {
    state: WorkerState,
    cancel: CancelToken,
    steering: Arc<TurnSteering>,
    pending: Option<(String, DecisionKind)>,
}

/// What the control thread hands the turn thread.
enum TurnJob {
    Message(String),
    Decide {
        proposal_id: String,
        kind: DecisionKind,
        approved: bool,
    },
}

pub(crate) struct WorkerHost {
    session_id: String,
    pid: u32,
    turn: Mutex<TurnSlot>,
    shutdown: Arc<ShutdownSignal>,
    /// `None` until a turn thread is attached; the socket tests serve without
    /// one.
    jobs: Mutex<Option<Sender<TurnJob>>>,
}

impl WorkerHost {
    fn new(session_id: String, shutdown: Arc<ShutdownSignal>) -> Self {
        Self {
            session_id,
            pid: std::process::id(),
            turn: Mutex::new(TurnSlot {
                state: WorkerState::Idle,
                cancel: CancelToken::new(),
                steering: Arc::new(TurnSteering::new()),
                pending: None,
            }),
            shutdown,
            jobs: Mutex::new(None),
        }
    }

    fn slot(&self) -> std::sync::MutexGuard<'_, TurnSlot> {
        self.turn
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
    }

    fn set_state(&self, state: WorkerState) {
        self.slot().state = state;
    }

    /// Stops the turn in flight at its next safe point, and lets a paused
    /// one leave its wait to see the cancel.
    fn cancel_turn(&self) {
        let slot = self.slot();
        slot.cancel.cancel();
        slot.steering.wake();
    }

    fn turn_handles(&self) -> (CancelToken, Arc<TurnSteering>) {
        let slot = self.slot();
        (slot.cancel.clone(), Arc::clone(&slot.steering))
    }

    /// The turn reached a round boundary and held, or left it.
    fn set_paused(&self, paused: bool) {
        let mut slot = self.slot();
        if matches!(slot.state, WorkerState::Running | WorkerState::Paused) {
            slot.state = if paused {
                WorkerState::Paused
            } else {
                WorkerState::Running
            };
        }
    }

    /// The turn thread finished a job: idle, or holding an approval.
    fn settle(&self, pending: Option<(String, DecisionKind)>) {
        let mut slot = self.slot();
        if slot.state == WorkerState::ShuttingDown {
            return;
        }
        slot.state = if pending.is_some() {
            WorkerState::WaitingForApproval
        } else {
            WorkerState::Idle
        };
        slot.pending = pending;
    }

    /// Answers one command. The flag says whether to close the connection.
    pub(crate) fn dispatch(&self, command: ControlCommand) -> (Value, bool) {
        match command {
            ControlCommand::Status => (self.snapshot(), false),
            ControlCommand::Shutdown => {
                self.set_state(WorkerState::ShuttingDown);
                self.cancel_turn();
                self.shutdown.trigger();
                (json!({"ok": true}), true)
            }
            ControlCommand::Message { text } => (self.start(TurnJob::Message(text)), false),
            ControlCommand::Approve { proposal_id } => (self.decide(proposal_id, true), false),
            ControlCommand::Reject { proposal_id } => (self.decide(proposal_id, false), false),
            ControlCommand::Pause => (self.steer(|slot| slot.steering.pause()), false),
            ControlCommand::Resume { note } => (
                self.steer(|slot| {
                    if let Some(note) = note {
                        slot.steering.queue_note(note);
                    }
                    slot.steering.resume();
                }),
                false,
            ),
            ControlCommand::Note { text } => {
                (self.steer(|slot| slot.steering.queue_note(text)), false)
            }
            ControlCommand::Stop => (
                self.steer(|slot| {
                    slot.cancel.cancel();
                    slot.steering.wake();
                }),
                false,
            ),
        }
    }

    /// Pause, resume, notes and stop act on a running turn only. A turn
    /// waiting for approval has already left its loop, so they do not apply
    /// there (§5.4); it is resolved with `approve` or `reject`.
    fn steer(&self, apply: impl FnOnce(&TurnSlot)) -> Value {
        let slot = self.slot();
        match slot.state {
            WorkerState::Running | WorkerState::Paused => {
                apply(&slot);
                json!({"ok": true})
            }
            WorkerState::WaitingForApproval => {
                refusal("the turn is waiting for approval; approve or reject it")
            }
            WorkerState::Idle => refusal("no turn is running"),
            WorkerState::ShuttingDown => refusal("the worker is shutting down"),
        }
    }

    /// `message`: refused unless idle (§5.2).
    fn start(&self, job: TurnJob) -> Value {
        let mut slot = self.slot();
        match slot.state {
            WorkerState::Idle => {}
            WorkerState::Running | WorkerState::Paused => {
                return refusal("a turn is already running");
            }
            WorkerState::WaitingForApproval => {
                return refusal("the turn is waiting for approval; approve or reject it");
            }
            WorkerState::ShuttingDown => return refusal("the worker is shutting down"),
        }
        self.hand_over(&mut slot, job)
    }

    fn decide(&self, proposal_id: String, approved: bool) -> Value {
        let mut slot = self.slot();
        let kind = match &slot.pending {
            Some((pending, kind))
                if slot.state == WorkerState::WaitingForApproval && *pending == proposal_id =>
            {
                *kind
            }
            _ => return refusal("no approval with that id is pending"),
        };
        let answer = self.hand_over(
            &mut slot,
            TurnJob::Decide {
                proposal_id,
                kind,
                approved,
            },
        );
        if answer["ok"] == true {
            slot.pending = None;
        }
        answer
    }

    /// Gives the turn thread a job with a fresh cancel token and steering.
    fn hand_over(&self, slot: &mut TurnSlot, job: TurnJob) -> Value {
        let jobs = self
            .jobs
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        let Some(jobs) = jobs.as_ref() else {
            return refusal("this worker hosts no turns");
        };
        slot.cancel = CancelToken::new();
        slot.steering = Arc::new(TurnSteering::new());
        let previous = slot.state;
        slot.state = WorkerState::Running;
        if jobs.send(job).is_err() {
            slot.state = previous;
            return refusal("the worker's turn thread has stopped");
        }
        json!({"ok": true})
    }

    pub(crate) fn snapshot(&self) -> Value {
        let slot = self.slot();
        let mut snapshot = json!({
            "ok": true,
            "sessionId": self.session_id,
            "pid": self.pid,
            "state": slot.state.as_str(),
        });
        if let (WorkerState::WaitingForApproval, Some((proposal_id, _))) =
            (slot.state, &slot.pending)
        {
            snapshot["proposalId"] = json!(proposal_id);
        }
        snapshot
    }
}

/// How often streamed model text is written to the progress file (§5.5).
const TEXT_BATCH_INTERVAL: Duration = Duration::from_millis(250);

/// Builds a turn's model adapter from its config. A worker's builds the
/// shell's adapter with the handed-over key (`crate::chat_model_adapter`); a
/// test scripts one.
pub(crate) type WorkerAdapterFor =
    dyn FnMut(&Config) -> Result<Box<dyn ModelAdapter>, String> + Send;

/// Runs the session's turns, one job at a time, on the turn thread. It alone
/// holds the progress writer (Task 3: one writer per worker, on one thread).
pub(crate) struct TurnDriver {
    session_id: String,
    data_dir: PathBuf,
    /// The worktree, read from the session's worker record on first use.
    repository: Option<PathBuf>,
    adapter_for: Box<WorkerAdapterFor>,
    progress: ProgressWriter,
}

impl TurnDriver {
    /// Truncates the session's progress file: a restarted worker starts again
    /// from `seq` 1 (§5.5).
    pub(crate) fn new(
        session_id: &str,
        data_dir: &Path,
        adapter_for: Box<WorkerAdapterFor>,
    ) -> Result<Self, String> {
        let config = load_worker_config(data_dir, None)?;
        let progress = ProgressWriter::create(
            data_dir,
            session_id,
            SecretScanner::new(config.secret_patterns.clone()),
        )
        .map_err(|error| format!("could not create the progress file: {error}"))?;
        Ok(Self {
            session_id: session_id.to_string(),
            data_dir: data_dir.to_path_buf(),
            repository: None,
            adapter_for,
            progress,
        })
    }

    fn serve(mut self, host: &WorkerHost, jobs: Receiver<TurnJob>) {
        for job in jobs {
            self.run(host, job);
        }
    }

    fn run(&mut self, host: &WorkerHost, job: TurnJob) {
        let (cancel, steering) = host.turn_handles();
        append_progress(
            &mut self.progress,
            ProgressKind::Status,
            json!({"state": WorkerState::Running.as_str()}),
        );
        match self.take_turn(host, job, &cancel, &steering) {
            Ok(result) => {
                let pending = pending_decision(&result);
                if let Some(approval) = approval_payload(&result) {
                    append_progress(&mut self.progress, ProgressKind::Approval, approval);
                }
                let state = if pending.is_some() {
                    WorkerState::WaitingForApproval
                } else {
                    WorkerState::Idle
                };
                append_progress(
                    &mut self.progress,
                    ProgressKind::Status,
                    json!({"state": state.as_str(), "taskStatus": result.task.status.as_str()}),
                );
                host.settle(pending);
            }
            Err(error) => {
                append_progress(
                    &mut self.progress,
                    ProgressKind::Status,
                    json!({"state": WorkerState::Idle.as_str(), "error": error}),
                );
                host.settle(None);
            }
        }
    }

    /// One turn through the shell's own entry points, engine and adapter.
    fn take_turn(
        &mut self,
        host: &WorkerHost,
        job: TurnJob,
        cancel: &CancelToken,
        steering: &TurnSteering,
    ) -> Result<ChatTurnResult, String> {
        let repository = self.repository()?;
        let engine = crate::chat_engine(load_worker_config(&self.data_dir, Some(&repository))?);
        let mut adapter = (self.adapter_for)(&engine.config)?;
        let progress = RefCell::new(ProgressSink {
            writer: &mut self.progress,
            text: String::new(),
            last_flush: Instant::now(),
        });
        let mut on_token = |token: &str| progress.borrow_mut().token(token);
        let mut on_progress = |event: TurnProgress| progress.borrow_mut().event(host, event);
        let mut sink = TurnSink {
            on_token: &mut on_token,
            on_progress: &mut on_progress,
            cancel,
            steering: Some(steering),
        };
        let chat = &engine.chat_orchestrator;
        let result = match job {
            TurnJob::Message(text) => chat.ask_with_session_with_options(
                &repository,
                &text,
                &[],
                Some(&self.session_id),
                adapter.as_mut(),
                &mut sink,
                ChatTurnOptions::default(),
            ),
            TurnJob::Decide {
                proposal_id,
                kind: DecisionKind::Command,
                approved,
            } => chat.resume_after_command_decision(
                &proposal_id,
                approved,
                "desktop_user",
                adapter.as_mut(),
                &mut sink,
            ),
            TurnJob::Decide {
                proposal_id,
                kind: DecisionKind::Plan,
                approved,
            } => chat.resume_after_plan_decision(
                &proposal_id,
                approved,
                None,
                "desktop_user",
                adapter.as_mut(),
                &mut sink,
            ),
        };
        progress.borrow_mut().flush();
        result.map_err(|error| error.to_string())
    }

    /// The worktree this session works in, from its worker record. A session
    /// without one is not a worker's, and gets no turns here.
    fn repository(&mut self) -> Result<PathBuf, String> {
        if let Some(repository) = &self.repository {
            return Ok(repository.clone());
        }
        let record = workspace_engine::SessionStore::new(&self.data_dir)
            .worker_record(&self.session_id)
            .map_err(|_| "could not read this session's worker record".to_string())?
            .ok_or_else(|| "this session is not a worker session".to_string())?;
        self.repository = Some(record.worktree_path.clone());
        Ok(record.worktree_path)
    }
}

/// Starts the turn thread and attaches it to `host`, which then accepts
/// `message`. The thread gets the shell's stack, as every thread that runs a
/// turn does (Task 2).
pub(crate) fn start_turn_thread(host: &Arc<WorkerHost>, driver: TurnDriver) -> Result<(), String> {
    let (sender, receiver) = std::sync::mpsc::channel();
    *host
        .jobs
        .lock()
        .unwrap_or_else(|poison| poison.into_inner()) = Some(sender);
    let host = Arc::clone(host);
    crate::spawn_with_shell_stack("damaian-worker-turn", move || {
        driver.serve(&host, receiver);
    })
    .map(|_| ())
    .map_err(|error| format!("could not start the turn thread: {error}"))
}

/// A worker host with its turn thread, on this process: what `run_worker`
/// builds, without the handover, the lifeline or the socket.
#[cfg(test)]
pub(crate) fn host_in_process(
    session_id: &str,
    data_dir: &Path,
    adapter_for: Box<WorkerAdapterFor>,
) -> Result<Arc<WorkerHost>, String> {
    let host = Arc::new(WorkerHost::new(
        session_id.to_string(),
        Arc::new(ShutdownSignal::default()),
    ));
    start_turn_thread(&host, TurnDriver::new(session_id, data_dir, adapter_for)?)?;
    Ok(host)
}

/// The progress file is a view, and the audit log is the record (§5.5), so a
/// failed write does not stop the turn.
fn append_progress(writer: &mut ProgressWriter, kind: ProgressKind, payload: Value) {
    let _ = writer.append(kind, payload);
}

/// The turn's `TurnSink`, written to the progress file instead of SSE.
struct ProgressSink<'a> {
    writer: &'a mut ProgressWriter,
    text: String,
    last_flush: Instant,
}

impl ProgressSink<'_> {
    /// Model text is batched (§5.5): a line per token would be most of the
    /// file.
    fn token(&mut self, token: &str) {
        self.text.push_str(token);
        if self.last_flush.elapsed() >= TEXT_BATCH_INTERVAL {
            self.flush();
        }
    }

    fn flush(&mut self) {
        if !self.text.is_empty() {
            let text = std::mem::take(&mut self.text);
            append_progress(self.writer, ProgressKind::Text, json!({ "text": text }));
        }
        self.last_flush = Instant::now();
    }

    /// Text batched so far goes first, so the file keeps the turn's order.
    fn event(&mut self, host: &WorkerHost, event: TurnProgress) {
        self.flush();
        match event {
            TurnProgress::Phase(phase) => {
                append_progress(
                    self.writer,
                    ProgressKind::Phase,
                    parsed(&crate::phase_json(&phase)),
                );
                if phase.kind == PhaseKind::Tool {
                    append_progress(
                        self.writer,
                        ProgressKind::Tool,
                        json!({ "label": phase.label }),
                    );
                }
            }
            TurnProgress::Plan(plan) => {
                append_progress(
                    self.writer,
                    ProgressKind::Plan,
                    parsed(&crate::plan_json(&plan)),
                );
            }
            TurnProgress::Paused => {
                host.set_paused(true);
                append_progress(
                    self.writer,
                    ProgressKind::Status,
                    json!({
                        "state": WorkerState::Paused.as_str(),
                        "taskStatus": TaskStatus::Paused.as_str(),
                    }),
                );
            }
            TurnProgress::Resumed => {
                host.set_paused(false);
                append_progress(
                    self.writer,
                    ProgressKind::Status,
                    json!({"state": WorkerState::Running.as_str()}),
                );
            }
            TurnProgress::NoteAdded { message_id } => {
                append_progress(
                    self.writer,
                    ProgressKind::Note,
                    json!({ "messageId": message_id }),
                );
            }
            // The session is the worker's own, and diagnostics are in the
            // session log; neither is a board column.
            TurnProgress::Session(_) | TurnProgress::WebDiagnostic(_) => {}
        }
    }
}

/// The shell's SSE payloads are built as JSON text; the progress file takes
/// values.
fn parsed(json_text: &str) -> Value {
    serde_json::from_str(json_text).unwrap_or(Value::Null)
}

/// A command or plan approval keeps the worker waiting on the socket (§5.6).
/// A patch or a provider fallback offer ends the turn, and is answered through
/// the shell's own routes with the worktree as `repo`.
fn pending_decision(result: &ChatTurnResult) -> Option<(String, DecisionKind)> {
    if result.task.status != TaskStatus::WaitingForApproval {
        return None;
    }
    if let Some(plan) = &result.plan_proposal {
        return Some((plan.id.clone(), DecisionKind::Plan));
    }
    result
        .command_proposal
        .as_ref()
        .map(|command| (command.id.clone(), DecisionKind::Command))
}

/// The pending proposal, as the approval card needs it: the same JSON the
/// shell sends.
fn approval_payload(result: &ChatTurnResult) -> Option<Value> {
    let (kind, proposal) = if result.plan_proposal.is_some() {
        (
            "plan",
            crate::plan_proposal_json(result.plan_proposal.as_ref()),
        )
    } else if result.command_proposal.is_some() {
        ("command", crate::command_proposal_json(result))
    } else if result.patch_proposal.is_some() {
        ("patch", crate::patch_proposal_json(result))
    } else if result.fallback_proposal.is_some() {
        (
            "fallback",
            crate::fallback_proposal_json(result.fallback_proposal.as_ref()),
        )
    } else {
        return None;
    };
    Some(json!({ "kind": kind, "proposal": parsed(&proposal) }))
}

fn refusal(error: &str) -> Value {
    json!({"ok": false, "error": error})
}

/// Serves one connection at a time until `shutdown` is answered.
fn serve_control(listener: UnixListener, token: &str, host: &WorkerHost) {
    for stream in listener.incoming() {
        let Ok(stream) = stream else { continue };
        if serve_connection(stream, token, host) {
            return;
        }
    }
}

/// One connection, one request line at a time. Returns true once `shutdown`
/// has been answered.
fn serve_connection(stream: UnixStream, token: &str, host: &WorkerHost) -> bool {
    if stream
        .set_read_timeout(Some(CONNECTION_IDLE_TIMEOUT))
        .and_then(|()| stream.set_write_timeout(Some(CONNECTION_IDLE_TIMEOUT)))
        .is_err()
    {
        return false;
    }
    let Ok(writer) = stream.try_clone() else {
        return false;
    };
    let mut writer = writer;
    let mut reader = BufReader::new(stream);
    loop {
        let mut line = Vec::new();
        let read = (&mut reader)
            .take(MAX_REQUEST_BYTES + 1)
            .read_until(b'\n', &mut line);
        // EOF, an idle timeout, an oversized line or a torn one: close. None
        // of them can be answered without first knowing who is asking.
        match read {
            Ok(_) if line.last() == Some(&b'\n') && line.len() as u64 <= MAX_REQUEST_BYTES => {}
            _ => return false,
        }
        let (answer, close) = match parse_request(&line, token) {
            ParsedRequest::Unauthorized => return false,
            ParsedRequest::Invalid(error) => (refusal(error), false),
            ParsedRequest::Command(command) => host.dispatch(command),
        };
        let shutting_down = close && host.shutdown.is_triggered();
        if writer
            .write_all(format!("{answer}\n").as_bytes())
            .and_then(|()| writer.flush())
            .is_err()
            || close
        {
            return shutting_down;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;
    use std::os::unix::fs::MetadataExt;

    const TOKEN: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
    const KEY: &str = "sk-test-handover-key-not-real";
    const SESSION: &str = "session_18c1da785654f0c8_1712c_2";

    /// Short, because a socket path must fit in 104 bytes and `$TMPDIR` alone
    /// is about 50.
    fn scratch(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("dw-{}-{tag}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn handover_line(fields: &str) -> Vec<u8> {
        format!("{{{fields}}}\n").into_bytes()
    }

    #[test]
    fn worker_args_need_a_session_and_an_absolute_data_dir() {
        let args = |rest: &[&str]| -> Vec<String> {
            std::iter::once("damaian")
                .chain(rest.iter().copied())
                .map(str::to_string)
                .collect()
        };
        assert_eq!(
            parse_worker_args(&args(&[
                "--worker",
                "--session",
                SESSION,
                "--data-dir",
                "/d"
            ])),
            Ok(WorkerArgs {
                session_id: SESSION.to_string(),
                data_dir: PathBuf::from("/d"),
            })
        );
        for bad in [
            &["--worker", "--data-dir", "/d"][..],
            &["--worker", "--session", SESSION][..],
            &["--worker", "--session", SESSION, "--data-dir", "relative"][..],
            &["--worker", "--session", "../x", "--data-dir", "/d"][..],
            &[
                "--worker",
                "--session",
                SESSION,
                "--data-dir",
                "/d",
                "--extra",
            ][..],
            &["--session", SESSION, "--data-dir", "/d"][..],
        ] {
            assert!(parse_worker_args(&args(bad)).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn worker_mode_refuses_a_terminal_stdin() {
        let refusal = refuse_terminal_stdin(true).unwrap_err();
        assert!(refusal.contains("terminal"), "{refusal}");
        assert_eq!(refuse_terminal_stdin(false), Ok(()));
    }

    #[test]
    fn the_handover_line_parses_and_rejects_extra_fields() {
        let line = handover_line(&format!(r#""apiKey":"{KEY}","token":"{TOKEN}""#));
        let handover = read_handover(&mut Cursor::new(line)).unwrap();
        assert_eq!(handover.api_key, KEY);
        assert_eq!(handover.token, TOKEN);
        assert!(
            !format!("{handover:?}").contains(KEY),
            "Debug must not print the key"
        );

        let refused = [
            handover_line(&format!(
                r#""apiKey":"{KEY}","token":"{TOKEN}","extra":"x""#
            )),
            handover_line(&format!(r#""apiKey":"{KEY}""#)),
            handover_line(&format!(r#""token":"{TOKEN}""#)),
            handover_line(&format!(r#""apiKey":7,"token":"{TOKEN}""#)),
            handover_line(&format!(r#""apiKey":"{KEY}","token":"short""#)),
            handover_line(&format!(r#""apiKey":"{KEY}","token":"{}""#, "G".repeat(64))),
            format!("[\"{KEY}\"]\n").into_bytes(),
            format!("{{\"apiKey\":\"{KEY}\",\n").into_bytes(),
            // No newline: stdin closed before the line ended.
            format!(r#"{{"apiKey":"{KEY}","token":"{TOKEN}"}}"#).into_bytes(),
            Vec::new(),
            format!(
                "{{\"apiKey\":\"{}\",\"token\":\"{TOKEN}\"}}\n",
                "k".repeat(MAX_HANDOVER_BYTES as usize)
            )
            .into_bytes(),
        ];
        for line in refused {
            let error = read_handover(&mut Cursor::new(line.clone())).unwrap_err();
            assert!(
                !error.contains(KEY) && !error.contains(TOKEN),
                "a refusal must not echo the key or the token: {error}"
            );
        }
    }

    #[test]
    fn the_socket_is_named_by_a_short_hash_and_must_fit_sun_path() {
        let path = worker_socket_path(Path::new("/data"), SESSION).unwrap();
        let name = path.file_name().unwrap().to_str().unwrap();
        assert_eq!(path.parent(), Some(Path::new("/data/workers")));
        assert_eq!(name.len(), 16 + ".sock".len(), "{name}");
        assert!(!name.contains("session"), "{name}");
        assert_ne!(
            worker_socket_path(Path::new("/data"), "session_other").unwrap(),
            path
        );

        let deep = PathBuf::from("/").join("d".repeat(80));
        let error = worker_socket_path(&deep, SESSION).unwrap_err();
        assert!(error.contains("104"), "{error}");
    }

    #[test]
    fn the_worker_loads_config_from_its_data_dir() {
        let data_dir = scratch("cfg");
        fs::create_dir_all(data_dir.join("config")).unwrap();
        fs::write(
            data_dir.join("config").join("user.conf"),
            "audit_enabled=false\n",
        )
        .unwrap();
        let config = load_worker_config(&data_dir, None).unwrap();
        assert_eq!(config.data_dir, data_dir);
        assert!(
            !config.audit_enabled,
            "the user.conf under --data-dir was not read"
        );
    }

    #[test]
    fn the_socket_is_0600_in_a_0700_directory() {
        let data_dir = scratch("mode");
        let (_listener, path) = bind_control_socket(&data_dir, SESSION).unwrap();
        let socket = fs::symlink_metadata(&path).unwrap();
        assert!(socket.file_type().is_socket());
        assert_eq!(socket.mode() & 0o777, 0o600);
        let dir = fs::symlink_metadata(path.parent().unwrap()).unwrap();
        assert_eq!(dir.mode() & 0o777, 0o700);
    }

    #[test]
    fn a_stale_socket_is_replaced_but_a_live_one_is_not_taken_over() {
        let data_dir = scratch("stale");
        let (live, path) = bind_control_socket(&data_dir, SESSION).unwrap();
        let error = bind_control_socket(&data_dir, SESSION).unwrap_err();
        assert!(error.contains("already"), "{error}");
        drop(live);
        // The listener is gone but its file stays, as after a crash.
        assert!(path.exists());
        bind_control_socket(&data_dir, SESSION).expect("a stale socket is replaced");
    }

    #[test]
    fn a_symlinked_workers_directory_is_refused() {
        let data_dir = scratch("link");
        let elsewhere = scratch("link-to");
        std::os::unix::fs::symlink(&elsewhere, data_dir.join("workers")).unwrap();
        assert!(bind_control_socket(&data_dir, SESSION).is_err());
    }

    #[test]
    fn requests_are_validated_against_the_command_table() {
        let request = |body: &str| parse_request(format!("{{{body}}}").as_bytes(), TOKEN);
        let with_token = |body: &str| request(&format!(r#""token":"{TOKEN}",{body}"#));
        assert_eq!(
            with_token(r#""command":"status""#),
            ParsedRequest::Command(ControlCommand::Status)
        );
        assert_eq!(
            with_token(r#""command":"resume""#),
            ParsedRequest::Command(ControlCommand::Resume { note: None })
        );
        assert_eq!(
            with_token(r#""command":"resume","note":"n""#),
            ParsedRequest::Command(ControlCommand::Resume {
                note: Some("n".to_string())
            })
        );
        assert_eq!(
            with_token(r#""command":"approve","proposalId":"p1""#),
            ParsedRequest::Command(ControlCommand::Approve {
                proposal_id: "p1".to_string()
            })
        );
        assert_eq!(
            with_token(r#""command":"message","text":"go""#),
            ParsedRequest::Command(ControlCommand::Message {
                text: "go".to_string()
            })
        );
        for invalid in [
            r#""command":"status","text":"x""#,
            r#""command":"note""#,
            r#""command":"note","text":7"#,
            r#""command":"reject""#,
            r#""command":"message""#,
            r#""command":7"#,
        ] {
            assert!(
                matches!(with_token(invalid), ParsedRequest::Invalid(_)),
                "{invalid}"
            );
        }
        let long_note = format!(
            r#""command":"note","text":"{}""#,
            "n".repeat(MAX_NOTE_BYTES + 1)
        );
        assert!(matches!(with_token(&long_note), ParsedRequest::Invalid(_)));
        let long_resume = format!(
            r#""command":"resume","note":"{}""#,
            "n".repeat(MAX_NOTE_BYTES + 1)
        );
        assert!(matches!(
            with_token(&long_resume),
            ParsedRequest::Invalid(_)
        ));
    }

    #[test]
    fn an_unknown_command_is_rejected() {
        let line = format!(r#"{{"token":"{TOKEN}","command":"self-destruct"}}"#);
        assert_eq!(
            parse_request(line.as_bytes(), TOKEN),
            ParsedRequest::Invalid("unknown command")
        );

        let served = ServedWorker::start("unk");
        let mut connection = served.connect();
        let answer = connection.exchange(&line).expect("an answer, not a close");
        assert_eq!(answer["ok"], false);
        assert_eq!(answer["error"], "unknown command");
        // The connection stays usable: an unknown command is not an intruder.
        let status = connection
            .exchange(&format!(r#"{{"token":"{TOKEN}","command":"status"}}"#))
            .expect("status after an unknown command");
        assert_eq!(status["ok"], true);
    }

    #[test]
    fn a_wrong_or_missing_token_closes_the_connection() {
        let served = ServedWorker::start("tok");
        let wrong = TOKEN.replace('0', "1");
        for line in [
            format!(r#"{{"token":"{wrong}","command":"status"}}"#),
            r#"{"command":"status"}"#.to_string(),
            r#"{"token":7,"command":"status"}"#.to_string(),
            "not json".to_string(),
        ] {
            let mut connection = served.connect();
            assert_eq!(
                connection.exchange(&line),
                None,
                "an unauthorized request must be closed unanswered: {line}"
            );
        }
        // The server itself survives an intruder.
        let mut connection = served.connect();
        let status = connection
            .exchange(&format!(r#"{{"token":"{TOKEN}","command":"status"}}"#))
            .expect("status");
        assert_eq!(status["ok"], true);
    }

    #[test]
    fn status_answers_with_the_current_snapshot() {
        let served = ServedWorker::start("stat");
        let request = format!(r#"{{"token":"{TOKEN}","command":"status"}}"#);
        let status = served.connect().exchange(&request).unwrap();
        assert_eq!(
            status,
            json!({
                "ok": true,
                "sessionId": SESSION,
                "pid": std::process::id(),
                "state": "idle",
            })
        );
        served.host.set_state(WorkerState::ShuttingDown);
        let status = served.connect().exchange(&request).unwrap();
        assert_eq!(status["state"], "shutting_down");
        assert!(
            !status.to_string().contains(KEY),
            "the snapshot must never carry the key"
        );
    }

    #[test]
    fn turn_commands_are_refused_while_no_turn_is_hosted() {
        let served = ServedWorker::start("turn");
        let mut connection = served.connect();
        for body in [
            r#""command":"message","text":"go""#,
            r#""command":"pause""#,
            r#""command":"note","text":"n""#,
            r#""command":"stop""#,
        ] {
            let answer = connection
                .exchange(&format!(r#"{{"token":"{TOKEN}",{body}}}"#))
                .unwrap();
            assert_eq!(answer["ok"], false, "{body}");
        }
        assert!(!served.host.shutdown.is_triggered());
    }

    #[test]
    fn shutdown_answers_then_signals_the_main_thread() {
        let served = ServedWorker::start("down");
        let answer = served
            .connect()
            .exchange(&format!(r#"{{"token":"{TOKEN}","command":"shutdown"}}"#))
            .unwrap();
        assert_eq!(answer, json!({"ok": true}));
        // `wait` returns only once triggered; a hang here is the failure.
        served.host.shutdown.wait();
    }

    #[test]
    fn an_oversized_request_closes_the_connection() {
        let served = ServedWorker::start("big");
        let line = format!(
            r#"{{"token":"{TOKEN}","command":"message","text":"{}"}}"#,
            "m".repeat(MAX_REQUEST_BYTES as usize)
        );
        assert_eq!(served.connect().exchange(&line), None);
    }

    /// The control server on a thread against a temp data dir.
    struct ServedWorker {
        host: Arc<WorkerHost>,
        path: PathBuf,
    }

    impl ServedWorker {
        fn start(tag: &str) -> Self {
            let data_dir = scratch(tag);
            let (listener, path) = bind_control_socket(&data_dir, SESSION).unwrap();
            let host = Arc::new(WorkerHost::new(
                SESSION.to_string(),
                Arc::new(ShutdownSignal::default()),
            ));
            let served = Arc::clone(&host);
            std::thread::spawn(move || serve_control(listener, TOKEN, &served));
            Self { host, path }
        }

        fn connect(&self) -> Connection {
            let stream = UnixStream::connect(&self.path).unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(10)))
                .unwrap();
            Connection {
                reader: BufReader::new(stream.try_clone().unwrap()),
                writer: stream,
            }
        }
    }

    struct Connection {
        reader: BufReader<UnixStream>,
        writer: UnixStream,
    }

    impl Connection {
        /// One request, one response; `None` when the worker closed instead.
        fn exchange(&mut self, line: &str) -> Option<Value> {
            // A worker that closes mid-write can reset the connection; that is
            // a close too.
            if self
                .writer
                .write_all(format!("{line}\n").as_bytes())
                .is_err()
            {
                return None;
            }
            let mut answer = String::new();
            match self.reader.read_line(&mut answer) {
                Ok(0) | Err(_) => None,
                Ok(_) => Some(serde_json::from_str(&answer).expect("a JSON answer")),
            }
        }
    }
}
