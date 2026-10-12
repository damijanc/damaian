//! The real-process half of spec 57 Task 5: the `--worker` host started from
//! the real `damaian-desktop-shell` binary. Every test here spawns real
//! processes, so every test is `#[ignore]`d, per `AGENTS.md`.
//!
//! Cleanup goes through a #46 registry of this test's own (`Harness`), so a
//! failing test still kills what it started, by identity and never by name.

use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::net::UnixStream;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::time::{Duration, Instant};

use workspace_engine::{
    AuditLog, ProcessIdentity, ProcessKind, ProcessRegistry, RegisteredProcess, RegistrationHandle,
    SecretScanner,
};

const TOKEN: &str = "fedcba9876543210fedcba9876543210fedcba9876543210fedcba9876543210";
const KEY: &str = "sk-test-handover-key-not-real";
const SESSION: &str = "session_18c1da785654f0c8_1712c_2";

/// Short, because the worker's socket path must fit in 104 bytes.
fn scratch(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("dwp-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// This test's own registry, in its own directory, so its entries never mix
/// with the worker's. Dropping it sweeps everything the test registered: an
/// assertion that fails part way still leaves nothing running.
struct Harness {
    registry: ProcessRegistry,
    audit: AuditLog,
    handles: Vec<RegistrationHandle>,
}

impl Harness {
    fn new(dir: &Path) -> Self {
        let registry = ProcessRegistry::open(dir).unwrap();
        let audit = AuditLog::new(dir, true, SecretScanner::new(Vec::new()));
        Self {
            registry,
            audit,
            handles: Vec::new(),
        }
    }

    fn track(&mut self, kind: ProcessKind, pid: u32) {
        self.handles
            .push(self.registry.register(kind, SESSION, pid).unwrap());
    }
}

impl Drop for Harness {
    fn drop(&mut self) {
        let _ = self.registry.sweep_own(&self.audit);
    }
}

struct Worker {
    child: Child,
    stdin: Option<ChildStdin>,
    socket: PathBuf,
}

/// Starts the real binary in worker mode, the way Task 7's supervisor will:
/// its own process group, piped stdin, one handover line, stdin held open.
fn start_worker(data_dir: &Path, harness: &mut Harness) -> Worker {
    let mut child = Command::new(env!("CARGO_BIN_EXE_damaian-desktop-shell"))
        .args(["--worker", "--session", SESSION, "--data-dir"])
        .arg(data_dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0)
        .spawn()
        .expect("the worker should spawn");
    harness.track(ProcessKind::Worker, child.id());
    let mut stdin = child.stdin.take().unwrap();
    writeln!(stdin, r#"{{"apiKey":"{KEY}","token":"{TOKEN}"}}"#).unwrap();
    stdin.flush().unwrap();

    let socket = desktop_shell::worker_socket_path(data_dir, SESSION).unwrap();
    let deadline = Instant::now() + Duration::from_secs(30);
    while UnixStream::connect(&socket).is_err() {
        if let Ok(Some(status)) = child.try_wait() {
            panic!("the worker exited before serving: {status}");
        }
        assert!(
            Instant::now() < deadline,
            "the worker never served its socket"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    Worker {
        child,
        stdin: Some(stdin),
        socket,
    }
}

fn request(socket: &Path, body: &str) -> serde_json::Value {
    let stream = UnixStream::connect(socket).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    let mut writer = stream.try_clone().unwrap();
    writeln!(writer, r#"{{"token":"{TOKEN}",{body}}}"#).unwrap();
    let mut answer = String::new();
    BufReader::new(stream).read_line(&mut answer).unwrap();
    serde_json::from_str(&answer).unwrap()
}

fn wait_for_exit(child: &mut Child, within: Duration) -> std::process::ExitStatus {
    let deadline = Instant::now() + within;
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            return status;
        }
        assert!(
            Instant::now() < deadline,
            "the process did not exit in time"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// Requirement 8, the lifeline: closing the worker's stdin — which is what the
/// kernel does when the shell dies, `SIGKILL` included — makes the worker
/// sweep the processes it owns and exit.
///
/// The worker runs no commands until Task 6, so the test registers a silent
/// `sleep` **as the worker's**: an entry in the worker's registry naming the
/// worker as owner, which is exactly what `command_runner` writes for a
/// command the worker runs. Only the worker's `sweep_own` can then kill it.
///
/// `#[ignore]`d per `AGENTS.md` because it spawns and kills real processes.
/// Run it by hand:
///
/// ```sh
/// CARGO_TARGET_DIR="$PWD/target" cargo test -p desktop-shell --test worker_process -- \
///   --ignored --exact closing_stdin_exits_the_worker_and_kills_its_sleeping_child
/// ```
#[test]
#[ignore]
fn closing_stdin_exits_the_worker_and_kills_its_sleeping_child() {
    let root = scratch("life");
    let data_dir = root.join("data");
    let mut harness = Harness::new(&root.join("harness"));
    let mut worker = start_worker(&data_dir, &mut harness);

    // Silent, so it cannot die of `SIGPIPE` and pass the test by accident.
    let mut sleeper = Command::new("/bin/sleep")
        .arg("300")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .process_group(0)
        .spawn()
        .unwrap();
    harness.track(ProcessKind::Command, sleeper.id());
    let identity = ProcessIdentity::of(sleeper.id()).unwrap();
    let owner = ProcessIdentity::of(worker.child.id()).unwrap();
    let entry = RegisteredProcess {
        pid: identity.pid,
        start_time_us: identity.start_time_us,
        pgid: identity.pid,
        kind: ProcessKind::Command.as_str().to_string(),
        session_id: SESSION.to_string(),
        registered_at_ms: 0,
        owner_pid: owner.pid,
        owner_start_time_us: owner.start_time_us,
    };
    std::fs::write(
        data_dir.join("processes").join(entry.file_name()),
        serde_json::to_string(&entry).unwrap(),
    )
    .unwrap();

    assert_eq!(request(&worker.socket, r#""command":"status""#)["ok"], true);
    drop(worker.stdin.take());

    let status = wait_for_exit(&mut worker.child, Duration::from_secs(10));
    assert!(status.success(), "the worker should exit cleanly: {status}");
    let sleeper_status = wait_for_exit(&mut sleeper, Duration::from_secs(10));
    assert!(
        !sleeper_status.success(),
        "the worker's child must be killed, not left to finish: {sleeper_status}"
    );
    assert!(
        !worker.socket.exists(),
        "a clean exit removes the control socket"
    );
}

/// Requirement 9: the key and the token reach the worker on stdin only. Its
/// argv (read back with `ps`) and everything it prints carry neither.
///
/// The child-environment half named in `tasks.md` moves to Task 6: the worker
/// runs no command until then, and macOS does not show another process's
/// environment (`ps -E` prints none), so there is no child to read it from.
///
/// `#[ignore]`d per `AGENTS.md` because it spawns real processes. Run it by
/// hand:
///
/// ```sh
/// CARGO_TARGET_DIR="$PWD/target" cargo test -p desktop-shell --test worker_process -- \
///   --ignored --exact the_worker_keeps_key_and_token_out_of_argv_and_output
/// ```
#[test]
#[ignore]
fn the_worker_keeps_key_and_token_out_of_argv_and_output() {
    let root = scratch("argv");
    let data_dir = root.join("data");
    let mut harness = Harness::new(&root.join("harness"));
    let mut worker = start_worker(&data_dir, &mut harness);

    let ps = Command::new("/bin/ps")
        .args(["-ww", "-o", "args=", "-p", &worker.child.id().to_string()])
        .output()
        .unwrap();
    let argv = String::from_utf8_lossy(&ps.stdout);
    assert!(
        argv.contains("--worker"),
        "ps should show the worker's argv"
    );
    assert!(!argv.contains(KEY), "the key is in the worker's argv");
    assert!(!argv.contains(TOKEN), "the token is in the worker's argv");

    let shutdown = request(&worker.socket, r#""command":"shutdown""#);
    assert_eq!(shutdown, serde_json::json!({"ok": true}));
    let status = wait_for_exit(&mut worker.child, Duration::from_secs(10));
    assert!(status.success(), "{status}");

    let mut output = String::new();
    worker
        .child
        .stdout
        .take()
        .unwrap()
        .read_to_string(&mut output)
        .unwrap();
    worker
        .child
        .stderr
        .take()
        .unwrap()
        .read_to_string(&mut output)
        .unwrap();
    assert!(!output.contains(KEY), "the worker printed the key");
    assert!(!output.contains(TOKEN), "the worker printed the token");
    let on_disk = Command::new("/usr/bin/grep")
        .args(["-rl", KEY])
        .arg(&data_dir)
        .output()
        .unwrap();
    assert!(
        on_disk.stdout.is_empty(),
        "the key reached the data directory"
    );
}

/// A worker that is started without its handover refuses instead of serving.
///
/// `#[ignore]`d per `AGENTS.md` because it spawns a real process. Run it by
/// hand:
///
/// ```sh
/// CARGO_TARGET_DIR="$PWD/target" cargo test -p desktop-shell --test worker_process -- \
///   --ignored --exact a_worker_without_a_handover_exits_without_serving
/// ```
#[test]
#[ignore]
fn a_worker_without_a_handover_exits_without_serving() {
    let root = scratch("none");
    let data_dir = root.join("data");
    let mut harness = Harness::new(&root.join("harness"));
    let mut child = Command::new(env!("CARGO_BIN_EXE_damaian-desktop-shell"))
        .args(["--worker", "--session", SESSION, "--data-dir"])
        .arg(&data_dir)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .process_group(0)
        .spawn()
        .unwrap();
    harness.track(ProcessKind::Worker, child.id());
    let status = wait_for_exit(&mut child, Duration::from_secs(10));
    assert!(!status.success(), "{status}");
    let socket = desktop_shell::worker_socket_path(&data_dir, SESSION).unwrap();
    assert!(!socket.exists());
}
