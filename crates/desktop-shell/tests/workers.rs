//! The real-process half of spec 57 Task 7: the shell-side supervisor
//! (`desktop_shell::workers`) starting the real `--worker` binary, signalling a
//! stub by its registry identity, and the standalone shell's signal handler
//! reaching a worker. Every test here starts or signals real processes, so
//! every test is `#[ignore]`d, per `AGENTS.md`.
//!
//! Nothing here kills by name. A worker the supervisor started is ended by the
//! supervisor (lifeline, `shutdown`, or `SIGTERM` after an identity check); the
//! one helper process this file starts is the test's own unreaped child.

use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use desktop_shell::workers::{SpawnRequest, Supervisor, WorkerTimeouts};
use workspace_engine::worker_progress::read_progress_after;
use workspace_engine::{Config, ProcessIdentity, ProcessRegistry};

const KEY: &str = "plain-worker-handover-key";

/// Holds stdin like a worker, ignores everything else, and has no socket: the
/// stub for a worker that answers neither `stop` nor `shutdown`.
const DEAF_STUB: &str = "#!/bin/sh\nexec /bin/cat > /dev/null\n";

/// Short, because the worker's socket path must fit in 104 bytes.
fn scratch(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("dwk-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn git(dir: &Path, args: &[&str]) {
    let status = Command::new("git")
        .args(["-c", "user.name=Fixture", "-c", "user.email=f@example.test"])
        .args(args)
        .current_dir(dir)
        .stdout(Stdio::null())
        .status()
        .unwrap();
    assert!(status.success(), "git {args:?}");
}

fn repository(root: &Path) -> PathBuf {
    let repo = root.join("r");
    std::fs::create_dir_all(&repo).unwrap();
    std::fs::write(repo.join("README.md"), "# Fixture\n").unwrap();
    git(&repo, &["init", "-q"]);
    git(&repo, &["add", "."]);
    git(&repo, &["commit", "-qm", "init"]);
    std::fs::canonicalize(&repo).unwrap()
}

fn deaf_stub(root: &Path) -> PathBuf {
    let stub = root.join("deaf-worker");
    std::fs::write(&stub, DEAF_STUB).unwrap();
    std::fs::set_permissions(&stub, std::fs::Permissions::from_mode(0o755)).unwrap();
    stub
}

fn config(root: &Path) -> Config {
    Config {
        data_dir: root.join("d"),
        enable_worker_sessions: true,
        enable_index_watcher: false,
        ..Config::default()
    }
}

fn spawn(supervisor: &Supervisor, config: &Config, repo: &Path, prompt: &str) -> String {
    supervisor
        .spawn(
            config,
            SpawnRequest {
                repo: repo.to_path_buf(),
                prompt: prompt.to_string(),
                include_uncommitted: false,
                parent_session_id: None,
            },
            KEY,
        )
        .expect("the worker should spawn")
        .session_id
}

fn wait_until(what: &str, within: Duration, done: impl Fn() -> bool) {
    let deadline = Instant::now() + within;
    while !done() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn audit_events(data_dir: &Path, event_type: &str) -> Vec<serde_json::Value> {
    std::fs::read_to_string(data_dir.join("audit").join("events.jsonl"))
        .unwrap_or_default()
        .lines()
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
        .filter(|event| event["eventType"] == event_type)
        .collect()
}

/// The whole path through the real binary: the supervisor spawns the real
/// `--worker`, hands over key and token, sends the prompt as the first
/// message, and the worker runs the turn (scripted by
/// `DAMAIAN_MOCK_MODEL_RESPONSE`, which reaches it through the inherited
/// environment, as any variable of the shell's does). `status` goes through
/// the supervisor's socket client, and `shutdown_all` ends the worker without
/// a signal.
///
/// `#[ignore]`d per `AGENTS.md` because it spawns real processes. Run it by
/// hand:
///
/// ```sh
/// CARGO_TARGET_DIR="$PWD/target" cargo test -p desktop-shell --test workers -- \
///   --ignored --exact a_spawned_worker_runs_its_prompt_and_ends_with_shutdown_all
/// ```
#[test]
#[ignore]
fn a_spawned_worker_runs_its_prompt_and_ends_with_shutdown_all() {
    // SAFETY: set before any thread of this test reads the environment; the
    // other tests in this file never read it.
    unsafe { std::env::set_var("DAMAIAN_MOCK_MODEL_RESPONSE", "Done by the worker.") };
    let root = scratch("real");
    let repo = repository(&root);
    let config = config(&root);
    let supervisor = Supervisor::new(
        PathBuf::from(env!("CARGO_BIN_EXE_damaian-desktop-shell")),
        WorkerTimeouts::default(),
    );
    let session = spawn(&supervisor, &config, &repo, "Say hello");
    let pid = supervisor.pid(&session).unwrap();

    wait_until(
        "the first turn to complete",
        Duration::from_secs(60),
        || {
            read_progress_after(&config.data_dir, &session, 0)
                .unwrap_or_default()
                .iter()
                .any(|event| {
                    event["kind"] == "status" && event["payload"]["taskStatus"] == "complete"
                })
        },
    );
    let status = supervisor
        .control(&session, "status", serde_json::Map::new())
        .unwrap();
    assert_eq!(status["ok"], true, "{status}");
    assert_eq!(status["state"], "idle");
    assert_eq!(status["pid"], pid, "the process the supervisor started");
    assert_eq!(status["sessionId"], session.as_str());

    let ps = Command::new("/bin/ps")
        .args(["-ww", "-o", "args=", "-p", &pid.to_string()])
        .output()
        .unwrap();
    let argv = String::from_utf8_lossy(&ps.stdout);
    assert!(argv.contains("--worker"), "{argv}");
    assert!(!argv.contains(KEY), "the key is in the worker's argv");

    let socket = desktop_shell::worker_socket_path(&config.data_dir, &session).unwrap();
    supervisor.shutdown_all();
    assert!(
        ProcessIdentity::of(pid).is_none(),
        "the worker outlived shutdown_all"
    );
    assert!(!socket.exists(), "a clean exit removes the control socket");
    assert!(
        audit_events(&config.data_dir, "worker_terminated").is_empty(),
        "no signal was needed"
    );
}

/// §5.3's stop escalation against a worker that answers nothing: no `stop`, no
/// `shutdown`, stdin held. With the timeouts injected short, the supervisor
/// `SIGTERM`s it by its registry identity, audits that, and nothing else could
/// have ended it: its lifeline is still held.
///
/// `#[ignore]`d per `AGENTS.md` because it deliberately signals a process.
/// Run it by hand:
///
/// ```sh
/// CARGO_TARGET_DIR="$PWD/target" cargo test -p desktop-shell --test workers -- \
///   --ignored --exact stop_escalates_to_shutdown_then_sigterm_by_registry_pid
/// ```
#[test]
#[ignore]
fn stop_escalates_to_shutdown_then_sigterm_by_registry_pid() {
    let root = scratch("stop");
    let repo = repository(&root);
    let config = config(&root);
    let timeouts = WorkerTimeouts {
        ready: Duration::from_millis(200),
        exchange: Duration::from_millis(200),
        stop_to_shutdown: Duration::from_millis(500),
        shutdown_to_sigterm: Duration::from_millis(500),
        poll: Duration::from_millis(20),
    };
    let supervisor = Supervisor::new(deaf_stub(&root), timeouts);
    let session = spawn(&supervisor, &config, &repo, "Ignore everything");
    let pid = supervisor.pid(&session).unwrap();
    let registered = ProcessRegistry::open(&config.data_dir)
        .unwrap()
        .entries()
        .unwrap()
        .into_iter()
        .filter_map(|(_, entry)| entry)
        .find(|entry| entry.session_id == session)
        .expect("the worker is registered");
    assert_eq!(registered.pid, pid);

    let started = Instant::now();
    let answer = supervisor.stop(&session).unwrap();
    assert_eq!(answer["ok"], false, "nothing answered: {answer}");
    wait_until("the stub to be signalled", Duration::from_secs(10), || {
        !supervisor.is_alive(&session)
    });
    assert!(
        started.elapsed() >= timeouts.shutdown_to_sigterm,
        "signalled only after shutdown had its time: {:?}",
        started.elapsed()
    );
    let terminated = audit_events(&config.data_dir, "worker_terminated");
    assert_eq!(terminated.len(), 1, "{terminated:?}");
    assert_eq!(terminated[0]["pid"], pid.to_string());
    assert_eq!(
        terminated[0]["startTimeUs"],
        registered.start_time_us.to_string(),
        "the registry's identity, not a bare number"
    );
    assert_eq!(terminated[0]["sessionId"], session.as_str());
    assert!(ProcessIdentity::of(pid).is_none());
}

/// The standalone shell's `main` installs #46's handler over its own registry
/// (`desktop-shell/src/main.rs`, `install_shutdown_handler`). C6 says that
/// reaches workers because the shell registers them as its own. This confirms
/// it: a helper process installs the handler exactly as `main` does, spawns a
/// worker through the supervisor, and is `SIGTERM`ed. The handler's
/// `sweep_own` must record `orphan_process_killed` for the worker.
///
/// The worker would also exit through its lifeline when the helper dies, so
/// the audit record, which only `sweep_own` writes, is what tells the two
/// apart.
///
/// `#[ignore]`d per `AGENTS.md` because it signals a process. Run it by hand:
///
/// ```sh
/// CARGO_TARGET_DIR="$PWD/target" cargo test -p desktop-shell --test workers -- \
///   --ignored --exact the_shells_signal_handler_reaches_workers_through_sweep_own
/// ```
#[test]
#[ignore]
fn the_shells_signal_handler_reaches_workers_through_sweep_own() {
    let root = scratch("sig");
    let mut helper = Command::new(std::env::current_exe().unwrap())
        .args([
            "--ignored",
            "--exact",
            "signal_helper_spawns_a_worker_and_waits",
        ])
        .env("DAMAIAN_WORKERS_HELPER_DIR", &root)
        .stdout(Stdio::null())
        .spawn()
        .expect("the helper should start");

    let pid_file = root.join("worker.pid");
    wait_until("the helper's worker", Duration::from_secs(30), || {
        std::fs::read_to_string(&pid_file).is_ok_and(|text| text.ends_with('\n'))
    });
    let worker_pid: u32 = std::fs::read_to_string(&pid_file)
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    assert!(ProcessIdentity::of(worker_pid).is_some());

    // Our own unreaped child, so its pid cannot have been reused.
    let status = Command::new("/bin/kill")
        .args(["-TERM", &helper.id().to_string()])
        .status()
        .unwrap();
    assert!(status.success());
    let exit = helper.wait().unwrap();
    assert!(!exit.success(), "the handler re-raises SIGTERM: {exit}");

    wait_until("the worker to be gone", Duration::from_secs(5), || {
        ProcessIdentity::of(worker_pid).is_none()
    });
    // Audit fields are strings.
    let pid_field = worker_pid.to_string();
    let killed = audit_events(&root.join("d"), "orphan_process_killed");
    assert!(
        killed.iter().any(|event| event["kind"] == "worker"
            && event["pid"].as_str() == Some(pid_field.as_str())
            && event["scope"] == "OwnProcess"),
        "sweep_own must reach the worker: {killed:?}"
    );
}

/// Requirement 8, proposal §6's end-to-end kill test: a shell process with a
/// real worker running a long command is `SIGKILL`ed, so no cleanup code of
/// the shell's runs, and the worker and its command are gone within 5
/// seconds. Only the worker's lifeline (§5.3 step 1) can end them: the kernel
/// closes the dead shell's end of the pipe, the worker reads EOF, and its own
/// `sweep_own` ends the command it registered.
///
/// The "shell" is a helper process that does what the standalone shell's
/// `main` does with a worker (the handler over its registry, a `Supervisor`
/// over the real binary) and approves the command when asked. The real
/// shell's API token arrives only over Tauri IPC, so a test cannot drive it.
/// The helper is this test's own unreaped child, so its pid cannot have been
/// reused when it is killed. If the worker or its command outlives the
/// assertion, a guard `SIGTERM`s them by their registered identity; nothing
/// is killed by name.
///
/// `#[ignore]`d per `AGENTS.md` because it spawns and `SIGKILL`s real
/// processes. Run it by hand:
///
/// ```sh
/// CARGO_TARGET_DIR="$PWD/target" cargo test -p desktop-shell --test workers -- \
///   --ignored --exact sigkilling_the_shell_ends_its_worker_and_the_workers_command
/// ```
#[test]
#[ignore]
fn sigkilling_the_shell_ends_its_worker_and_the_workers_command() {
    /// `SIGTERM`s whatever still has its registered identity when dropped, so
    /// a failing run leaves nothing behind.
    struct Leftovers(Vec<ProcessIdentity>);
    impl Drop for Leftovers {
        fn drop(&mut self) {
            for identity in &self.0 {
                identity.send_sigterm();
            }
        }
    }

    let root = scratch("kill");
    let mut shell = Command::new(std::env::current_exe().unwrap())
        .args([
            "--ignored",
            "--exact",
            "kill_helper_runs_a_worker_command_and_waits",
        ])
        .env("DAMAIAN_WORKERS_KILL_DIR", &root)
        .stdout(Stdio::null())
        .spawn()
        .expect("the shell helper should start");
    let shell_identity = ProcessIdentity::of(shell.id()).unwrap();

    let pids_file = root.join("pids");
    wait_until(
        "the worker's command to start",
        Duration::from_secs(120),
        || std::fs::read_to_string(&pids_file).is_ok_and(|text| text.ends_with('\n')),
    );
    let pids: Vec<u32> = std::fs::read_to_string(&pids_file)
        .unwrap()
        .split_whitespace()
        .map(|pid| pid.parse().unwrap())
        .collect();
    let [worker_pid, command_pid] = pids[..] else {
        panic!("expected two pids: {pids:?}");
    };
    let registered = |pid: u32| {
        ProcessRegistry::open(root.join("d"))
            .unwrap()
            .entries()
            .unwrap()
            .into_iter()
            .filter_map(|(_, entry)| entry)
            .find(|entry| entry.pid == pid)
            .unwrap_or_else(|| panic!("pid {pid} is not registered"))
    };
    let worker = registered(worker_pid);
    let command = registered(command_pid);
    assert_eq!(worker.kind, "worker");
    assert_eq!(
        worker.owner_pid,
        shell.id(),
        "the shell registered the worker"
    );
    assert_eq!(command.kind, "command");
    assert_eq!(
        command.owner_pid, worker_pid,
        "the worker registered its command"
    );
    assert_eq!(ProcessIdentity::of(worker_pid), Some(worker.identity()));
    assert_eq!(ProcessIdentity::of(command_pid), Some(command.identity()));
    let _leftovers = Leftovers(vec![worker.identity(), command.identity()]);

    assert_eq!(
        ProcessIdentity::of(shell.id()),
        Some(shell_identity),
        "the pid must still name the helper this test started"
    );
    shell.kill().expect("SIGKILL should be delivered");
    let killed_at = Instant::now();
    let exit = shell.wait().unwrap();
    assert!(!exit.success(), "{exit}");

    wait_until(
        "the worker and its command to be gone",
        Duration::from_secs(5),
        || ProcessIdentity::of(worker_pid).is_none() && ProcessIdentity::of(command_pid).is_none(),
    );
    // The command ran in its own group; nothing of that group may remain.
    let group = Command::new("/usr/bin/pgrep")
        .args(["-g", &command.pgid.to_string()])
        .output()
        .unwrap();
    assert!(
        group.stdout.is_empty(),
        "the command's process group outlived the worker: {}",
        String::from_utf8_lossy(&group.stdout)
    );
    println!(
        "worker {worker_pid} and command {command_pid} (group {}) gone {:?} after SIGKILL of shell {}",
        command.pgid,
        killed_at.elapsed(),
        shell_identity.pid
    );
}

/// Proposal §6's informational measurement, not a gate: a worker's cold start
/// on this repository, from the shell's `spawn` call to the worker's first
/// model request (`model_request_prepared` in the audit log). The source is a
/// local clone of this checkout's `HEAD`, so the worker indexes and assembles
/// context over the real tree; the model is `DAMAIAN_MOCK_MODEL_RESPONSE`, so
/// no provider latency is counted. Three workers in a row, printed per phase.
///
/// `#[ignore]`d per `AGENTS.md` because it spawns real processes. Run it by
/// hand:
///
/// ```sh
/// CARGO_TARGET_DIR="$PWD/target" cargo test -p desktop-shell --test workers -- \
///   --ignored --exact worker_cold_start_on_this_repository --nocapture
/// ```
#[test]
#[ignore]
fn worker_cold_start_on_this_repository() {
    // SAFETY: set before any thread of this test reads the environment.
    unsafe { std::env::set_var("DAMAIAN_MOCK_MODEL_RESPONSE", "Measured.") };
    let root = scratch("cold");
    let checkout = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let repo = root.join("r");
    let cloned = Command::new("git")
        .args(["clone", "-q", "--local"])
        .arg(&checkout)
        .arg(&repo)
        .status()
        .unwrap();
    assert!(cloned.success());
    let repo = std::fs::canonicalize(&repo).unwrap();
    let files = Command::new("git")
        .args(["ls-files"])
        .current_dir(&repo)
        .output()
        .unwrap();
    println!(
        "source: a clone of this checkout, {} tracked files",
        String::from_utf8_lossy(&files.stdout).lines().count()
    );
    let config = config(&root);
    let supervisor = Supervisor::new(
        PathBuf::from(env!("CARGO_BIN_EXE_damaian-desktop-shell")),
        WorkerTimeouts::default(),
    );
    for run in 1..=3 {
        let prepared_before = audit_events(&config.data_dir, "model_request_prepared").len();
        let started = Instant::now();
        let session = spawn(&supervisor, &config, &repo, &format!("Cold start {run}"));
        let spawned = started.elapsed();
        wait_until("the worker's socket", Duration::from_secs(60), || {
            supervisor
                .control(&session, "status", serde_json::Map::new())
                .is_ok()
        });
        let answering = started.elapsed();
        wait_until("the first model request", Duration::from_secs(300), || {
            audit_events(&config.data_dir, "model_request_prepared").len() > prepared_before
        });
        let first_request = started.elapsed();
        println!(
            "run {run}: spawn returned {spawned:?} (worktree, records, process), \
             socket answered {answering:?}, first model request {first_request:?}"
        );
        supervisor.shutdown_all();
    }
}

/// Re-executed by the kill test, never run on its own: the shell's part. It
/// installs the handler as `main` does, spawns a real worker whose scripted
/// model asks for `sleep 300` on every call, approves it if asked, and writes
/// the worker's pid and the command's once the worker has registered the
/// command. Then it waits to be killed.
#[test]
#[ignore]
fn kill_helper_runs_a_worker_command_and_waits() {
    let Ok(root) = std::env::var("DAMAIAN_WORKERS_KILL_DIR") else {
        return;
    };
    // SAFETY: set before this process starts any other thread; the worker
    // inherits it, as it inherits every variable of the shell's.
    unsafe {
        std::env::set_var(
            "DAMAIAN_MOCK_MODEL_RESPONSE",
            "DAMAIAN_COMMAND_V1\nCOMMAND: sleep 300\nREASON: Fixture.\nEND_COMMAND",
        )
    };
    let root = PathBuf::from(root);
    let config = config(&root);
    let (registry, audit) = ProcessRegistry::open_with_audit(&config).unwrap();
    registry.install_shutdown_handler(audit).unwrap();
    let registry = ProcessRegistry::open(&config.data_dir).unwrap();

    let repo = repository(&root);
    let supervisor = Supervisor::new(
        PathBuf::from(env!("CARGO_BIN_EXE_damaian-desktop-shell")),
        WorkerTimeouts::default(),
    );
    let session = spawn(&supervisor, &config, &repo, "Run the long command");
    let worker_pid = supervisor.pid(&session).unwrap();

    let deadline = Instant::now() + Duration::from_secs(100);
    let command_pid = loop {
        assert!(Instant::now() < deadline, "the command never started");
        if let Ok(status) = supervisor.control(&session, "status", serde_json::Map::new())
            && status["state"] == "waiting_for_approval"
        {
            let mut arguments = serde_json::Map::new();
            arguments.insert("proposalId".to_string(), status["proposalId"].clone());
            supervisor.control(&session, "approve", arguments).unwrap();
        }
        let command = registry
            .entries()
            .unwrap()
            .into_iter()
            .filter_map(|(_, entry)| entry)
            .find(|entry| entry.kind == "command" && entry.owner_pid == worker_pid);
        if let Some(command) = command {
            break command.pid;
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    let mut file = std::fs::File::create(root.join("pids.tmp")).unwrap();
    writeln!(file, "{worker_pid} {command_pid}").unwrap();
    drop(file);
    std::fs::rename(root.join("pids.tmp"), root.join("pids")).unwrap();

    std::thread::sleep(Duration::from_secs(120));
}

/// Re-executed by the signal test, never run on its own: installs the handler
/// as the standalone shell's `main` does, spawns one worker, and waits.
#[test]
#[ignore]
fn signal_helper_spawns_a_worker_and_waits() {
    let Ok(root) = std::env::var("DAMAIAN_WORKERS_HELPER_DIR") else {
        return;
    };
    let root = PathBuf::from(root);
    let config = config(&root);
    let (registry, audit) = ProcessRegistry::open_with_audit(&config).unwrap();
    registry.install_shutdown_handler(audit).unwrap();

    let repo = repository(&root);
    let supervisor = Supervisor::new(deaf_stub(&root), WorkerTimeouts::default());
    let session = spawn(&supervisor, &config, &repo, "Wait for the signal");
    let pid = supervisor.pid(&session).unwrap();
    let mut file = std::fs::File::create(root.join("worker.pid.tmp")).unwrap();
    writeln!(file, "{pid}").unwrap();
    drop(file);
    std::fs::rename(root.join("worker.pid.tmp"), root.join("worker.pid")).unwrap();

    std::thread::sleep(Duration::from_secs(60));
}
