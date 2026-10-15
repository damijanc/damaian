//! The real-process half of spec 57 Tasks 5 and 6: the `--worker` host started
//! from the real `damaian-desktop-shell` binary. Every test here spawns real
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
    SecretScanner, SessionStore, WorkerRecord,
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
    start_worker_for(data_dir, harness, SESSION, KEY, &[])
}

/// [`start_worker`] for a given session, with extra environment. The worker's
/// environment is the test's, as it is the shell's in the app.
fn start_worker_for(
    data_dir: &Path,
    harness: &mut Harness,
    session: &str,
    key: &str,
    env: &[(&str, &str)],
) -> Worker {
    let mut child = Command::new(env!("CARGO_BIN_EXE_damaian-desktop-shell"))
        .args(["--worker", "--session", session, "--data-dir"])
        .arg(data_dir)
        .envs(env.iter().copied())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0)
        .spawn()
        .expect("the worker should spawn");
    harness.track(ProcessKind::Worker, child.id());
    let mut stdin = child.stdin.take().unwrap();
    writeln!(stdin, r#"{{"apiKey":"{key}","token":"{TOKEN}"}}"#).unwrap();
    stdin.flush().unwrap();

    let socket = desktop_shell::worker_socket_path(data_dir, session).unwrap();
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
/// So that no turn is needed, the test registers a silent `sleep` **as the
/// worker's**: an entry in the worker's registry naming the
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
/// The child-environment half is
/// `a_command_the_worker_runs_sees_neither_key_nor_token`, below: macOS does
/// not show another process's environment (`ps -E` prints none), so it is read
/// from a command a turn runs. That test also found that an `sk-…` value is
/// redacted on its way into the data directory, so the data-directory search
/// here would not see this key if it leaked; argv and output are not redacted.
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

/// A committed one-file repository, standing in for the worktree, and a
/// worker session for it in `data_dir`, as Task 7's spawn will record it.
fn worker_session(root: &Path, data_dir: &Path) -> (PathBuf, String) {
    let repo = root.join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    std::fs::write(repo.join("README.md"), "# Worker fixture\n").unwrap();
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
            .current_dir(&repo)
            .status()
            .unwrap();
        assert!(status.success(), "git {args:?}");
    }
    let repo = std::fs::canonicalize(&repo).unwrap();
    let store = SessionStore::new(data_dir);
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
                source_repository: repo.clone(),
                worktree_path: repo.clone(),
                branch: "damaian/fixture".to_string(),
                base_commit: "HEAD".to_string(),
                included_uncommitted: false,
                git_commands: Vec::new(),
            },
        )
        .unwrap();
    (repo, session.id)
}

/// A model that asks for one command, through the `DAMAIAN_COMMAND_V1`
/// envelope, on every call.
fn command_response(command: &str) -> String {
    format!("DAMAIAN_COMMAND_V1\nCOMMAND: {command}\nREASON: Fixture.\nEND_COMMAND")
}

/// Polls `status` until the worker holds an approval, and returns its id.
fn wait_for_approval(socket: &Path) -> String {
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        let status = request(socket, r#""command":"status""#);
        if status["state"] == "waiting_for_approval" {
            return status["proposalId"].as_str().unwrap().to_string();
        }
        assert!(
            Instant::now() < deadline,
            "the worker never asked for approval: {status}"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// Whether any file under `dir` contains `needle`.
fn data_dir_contains(dir: &Path, needle: &str) -> bool {
    let found = Command::new("/usr/bin/grep")
        .args(["-rlF", needle])
        .arg(dir)
        .output()
        .unwrap();
    !found.stdout.is_empty()
}

/// Requirement 9, the child half moved here from Task 5: a command a worker
/// turn runs inherits the worker's environment, and neither the key nor the
/// control token is in it. The turn runs `env` (through
/// `DAMAIAN_MOCK_MODEL_RESPONSE`), and its output, which the session log keeps
/// as written, is searched for both.
///
/// Probes make the search mean something: a marker that proves the output was
/// recorded, and values shaped like the key and the token, which prove a value
/// of that shape survives into the data directory rather than being redacted
/// before the search could see it. An `sk-…` key is redacted (the first run of
/// this test found that), so this test hands over a key the scanner does not
/// recognise: a leak of it would be written as is.
///
/// `#[ignore]`d per `AGENTS.md` because it spawns real processes. Run it by
/// hand:
///
/// ```sh
/// CARGO_TARGET_DIR="$PWD/target" cargo test -p desktop-shell --test worker_process -- \
///   --ignored --exact a_command_the_worker_runs_sees_neither_key_nor_token
/// ```
#[test]
#[ignore]
fn a_command_the_worker_runs_sees_neither_key_nor_token() {
    const MARKER: &str = "dwp-env-probe-marker";
    const PLAIN_KEY: &str = "dwp-handover-key-in-plain-words";
    const KEY_SHAPED: &str = "dwp-handover-probe-in-plain-words";
    const TOKEN_SHAPED: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
    let root = scratch("env");
    let data_dir = root.join("data");
    let mut harness = Harness::new(&root.join("harness"));
    let (_repo, session) = worker_session(&root, &data_dir);
    let response = command_response("env");
    let mut worker = start_worker_for(
        &data_dir,
        &mut harness,
        &session,
        PLAIN_KEY,
        &[
            ("DAMAIAN_MOCK_MODEL_RESPONSE", response.as_str()),
            ("DWP_ENV_PROBE", MARKER),
            ("DWP_KEY_SHAPED", KEY_SHAPED),
            ("DWP_TOKEN_SHAPED", TOKEN_SHAPED),
        ],
    );

    let started = request(
        &worker.socket,
        r#""command":"message","text":"Show the environment.""#,
    );
    assert_eq!(started["ok"], true, "{started}");
    // The mock asks for `env` on every call; the first approval runs it.
    let deadline = Instant::now() + Duration::from_secs(60);
    while !data_dir_contains(&data_dir, MARKER) {
        let status = request(&worker.socket, r#""command":"status""#);
        if status["state"] == "waiting_for_approval" {
            let proposal = status["proposalId"].as_str().unwrap();
            let approved = request(
                &worker.socket,
                &format!(r#""command":"approve","proposalId":"{proposal}""#),
            );
            assert_eq!(approved["ok"], true, "{approved}");
        }
        assert!(
            Instant::now() < deadline,
            "the command's output never reached the data directory: {status}"
        );
        std::thread::sleep(Duration::from_millis(50));
    }

    assert!(
        data_dir_contains(&data_dir, KEY_SHAPED),
        "a key-shaped value was redacted from the output, so the key search below proves nothing"
    );
    assert!(
        data_dir_contains(&data_dir, TOKEN_SHAPED),
        "a token-shaped value was redacted from the output, so the token search below proves nothing"
    );
    assert!(
        !data_dir_contains(&data_dir, PLAIN_KEY),
        "the key reached a command's environment"
    );
    assert!(
        !data_dir_contains(&data_dir, TOKEN),
        "the control token reached a command's environment"
    );

    assert_eq!(
        request(&worker.socket, r#""command":"shutdown""#)["ok"],
        true
    );
    let status = wait_for_exit(&mut worker.child, Duration::from_secs(10));
    assert!(status.success(), "{status}");
}

/// §5.6 and requirement 8: a worker killed while it waits for a command
/// approval leaves the pending snapshot in place, and #17's recovery finds the
/// approval on the next start rather than losing the turn.
///
/// The kill is `SIGKILL` to this test's own unreaped child, after checking
/// that the pid still names the process the test started.
///
/// `#[ignore]`d per `AGENTS.md` because it spawns and `SIGKILL`s a real
/// process. Run it by hand:
///
/// ```sh
/// CARGO_TARGET_DIR="$PWD/target" cargo test -p desktop-shell --test worker_process -- \
///   --ignored --exact killing_a_worker_waiting_for_approval_leaves_a_snapshot_recovery_finds
/// ```
#[test]
#[ignore]
fn killing_a_worker_waiting_for_approval_leaves_a_snapshot_recovery_finds() {
    let root = scratch("kill");
    let data_dir = root.join("data");
    let mut harness = Harness::new(&root.join("harness"));
    let (repo, session) = worker_session(&root, &data_dir);
    let response = command_response("touch approved.txt");
    let mut worker = start_worker_for(
        &data_dir,
        &mut harness,
        &session,
        KEY,
        &[("DAMAIAN_MOCK_MODEL_RESPONSE", response.as_str())],
    );
    let identity = ProcessIdentity::of(worker.child.id()).unwrap();

    let started = request(
        &worker.socket,
        r#""command":"message","text":"Mark the tree.""#,
    );
    assert_eq!(started["ok"], true, "{started}");
    let proposal_id = wait_for_approval(&worker.socket);
    let snapshot = data_dir
        .join("chat")
        .join("pending")
        .join(format!("{proposal_id}.json"));
    assert!(snapshot.is_file(), "the turn stopped on its snapshot");

    assert_eq!(
        ProcessIdentity::of(worker.child.id()),
        Some(identity),
        "the pid must still name the worker this test started"
    );
    worker.child.kill().expect("SIGKILL should be delivered");
    let status = wait_for_exit(&mut worker.child, Duration::from_secs(10));
    assert!(!status.success(), "{status}");

    assert!(snapshot.is_file(), "the snapshot outlives the worker");
    assert!(
        !repo.join("approved.txt").exists(),
        "nothing ran unapproved"
    );
    let audit = AuditLog::new(&data_dir, true, SecretScanner::new(Vec::new()));
    let reattached = workspace_engine::reattach_pending_approvals(
        &SessionStore::new(&data_dir),
        &audit,
        &workspace_engine::CommandStore::new(&data_dir),
        &workspace_engine::PatchStore::new(&data_dir),
        &workspace_engine::PausedTurns::new(&data_dir),
        &session,
    )
    .unwrap();
    assert!(
        matches!(
            reattached.as_slice(),
            [workspace_engine::ReattachedApproval::Command { .. }]
        ),
        "recovery should reattach the command approval: {reattached:?}"
    );
}
