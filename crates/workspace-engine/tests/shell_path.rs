//! Commands see the `PATH` the user's interactive login shell has, per
//! `docs/specs/69_interactive_shell_path.md`.
//!
//! Every test uses a fake `zsh`, a `/bin/sh` script in a scratch directory,
//! never the user's own shell: the real one runs their rc files and takes
//! about a second. The fake gives `-ilc` and `-lc` different `PATH`s, the way
//! `~/.zshrc` and a login profile's `path_helper` do, and records each
//! interactive start.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use workspace_engine::shell_path::{ResolutionStatus, resolve};
use workspace_engine::{
    AuditLog, CancelToken, CommandPolicy, CommandPurpose, CommandRunOptions, CommandRunner, Config,
    SecretScanner,
};

static COUNTER: AtomicU64 = AtomicU64::new(1);

const INTERACTIVE_PATH: &str = "/interactive/bin:/usr/bin:/bin";
const LOGIN_PATH: &str = "/usr/bin:/bin:/login/bin";

fn scratch(name: &str) -> PathBuf {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!(
        "damaian-shell-path-{name}-{now}-{}",
        COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir_all(&dir).unwrap();
    // Canonical, so it compares with what `pwd` prints (`/private/var/…`).
    fs::canonicalize(dir).unwrap()
}

/// A fake `zsh` in `dir`. In `-ilc` mode it prints rc noise around the
/// script and runs it with [`INTERACTIVE_PATH`]; in `-lc` mode it runs the
/// command with [`LOGIN_PATH`]. `interactive` replaces the `-ilc` branch.
fn fake_zsh(dir: &Path, interactive: Option<&str>) -> String {
    let log = dir.join("interactive-starts");
    let interactive = interactive.map(str::to_string).unwrap_or_else(|| {
        format!(
            "echo rc noise before\n    \
             PATH={INTERACTIVE_PATH}; export PATH\n    \
             /bin/sh -c \"$2\"\n    \
             echo rc noise after"
        )
    });
    let script = format!(
        "#!/bin/sh\n\
         case \"$1\" in\n  \
         -ilc)\n    \
         echo \"$(pwd) $DAMAIAN_RESOLVING_SHELL_PATH\" >> '{log}'\n    \
         {interactive}\n    \
         ;;\n  \
         -lc)\n    \
         PATH={LOGIN_PATH}; export PATH\n    \
         exec /bin/sh -c \"$2\"\n    \
         ;;\n\
         esac\n",
        log = log.display(),
    );
    let shell = dir.join("zsh");
    fs::write(&shell, script).unwrap();
    fs::set_permissions(&shell, fs::Permissions::from_mode(0o755)).unwrap();
    shell.to_string_lossy().to_string()
}

fn interactive_starts(dir: &Path) -> Vec<String> {
    fs::read_to_string(dir.join("interactive-starts"))
        .map(|text| text.lines().map(str::to_string).collect())
        .unwrap_or_default()
}

fn runner(dir: &Path, shell: &str, enabled: bool) -> CommandRunner {
    let config = Config {
        data_dir: dir.join("data"),
        shell: shell.to_string(),
        command_path_from_interactive_shell: enabled,
        enable_index_watcher: false,
        ..Config::default()
    };
    let scanner = SecretScanner::new(config.secret_patterns.clone());
    let policy = CommandPolicy::new(config.clone());
    let audit = AuditLog::new(&config.data_dir, true, scanner.clone());
    CommandRunner::new(config, policy, audit, scanner)
}

fn run(runner: &CommandRunner, dir: &Path, command: &str) -> String {
    let cancel = CancelToken::new();
    let mut on_output = |_line: &str| {};
    runner
        .run(
            command,
            dir,
            "test",
            CommandRunOptions {
                approved: true,
                approved_by: Some("local_user"),
                task_id: None,
                cancel: &cancel,
                on_output: &mut on_output,
                purpose: CommandPurpose::Other,
            },
        )
        .expect("the command should run")
        .stdout
        .trim()
        .to_string()
}

#[test]
fn the_path_is_read_from_between_the_rc_noise() {
    let dir = scratch("resolve");
    let shell = fake_zsh(&dir, None);
    let resolution = resolve(&shell, Duration::from_secs(5), &dir.join("data"));
    assert_eq!(resolution.status, ResolutionStatus::Resolved);
    assert_eq!(resolution.path.as_deref(), Some(INTERACTIVE_PATH));
    assert_eq!(resolution.entries, 3);
}

#[test]
fn the_interactive_shell_runs_in_home_and_is_told_why() {
    let dir = scratch("home");
    let shell = fake_zsh(&dir, None);
    resolve(&shell, Duration::from_secs(5), &dir.join("data"));
    let home = fs::canonicalize(std::env::var("HOME").unwrap()).unwrap();
    assert_eq!(
        interactive_starts(&dir),
        vec![format!("{} 1", home.display())],
        "never in a repository, and with DAMAIAN_RESOLVING_SHELL_PATH=1"
    );
}

#[test]
fn a_shell_that_does_not_answer_is_killed_at_the_timeout() {
    let dir = scratch("timeout");
    let pid_file = dir.join("pid");
    let shell = fake_zsh(
        &dir,
        Some(&format!(
            "echo $$ > '{}'; exec sleep 30",
            pid_file.display()
        )),
    );
    let started = Instant::now();
    let resolution = resolve(&shell, Duration::from_millis(500), &dir.join("data"));
    assert_eq!(resolution.status, ResolutionStatus::TimedOut);
    assert_eq!(resolution.path, None);
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "{:?}",
        started.elapsed()
    );
    let pid: i32 = fs::read_to_string(&pid_file)
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    // SAFETY: signal 0 only checks that the pid exists.
    let alive = unsafe { libc::kill(pid, 0) } == 0;
    assert!(!alive, "the timed-out shell must be gone");
}

#[test]
fn a_shell_that_prints_no_markers_failed() {
    let dir = scratch("no-markers");
    let shell = fake_zsh(&dir, Some("echo just rc noise; exit 0"));
    let resolution = resolve(&shell, Duration::from_secs(5), &dir.join("data"));
    assert_eq!(resolution.status, ResolutionStatus::Failed);
    assert_eq!(resolution.path, None);
}

#[test]
fn a_command_sees_the_interactive_path_in_its_order() {
    // The fake's `-lc` branch sets LOGIN_PATH, standing in for a login
    // profile that rewrites PATH; the command must still see the
    // interactive shell's PATH, in its order.
    let dir = scratch("command");
    let shell = fake_zsh(&dir, None);
    let runner = runner(&dir, &shell, true);
    assert_eq!(run(&runner, &dir, "echo \"$PATH\""), INTERACTIVE_PATH);
}

#[test]
fn with_the_setting_off_no_interactive_shell_starts() {
    let dir = scratch("off");
    let shell = fake_zsh(&dir, None);
    let runner = runner(&dir, &shell, false);
    assert_eq!(run(&runner, &dir, "echo \"$PATH\""), LOGIN_PATH);
    assert!(interactive_starts(&dir).is_empty());
}

#[test]
fn a_failed_resolution_leaves_commands_as_they_were() {
    let dir = scratch("failed-command");
    let shell = fake_zsh(&dir, Some("exit 1"));
    let runner = runner(&dir, &shell, true);
    assert_eq!(run(&runner, &dir, "echo \"$PATH\""), LOGIN_PATH);
}

#[test]
fn the_interactive_shell_starts_once_and_is_audited_once() {
    let dir = scratch("once");
    let shell = fake_zsh(&dir, None);
    let runner = runner(&dir, &shell, true);
    assert_eq!(run(&runner, &dir, "echo \"$PATH\""), INTERACTIVE_PATH);
    assert_eq!(run(&runner, &dir, "echo \"$PATH\""), INTERACTIVE_PATH);
    assert_eq!(interactive_starts(&dir).len(), 1);

    let events = fs::read_to_string(dir.join("data/audit/events.jsonl")).unwrap();
    let resolved: Vec<&str> = events
        .lines()
        .filter(|line| line.contains("\"eventType\":\"shell_path_resolved\""))
        .collect();
    assert_eq!(resolved.len(), 1, "{events}");
    assert!(
        resolved[0].contains("\"status\":\"resolved\""),
        "{}",
        resolved[0]
    );
    assert!(resolved[0].contains("\"entries\":\"3\""), "{}", resolved[0]);
    assert!(
        !resolved[0].contains("/interactive/bin"),
        "the PATH itself is not audited: {}",
        resolved[0]
    );
}

/// The user's real shell, rc files and all: the case the fakes stand in for.
///
/// `#[ignore]`d per `AGENTS.md` because it starts the user's own interactive
/// login shell. Run it by hand:
///
/// ```sh
/// cargo nextest run -p workspace-engine --test shell_path --run-ignored only
/// ```
#[test]
#[ignore]
fn the_users_own_shell_answers_with_a_usable_path() {
    let dir = scratch("real-shell");
    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".to_string());
    let resolution = resolve(&shell, Duration::from_secs(5), &dir.join("data"));
    assert_eq!(
        resolution.status,
        ResolutionStatus::Resolved,
        "{resolution:?}"
    );
    let path = resolution.path.expect("a resolved PATH");
    assert!(
        path.split(':').all(|entry| entry.starts_with('/')),
        "{path}"
    );
    let runner = runner(&dir, &shell, true);
    assert_eq!(run(&runner, &dir, "echo \"$PATH\""), path);
}
