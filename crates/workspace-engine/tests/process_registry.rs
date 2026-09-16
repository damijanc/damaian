//! The real-process half of spec 46. Every test here spawns, signals or kills
//! something, so every test here is `#[ignore]`d, per `AGENTS.md`.

use std::path::PathBuf;
use std::time::{Duration, Instant};

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("damaian-sweep-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// The pid a registry entry names, read back out of its filename.
fn registered_pid(processes: &std::path::Path) -> Option<u32> {
    std::fs::read_dir(processes)
        .ok()?
        .filter_map(|entry| entry.ok())
        .find_map(|entry| {
            entry
                .file_name()
                .to_str()
                .and_then(|name| name.split('-').next())
                .and_then(|pid| pid.parse::<u32>().ok())
        })
}

/// A **silent** child is the case that matters. One that writes would die of
/// `SIGPIPE` the moment the parent's pipe read-ends close, and this test would
/// pass without the handler having done anything at all.
///
/// `#[ignore]`d per `AGENTS.md` because it spawns and signals real processes.
/// Run it by hand:
///
/// ```sh
/// cargo test -p workspace-engine --test process_registry -- --ignored --exact \
///   sigint_cleans_up_a_silent_child_and_still_exits_signalled
/// ```
#[test]
#[ignore]
fn sigint_cleans_up_a_silent_child_and_still_exits_signalled() {
    signalling_the_owner_cleans_up_and_re_raises(libc::SIGINT, "sigint");
}

/// The same guarantee for `SIGTERM`, which is the one that catches a watchdog
/// re-raising a *hardcoded* signal: such a handler still cleans the child up
/// and still exits signalled, but reports `SIGINT` to whatever ran it. Only
/// comparing against the signal actually sent detects that.
///
/// `#[ignore]`d per `AGENTS.md` because it spawns and signals real processes.
/// Run it by hand:
///
/// ```sh
/// cargo test -p workspace-engine --test process_registry -- --ignored --exact \
///   sigterm_cleans_up_a_silent_child_and_re_raises_sigterm_not_sigint
/// ```
#[test]
#[ignore]
fn sigterm_cleans_up_a_silent_child_and_re_raises_sigterm_not_sigint() {
    signalling_the_owner_cleans_up_and_re_raises(libc::SIGTERM, "sigterm");
}

fn signalling_the_owner_cleans_up_and_re_raises(signal: libc::c_int, name: &str) {
    use std::os::unix::process::ExitStatusExt;
    use std::process::Command;
    use workspace_engine::ProcessIdentity;

    let data_dir = scratch(name);
    let mut owner = Command::new(std::env::current_exe().unwrap())
        .args([
            "--ignored",
            "--exact",
            "sigint_helper_registers_a_silent_child_and_waits",
        ])
        .env("DAMAIAN_SWEEP_DIR", &data_dir)
        .spawn()
        .expect("owner should spawn");

    // Wait for the entry to reach disk rather than sleeping a guessed interval.
    let processes = data_dir.join("processes");
    let deadline = Instant::now() + Duration::from_secs(30);
    let child_pid = loop {
        if let Some(pid) = registered_pid(&processes) {
            break pid;
        }
        assert!(Instant::now() < deadline, "the helper never registered");
        std::thread::sleep(Duration::from_millis(20));
    };

    assert!(
        ProcessIdentity::of(child_pid).is_some(),
        "the child must be alive before the signal, or this proves nothing"
    );

    // By pid, through the handle we own — never by name. `AGENTS.md`.
    // SAFETY: `kill` takes two integers by value and touches no memory.
    unsafe { libc::kill(owner.id() as libc::pid_t, signal) };
    let status = owner.wait().expect("owner should be reaped");

    assert_eq!(
        status.signal(),
        Some(signal),
        "the handler must re-raise the signal that arrived, so the exit status \
         still says it was signalled and says so accurately"
    );

    let deadline = Instant::now() + Duration::from_secs(10);
    while ProcessIdentity::of(child_pid).is_some() {
        assert!(
            Instant::now() < deadline,
            "the silent child survived the signal, so the handler did nothing"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// Re-executed by the test above, never run on its own. Registers a silent
/// child, installs the handler, and waits to be signalled.
#[test]
#[ignore]
fn sigint_helper_registers_a_silent_child_and_waits() {
    use std::os::unix::process::CommandExt;
    use std::process::{Command, Stdio};
    use workspace_engine::{AuditLog, ProcessKind, ProcessRegistry, SecretScanner};

    let Ok(data_dir) = std::env::var("DAMAIAN_SWEEP_DIR") else {
        // Run directly rather than re-executed: there is nothing to do, and
        // doing anything would spawn a process no one is waiting for.
        return;
    };
    let registry = ProcessRegistry::open(&data_dir).unwrap();
    let audit = AuditLog::new(&data_dir, true, SecretScanner::new(Vec::new()));
    registry
        .clone()
        .install_shutdown_handler(audit)
        .expect("handler should install");

    // Never `wait`ed on deliberately: this helper is killed by the signal the
    // test sends, and the child has to outlive it exactly as a crash would
    // leave it. Reaping the child here would remove the very thing the sweep
    // is meant to find.
    #[allow(clippy::zombie_processes)]
    let child = Command::new("/bin/sh")
        .arg("-lc")
        .arg("sleep 300")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0)
        .spawn()
        .unwrap();
    let handle = registry
        .register(ProcessKind::Command, "ses_1", child.id())
        .unwrap();
    // The entry must survive this process, exactly as a crash would leave it.
    std::mem::forget(handle);

    std::thread::sleep(Duration::from_secs(60));
}
