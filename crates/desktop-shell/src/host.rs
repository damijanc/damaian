//! The plumbing every host process shares (spec 110 D1a), factored out of
//! spec 57's worker so a session host is built from the same parts.
//!
//! A host is this executable re-entered with a mode flag ([`HostKind`]). The
//! shell registers it with the process registry (spec 46) before handing it
//! anything, writes one handover line on its stdin (an API key and the control
//! token) and keeps stdin open as the lifeline: EOF means the shell let go or
//! died. The host serves a `0600` Unix socket in a `0700` directory, named by a
//! hash of its id, and every line on it carries the token. It shuts down on
//! lifeline EOF, a `shutdown` request, or a signal through `sweep_own`.
//!
//! Keys travel only on stdin. Nothing here puts one in the environment, argv,
//! a file, a log line or an error: every error is a fixed string.

use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::fs::{DirBuilderExt, FileTypeExt, PermissionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::{Condvar, Mutex};
use std::time::{Duration, Instant};

use serde_json::{Map, Value, json};
use workspace_engine::{
    AuditLog, Config, ProcessIdentity, ProcessKind, ProcessRegistry, RegistrationHandle,
    SecretScanner,
};

/// Room for `{"apiKey": …, "token": …}` with any real provider key.
pub(crate) const MAX_HANDOVER_BYTES: u64 = 16 * 1024;
/// macOS's `sockaddr_un.sun_path`, including the terminating NUL.
pub(crate) const SUN_PATH_CAPACITY: usize = 104;
/// The control token is 32 random bytes, written as hex.
pub(crate) const TOKEN_HEX_LEN: usize = 64;
/// The longest session id a host accepts.
const MAX_SESSION_ID_BYTES: usize = 128;
/// The longest one-line answer read from a host. `status` is a few hundred
/// bytes; the bound only stops a misbehaving host from making the shell
/// buffer forever.
const MAX_ANSWER_BYTES: u64 = 1024 * 1024;

/// Which host a process is. Each has its own flag and socket directory, so a
/// worker and a session host for the same id never share a socket.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum HostKind {
    /// Spec 57: a worker session in its own worktree.
    Worker,
    /// Spec 110: an ordinary session's work, in the session's own checkout.
    Session,
}

impl HostKind {
    pub(crate) fn flag(self) -> &'static str {
        match self {
            Self::Worker => crate::worker::WORKER_FLAG,
            Self::Session => crate::session_host::SESSION_HOST_FLAG,
        }
    }

    fn socket_dir(self) -> &'static str {
        match self {
            Self::Worker => "workers",
            Self::Session => "hosts",
        }
    }

    fn noun(self) -> &'static str {
        match self {
            Self::Worker => "worker",
            Self::Session => "session host",
        }
    }

    pub(crate) fn process_kind(self) -> ProcessKind {
        match self {
            Self::Worker => ProcessKind::Worker,
            Self::Session => ProcessKind::SessionHost,
        }
    }
}

/// Blocks until stdin reaches EOF or fails, handing each further line to
/// `on_line`. The shell holds the write end for the host's whole life, so
/// this returns only when the shell let go of it or died, `SIGKILL` included:
/// the kernel closes the pipe either way. A worker expects nothing after the
/// handover and discards it; a session host reads the keys the shell resolves
/// for each request (D1a).
pub(crate) fn watch_lifeline(stdin: impl Read, mut on_line: impl FnMut(&[u8])) {
    let mut reader = BufReader::new(stdin);
    loop {
        let mut line = Vec::new();
        match (&mut reader)
            .take(MAX_HANDOVER_BYTES + 1)
            .read_until(b'\n', &mut line)
        {
            Ok(0) => return,
            Ok(_) if line.last() == Some(&b'\n') && line.len() as u64 <= MAX_HANDOVER_BYTES => {
                line.pop();
                on_line(&line);
            }
            // An oversized or torn line is dropped; the pipe stays the lifeline.
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
            Err(_) => return,
        }
    }
}

/// Session ids are `create_id` output; anything else is not one of ours.
pub(crate) fn validate_session_id(session_id: &str) -> Result<(), String> {
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

/// Host mode cannot be run by hand: without the handover there is no key and
/// no token, and a terminal on stdin means nobody is holding the lifeline.
pub(crate) fn refuse_terminal_stdin(is_terminal: bool) -> Result<(), String> {
    if is_terminal {
        return Err(
            "worker mode reads its handover from the shell and cannot run from a terminal"
                .to_string(),
        );
    }
    Ok(())
}

/// What the shell writes on a host's stdin first. `Debug` never prints either
/// value.
pub(crate) struct Handover {
    pub(crate) api_key: String,
    pub(crate) token: String,
}

impl std::fmt::Debug for Handover {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("Handover { .. }")
    }
}

pub(crate) fn read_handover(reader: &mut impl BufRead) -> Result<Handover, String> {
    let mut line = Vec::new();
    reader
        .take(MAX_HANDOVER_BYTES + 1)
        .read_until(b'\n', &mut line)
        .map_err(|_| "could not read the handover from stdin".to_string())?;
    if line.len() as u64 > MAX_HANDOVER_BYTES {
        return Err("the handover line is too long".to_string());
    }
    // A line without its newline means stdin closed part way: the shell that
    // started this host is already gone.
    if line.pop() != Some(b'\n') {
        return Err("stdin closed before the handover line ended".to_string());
    }
    parse_handover(&line)
}

/// Every refusal is a fixed string. In particular serde's own messages are
/// never passed on, because some of them quote the input.
pub(crate) fn parse_handover(line: &[u8]) -> Result<Handover, String> {
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

/// `<data_dir>/<workers|hosts>/<16 hex of sha256(id)>.sock`.
///
/// Named by a hash because the session id does not fit: measured on
/// 2026-10-12, the id-named path is 107 bytes under the real Application
/// Support directory and 132 under a `/var/folders/…` test directory, against
/// `sun_path`'s 104. Refuses when even the hashed path does not fit.
pub(crate) fn host_socket_path(
    data_dir: &Path,
    kind: HostKind,
    id: &str,
) -> Result<PathBuf, String> {
    let digest = workspace_engine::hash::sha256(id);
    let path = data_dir
        .join(kind.socket_dir())
        .join(format!("{}.sock", &digest[..16]));
    let length = path.as_os_str().len();
    if length >= SUN_PATH_CAPACITY {
        return Err(format!(
            "the {} socket path is {length} bytes, and macOS allows {} \
             (sun_path is {SUN_PATH_CAPACITY} bytes with its NUL); use a shorter \
             data directory",
            kind.noun(),
            SUN_PATH_CAPACITY - 1
        ));
    }
    Ok(path)
}

/// Binds `0600` in a `0700` directory. The directory is private before the
/// socket exists, so nobody else can reach it between `bind` and `chmod`.
pub(crate) fn bind_host_socket(
    data_dir: &Path,
    kind: HostKind,
    id: &str,
) -> Result<(UnixListener, PathBuf), String> {
    let path = host_socket_path(data_dir, kind, id)?;
    let dir = path
        .parent()
        .ok_or_else(|| format!("the {} socket has no directory", kind.noun()))?;
    fs::create_dir_all(data_dir)
        .map_err(|error| format!("could not create the data directory: {error}"))?;
    match fs::symlink_metadata(dir) {
        Ok(meta) if meta.file_type().is_dir() => {}
        Ok(_) => return Err(format!("{} is not a directory", dir.display())),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            match fs::DirBuilder::new().mode(0o700).create(dir) {
                Ok(()) => {}
                // Two session hosts start together, and the other created it
                // first; it must still be a directory, not a link.
                Err(error)
                    if error.kind() == std::io::ErrorKind::AlreadyExists
                        && fs::symlink_metadata(dir)
                            .is_ok_and(|meta| meta.file_type().is_dir()) => {}
                Err(error) => return Err(format!("could not create {}: {error}", dir.display())),
            }
        }
        Err(error) => return Err(format!("could not inspect {}: {error}", dir.display())),
    }
    fs::set_permissions(dir, fs::Permissions::from_mode(0o700))
        .map_err(|error| format!("could not make {} private: {error}", dir.display()))?;

    match fs::symlink_metadata(&path) {
        Ok(meta) if meta.file_type().is_socket() => {
            // A socket someone still answers on belongs to a live host for
            // this id. One left by a crash refuses the connection.
            if UnixStream::connect(&path).is_ok() {
                return Err(format!("a {} is already serving this session", kind.noun()));
            }
            fs::remove_file(&path).map_err(|error| {
                format!("could not remove a stale {} socket: {error}", kind.noun())
            })?;
        }
        Ok(_) => return Err(format!("{} is not a socket", path.display())),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(format!("could not inspect {}: {error}", path.display())),
    }
    let listener = UnixListener::bind(&path)
        .map_err(|error| format!("could not bind the {} socket: {error}", kind.noun()))?;
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600))
        .map_err(|error| format!("could not make the {} socket private: {error}", kind.noun()))?;
    Ok((listener, path))
}

/// Compares every byte whatever the first mismatch, so the time an answer
/// takes says nothing about how much of a guess was right.
pub(crate) fn tokens_match(offered: &str, expected: &str) -> bool {
    offered.len() == expected.len()
        && offered
            .bytes()
            .zip(expected.bytes())
            .fold(0_u8, |difference, (a, b)| difference | (a ^ b))
            == 0
}

/// Set once, by the lifeline or by `shutdown`; the main thread waits on it.
#[derive(Debug, Default)]
pub(crate) struct ShutdownSignal {
    requested: Mutex<bool>,
    changed: Condvar,
}

impl ShutdownSignal {
    pub(crate) fn trigger(&self) {
        *self.lock() = true;
        self.changed.notify_all();
    }

    pub(crate) fn is_triggered(&self) -> bool {
        *self.lock()
    }

    pub(crate) fn wait(&self) {
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

/// 32 bytes from `/dev/urandom`, as hex.
pub(crate) fn control_token() -> Result<String, String> {
    let mut bytes = [0_u8; 32];
    std::fs::File::open("/dev/urandom")
        .and_then(|mut random| random.read_exact(&mut bytes))
        .map_err(|_| "could not read /dev/urandom for the host's token".to_string())?;
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

/// A child that failed part way through starting: its stdin is already
/// closed, so it ends by its lifeline; this only reaps it.
pub(crate) fn abandon(mut child: Child) {
    let _ = std::thread::Builder::new()
        .name("damaian-host-reap".to_string())
        .spawn(move || {
            let _ = child.wait();
        });
}

/// Whether the process `identity` names is gone within `within`.
pub(crate) fn wait_for_exit(identity: &ProcessIdentity, poll: Duration, within: Duration) -> bool {
    let deadline = Instant::now() + within;
    loop {
        if ProcessIdentity::of(identity.pid) != Some(*identity) {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(poll);
    }
}

/// The audit log `ProcessRegistry::open_with_audit` builds, so host events
/// land beside every other event.
pub(crate) fn host_audit_log(config: &Config) -> AuditLog {
    AuditLog::with_retention(
        &config.data_dir,
        config.audit_enabled,
        config.audit_retention_days,
        SecretScanner::new(config.secret_patterns.clone()),
    )
}

/// One request and its one-line answer. `None` covers every failure: no
/// socket, a closed connection, a timeout, an answer that is not JSON.
pub(crate) fn exchange(
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

/// A host process this shell started: the child, its lifeline, its registry
/// entry and what it takes to talk to it.
///
/// No `Debug`: it holds the control token.
pub(crate) struct StartedHost {
    pub(crate) child: Child,
    /// The lifeline. `None` once the shell has let go of it.
    pub(crate) stdin: Option<ChildStdin>,
    pub(crate) registration: RegistrationHandle,
    pub(crate) identity: ProcessIdentity,
    pub(crate) token: String,
    pub(crate) socket: PathBuf,
}

/// What a host is started with, beyond its kind.
pub(crate) struct HostLaunch<'a> {
    pub(crate) exe: &'a Path,
    pub(crate) kind: HostKind,
    /// The session id, or for a session host not yet bound to a session, a
    /// placeholder id of the same shape; names the socket and the registry
    /// entry.
    pub(crate) id: &'a str,
    /// Absolute: a host refuses a relative data directory.
    pub(crate) data_dir: &'a Path,
    pub(crate) current_dir: Option<&'a Path>,
    pub(crate) api_key: &'a str,
}

/// Starts `exe <flag> --session <id> --data-dir <dir>`, registers it, writes
/// the handover and keeps the lifeline. Registration comes before the
/// handover, so a host never holds a key without a registry entry the launch
/// sweep can find.
pub(crate) fn start_host(launch: HostLaunch<'_>) -> Result<StartedHost, String> {
    let noun = launch.kind.noun();
    let socket = host_socket_path(launch.data_dir, launch.kind, launch.id)?;
    let token = control_token()?;
    let registry = ProcessRegistry::open(launch.data_dir).map_err(|error| error.to_string())?;
    let mut command = Command::new(launch.exe);
    command
        .arg(launch.kind.flag())
        .arg("--session")
        .arg(launch.id)
        .arg("--data-dir")
        .arg(launch.data_dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::inherit())
        .process_group(0);
    if let Some(dir) = launch.current_dir {
        command.current_dir(dir);
    }
    let mut child = command
        .spawn()
        .map_err(|error| format!("could not start the {noun}: {error}"))?;
    let Some(mut stdin) = child.stdin.take() else {
        abandon(child);
        return Err(format!("could not open the {noun}'s stdin"));
    };
    let registration = match registry.register(launch.kind.process_kind(), launch.id, child.id()) {
        Ok(registration) => registration,
        Err(error) => {
            drop(stdin);
            abandon(child);
            return Err(format!("could not register the {noun}: {error}"));
        }
    };
    let Some(identity) = ProcessIdentity::of(child.id()) else {
        drop(stdin);
        abandon(child);
        return Err(format!("the {noun} exited as it started"));
    };
    let handover = json!({"apiKey": launch.api_key, "token": token}).to_string();
    if stdin
        .write_all(handover.as_bytes())
        .and_then(|()| stdin.write_all(b"\n"))
        .and_then(|()| stdin.flush())
        .is_err()
    {
        drop(stdin);
        abandon(child);
        return Err(format!("could not hand over to the {noun}: it exited"));
    }
    drop(handover);
    Ok(StartedHost {
        child,
        stdin: Some(stdin),
        registration,
        identity,
        token,
        socket,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_kind_has_its_own_socket_directory_and_flag() {
        let worker = host_socket_path(Path::new("/data"), HostKind::Worker, "s").unwrap();
        let session = host_socket_path(Path::new("/data"), HostKind::Session, "s").unwrap();
        assert_eq!(worker.parent(), Some(Path::new("/data/workers")));
        assert_eq!(session.parent(), Some(Path::new("/data/hosts")));
        assert_eq!(worker.file_name(), session.file_name());
        assert_ne!(HostKind::Worker.flag(), HostKind::Session.flag());
    }

    #[test]
    fn the_lifeline_hands_over_each_later_line_until_eof() {
        let input = b"first\n{\"x\":1}\ntorn".to_vec();
        let mut lines = Vec::new();
        watch_lifeline(std::io::Cursor::new(input), |line| {
            lines.push(String::from_utf8_lossy(line).to_string());
        });
        assert_eq!(lines, vec!["first", "{\"x\":1}"]);
    }
}
