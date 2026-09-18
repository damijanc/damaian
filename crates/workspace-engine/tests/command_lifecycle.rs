//! Requirement 5's end-to-end command lifecycle: a running command is killed at
//! its deadline or on a stop, and reports output before it exits.
//!
//! Every test here spawns a real shell, so per `AGENTS.md` each is `#[ignore]`d
//! with the manual command in its doc comment. The pure decision the kill rests
//! on — `classify_wait` in `command_runner.rs` — has non-ignored unit tests, so
//! the default suite still covers the part that must not be a guess.

use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use workspace_engine::{
    AuditLog, CancelToken, CommandPolicy, CommandRunOptions, CommandRunner, CommandTermination,
    Config, SecretScanner,
};

static COUNTER: AtomicU64 = AtomicU64::new(1);

fn temp_dir(name: &str) -> PathBuf {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock should work")
        .as_nanos();
    let dir = std::env::temp_dir().join(format!(
        "damaian-cmd-lifecycle-{name}-{now}-{}",
        COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir_all(&dir).expect("temp dir should be created");
    dir
}

fn runner_with_timeout(name: &str, seconds: usize) -> (CommandRunner, PathBuf) {
    let data_dir = temp_dir(name);
    let config = Config {
        data_dir: data_dir.clone(),
        command_timeout_secs: seconds,
        // A non-login shell keeps the fixtures fast and has nothing to load.
        shell: "/bin/sh".to_string(),
        ..Config::default()
    };
    let scanner = SecretScanner::new(config.secret_patterns.clone());
    let policy = CommandPolicy::new(config.clone());
    let audit = AuditLog::new(&data_dir, false, scanner.clone());
    (CommandRunner::new(config, policy, audit, scanner), data_dir)
}

fn options<'a>(
    cancel: &'a CancelToken,
    on_output: &'a mut dyn FnMut(&str),
) -> CommandRunOptions<'a> {
    CommandRunOptions {
        approved: true,
        approved_by: Some("local_user"),
        task_id: None,
        cancel,
        on_output,
    }
}

/// ```sh
/// cargo test -p workspace-engine --test command_lifecycle -- --ignored --exact \
///   a_command_past_its_timeout_is_killed_and_reports_no_exit_code
/// ```
#[test]
#[ignore]
fn a_command_past_its_timeout_is_killed_and_reports_no_exit_code() {
    let (runner, data_dir) = runner_with_timeout("timeout", 1);
    let cancel = CancelToken::new();
    let mut on_output = |_line: &str| {};

    let started = Instant::now();
    let execution = runner
        .run(
            "sleep 30",
            &data_dir,
            "timeout test",
            options(&cancel, &mut on_output),
        )
        .expect("a killed command is an outcome, not an error");

    assert!(
        started.elapsed() < Duration::from_secs(10),
        "a 30s sleep under a 1s deadline must not run to completion"
    );
    assert_eq!(execution.termination, CommandTermination::TimedOut);
    assert_eq!(
        execution.exit_code, None,
        "a killed command has no exit code; reporting one would read as a clean exit"
    );
}

/// ```sh
/// cargo test -p workspace-engine --test command_lifecycle -- --ignored --exact \
///   a_stop_kills_the_command_before_its_deadline
/// ```
#[test]
#[ignore]
fn a_stop_kills_the_command_before_its_deadline() {
    let (runner, data_dir) = runner_with_timeout("cancelled", 600);
    let cancel = CancelToken::new();
    cancel.cancel();
    let mut on_output = |_line: &str| {};

    let started = Instant::now();
    let execution = runner
        .run(
            "sleep 30",
            &data_dir,
            "cancel test",
            options(&cancel, &mut on_output),
        )
        .expect("a stopped command is an outcome, not an error");

    assert!(
        started.elapsed() < Duration::from_secs(10),
        "a stop must take effect during the command, not after it"
    );
    assert_eq!(execution.termination, CommandTermination::Cancelled);
    assert_eq!(execution.exit_code, None);
}

/// The acceptance case: a stop issued *during* a command takes effect during
/// it, not after it returns.
///
/// ```sh
/// cargo test -p workspace-engine --test command_lifecycle -- --ignored --exact \
///   a_stop_mid_command_takes_effect_before_it_would_have_exited
/// ```
#[test]
#[ignore]
fn a_stop_mid_command_takes_effect_before_it_would_have_exited() {
    let (runner, data_dir) = runner_with_timeout("cancel-mid", 600);
    let cancel = CancelToken::new();
    let stopper = cancel.clone();
    let handle = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(500));
        stopper.cancel();
    });

    let mut on_output = |_line: &str| {};
    let started = Instant::now();
    let execution = runner
        .run(
            "sleep 30",
            &data_dir,
            "mid cancel test",
            options(&cancel, &mut on_output),
        )
        .unwrap();
    handle.join().unwrap();

    assert!(
        started.elapsed() < Duration::from_secs(10),
        "a stop 0.5s in must end a 30s command, not wait it out"
    );
    assert_eq!(execution.termination, CommandTermination::Cancelled);
    assert_eq!(execution.exit_code, None);
}

/// ```sh
/// cargo test -p workspace-engine --test command_lifecycle -- --ignored --exact \
///   output_is_streamed_before_the_command_exits
/// ```
#[test]
#[ignore]
fn output_is_streamed_before_the_command_exits() {
    let (runner, data_dir) = runner_with_timeout("stream", 1);
    let cancel = CancelToken::new();
    let lines = Arc::new(Mutex::new(Vec::<String>::new()));
    {
        let lines = Arc::clone(&lines);
        let mut on_output = move |line: &str| lines.lock().unwrap().push(line.to_string());
        let execution = runner
            .run(
                "printf 'first\\n'; sleep 30",
                &data_dir,
                "stream test",
                options(&cancel, &mut on_output),
            )
            .unwrap();
        assert_eq!(execution.termination, CommandTermination::TimedOut);
    }

    let lines = lines.lock().unwrap();
    assert!(
        lines.iter().any(|line| line.contains("first")),
        "output must reach the callback before the command exits: {lines:?}"
    );
}

/// The live stream must be redacted on the same path as the persisted output,
/// or streaming becomes the one way around the scanner.
///
/// ```sh
/// cargo test -p workspace-engine --test command_lifecycle -- --ignored --exact \
///   a_secret_in_streamed_output_is_redacted
/// ```
#[test]
#[ignore]
fn a_secret_in_streamed_output_is_redacted() {
    let (runner, data_dir) = runner_with_timeout("redact", 5);
    let cancel = CancelToken::new();
    let lines = Arc::new(Mutex::new(Vec::<String>::new()));
    {
        let lines = Arc::clone(&lines);
        let mut on_output = move |line: &str| lines.lock().unwrap().push(line.to_string());
        runner
            .run(
                "echo AKIAIOSFODNN7EXAMPLE",
                &data_dir,
                "redaction test",
                options(&cancel, &mut on_output),
            )
            .unwrap();
    }

    let lines = lines.lock().unwrap();
    assert!(
        !lines
            .iter()
            .any(|line| line.contains("AKIAIOSFODNN7EXAMPLE")),
        "the live stream must be redacted: {lines:?}"
    );
    assert!(
        lines.iter().any(|line| line.contains("[REDACTED_")),
        "a redaction marker is expected in the stream: {lines:?}"
    );
}
