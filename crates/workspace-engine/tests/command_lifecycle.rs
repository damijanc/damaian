//! Requirement 5's end-to-end command lifecycle: a running command is killed at
//! its deadline or on a stop, and reports output before it exits.
//!
//! Every test that spawns a real shell is `#[ignore]`d per `AGENTS.md`, with the
//! manual command in its doc comment. The pure decision the kill rests on —
//! `classify_wait` in `command_runner.rs` — has non-ignored unit tests, so the
//! default suite still covers the part that must not be a guess. The storage
//! and audit tests at the end spawn nothing and run in the default suite.

use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use workspace_engine::{
    ApprovalCause, AuditLog, CancelToken, CommandPolicy, CommandPurpose, CommandRunOptions,
    CommandRunner, CommandTermination, Config, SecretScanner, WorkspaceEngine,
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
        purpose: CommandPurpose::Other,
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

/// A repository-scoped engine whose data lives in the fixture, with the
/// watcher off (AGENTS.md's filesystem-watcher rule). Nothing here spawns a
/// shell: these tests cover what a proposal stores and audits.
fn proposal_fixture(name: &str) -> (WorkspaceEngine, PathBuf) {
    let repo = temp_dir(name);
    let config = Config {
        data_dir: repo.join(".damaian"),
        enable_index_watcher: false,
        ..Config::default()
    };
    (WorkspaceEngine::new(config), repo)
}

/// Spec 62 Task 5: the causes the classifier decided are stored with the
/// proposal, so a reload (reattach, *Allow Always*) sees the same values in
/// the same order instead of only their rendered strings.
#[test]
fn a_stored_proposal_round_trips_its_causes_in_order() {
    let (engine, repo) = proposal_fixture("causes-round-trip");
    let proposal = engine
        .validation_orchestrator
        .propose_command(&repo, "python3 ../script.py", "Spec 62")
        .unwrap();

    // Two causes, in classifier order: the unknown program first, then the
    // path that leaves the repository.
    assert_eq!(
        proposal.causes,
        vec![
            ApprovalCause::Unknown {
                program: "python3".to_string(),
            },
            ApprovalCause::OutsideRepository {
                path: "../script.py".to_string(),
            },
        ],
        "the fixture command must produce two ordered causes"
    );

    let loaded = engine
        .validation_orchestrator
        .load_proposal(&proposal.id)
        .unwrap();

    assert_eq!(loaded.causes, proposal.causes);
    assert_eq!(loaded, proposal, "the whole proposal round-trips");

    fs::remove_dir_all(repo).unwrap();
}

/// A permission block is the one cause whose field is not a string, so it
/// round-trips through the same JSON as the rest.
#[test]
fn a_stored_proposal_round_trips_a_profile_block() {
    let repo = temp_dir("causes-profile-block");
    let config = Config {
        data_dir: repo.join(".damaian"),
        enable_index_watcher: false,
        command_access: workspace_engine::CommandAccess::None,
        ..Config::default()
    };
    let engine = WorkspaceEngine::new(config);
    let proposal = engine
        .validation_orchestrator
        .propose_command(&repo, "cd x && git diff | head", "Spec 62")
        .unwrap();

    assert_eq!(
        proposal.causes,
        vec![
            ApprovalCause::ShellControl {
                constructs: "`&&`, `|`".to_string(),
            },
            ApprovalCause::ProfileBlocked {
                access: workspace_engine::CommandAccess::None,
            },
        ]
    );

    let loaded = engine
        .validation_orchestrator
        .load_proposal(&proposal.id)
        .unwrap();

    assert_eq!(loaded.causes, proposal.causes);
    assert_eq!(loaded, proposal);

    fs::remove_dir_all(repo).unwrap();
}

/// A cause can carry model-chosen text (an unknown program's first word), and
/// the audit log must not become a new way for a credential to leave the
/// machine. The event names the cause instead of quoting it, and
/// `AuditLog::record` redacts every value it is given, as it does beside it.
#[test]
fn the_audit_causes_name_causes_without_command_text() {
    let (engine, repo) = proposal_fixture("causes-redaction");
    let proposal = engine
        .validation_orchestrator
        .propose_command(&repo, "AKIAIOSFODNN7EXAMPLE ../script.py", "Spec 62")
        .unwrap();

    let events =
        fs::read_to_string(repo.join(".damaian").join("audit").join("events.jsonl")).unwrap();
    let event = events
        .lines()
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
        .find(|event| {
            event.get("eventType").and_then(|value| value.as_str())
                == Some("command_proposal_stored")
                && event.get("proposalId").and_then(|value| value.as_str())
                    == Some(proposal.id.as_str())
        })
        .expect("the proposal should have been audited");

    assert_eq!(event["causes"], "unknown,outside_repository");
    assert!(
        !event["causes"]
            .as_str()
            .unwrap()
            .contains("AKIAIOSFODNN7EXAMPLE")
    );
    assert!(
        event["command"].as_str().unwrap().contains("[REDACTED_"),
        "the audit path redacts the command beside the causes: {event}"
    );

    fs::remove_dir_all(repo).unwrap();
}

/// A proposal written before this change has no `CAUSES` field. It must still
/// load, with no causes rather than an error (the same tolerance `PURPOSE`
/// got in spec 63).
#[test]
fn a_proposal_stored_before_causes_loads_with_no_causes() {
    let (engine, repo) = proposal_fixture("causes-legacy");
    let working_directory = repo.to_string_lossy().to_string();
    let mut raw = String::from("DAMAIAN_COMMAND_PROPOSAL_V1\n");
    for (name, value) in [
        ("ID", "cmdprop_legacy"),
        ("COMMAND", "npm test"),
        ("WORKING_DIRECTORY", working_directory.as_str()),
        ("REASON", "Run the tests"),
        ("RISK", "medium"),
        ("REQUIRES_APPROVAL", "true"),
        ("BLOCKED", "false"),
        ("EXPECTED_EFFECTS", "Runs project validation"),
        ("MAY_USE_NETWORK", "false"),
        ("REASONS", "Validation command"),
        ("CREATED_AT_MS", "1"),
        ("STATUS", "pending"),
    ] {
        raw.push_str(&format!("{name} {}\n{value}\n", value.len()));
    }
    raw.push_str("END_COMMAND_PROPOSAL\n");
    let pending = repo.join(".damaian").join("commands").join("pending");
    fs::create_dir_all(&pending).unwrap();
    fs::write(pending.join("cmdprop_legacy.dcmd"), raw).unwrap();

    let loaded = engine
        .validation_orchestrator
        .load_proposal("cmdprop_legacy")
        .unwrap();

    assert!(loaded.causes.is_empty());
    assert_eq!(loaded.command, "npm test");

    fs::remove_dir_all(repo).unwrap();
}

/// Requirement 6: the audit record can be analysed for which causes produce
/// prompts, so `command_proposal_stored` names them in order.
#[test]
fn the_stored_proposal_audit_names_its_causes_in_order() {
    let (engine, repo) = proposal_fixture("causes-audit");
    let proposal = engine
        .validation_orchestrator
        .propose_command(&repo, "python3 ../script.py", "Spec 62")
        .unwrap();

    let events =
        fs::read_to_string(repo.join(".damaian").join("audit").join("events.jsonl")).unwrap();
    let event = events
        .lines()
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
        .find(|event| {
            event.get("eventType").and_then(|value| value.as_str())
                == Some("command_proposal_stored")
                && event.get("proposalId").and_then(|value| value.as_str())
                    == Some(proposal.id.as_str())
        })
        .expect("the proposal should have been audited");

    assert_eq!(event["causes"], "unknown,outside_repository");

    fs::remove_dir_all(repo).unwrap();
}
