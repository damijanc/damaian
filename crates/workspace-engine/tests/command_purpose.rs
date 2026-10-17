//! Spec 63 Task 1: a command's purpose is decided when it is proposed,
//! stored with the proposal, and carried to its execution — and it never
//! changes what the policy decided (requirement 6). Task 2: a failed
//! exploration command records no finding.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};
use workspace_engine::config::CommandAccess;
use workspace_engine::finding::{Finding, FindingSource, Severity};
use workspace_engine::{
    CancelToken, CommandProposal, CommandPurpose, Config, MockModelAdapter, ModelAdapter,
    ModelRequest, ModelRun, ToolCall, TurnProgress, TurnSink, WorkspaceEngine,
};

static COUNTER: AtomicU64 = AtomicU64::new(1);

fn temp_dir(name: &str) -> PathBuf {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock should work")
        .as_nanos();
    let dir = std::env::temp_dir().join(format!(
        "damaian-purpose-{name}-{now}-{}",
        COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir_all(&dir).expect("temp dir should be created");
    dir
}

/// No watcher, data kept in the fixture, and a non-login shell so the one
/// test that runs a command has nothing to load.
fn test_config(repo: &Path) -> Config {
    Config {
        data_dir: repo.join(".damaian"),
        enable_index_watcher: false,
        shell: "/bin/sh".to_string(),
        ..Config::default()
    }
}

/// One command of each purpose, plus the mixed and shell-control cases that
/// the policy treats most differently.
const COMMANDS: &[&str] = &[
    "ls",
    "ls -d missing-directory",
    "grep -rn nothing-matches .",
    "cd src && grep -rn 'x' --include=*.rs .",
    "git status",
    "git log --oneline -5",
    "cargo test",
    "cargo clippy --all-targets",
    "npm run lint",
    "python3 script.py",
    "git push",
    "rm -rf build",
    "git reset --hard",
    "docker ps",
    "curl https://example.com",
    "cat ../outside.txt",
];

#[test]
fn purpose_never_changes_what_the_policy_decided() {
    let repo = temp_dir("policy");
    for config in [
        test_config(&repo),
        Config {
            require_approval_for_all_commands: true,
            ..test_config(&repo)
        },
        Config {
            command_access: CommandAccess::ReadOnly,
            ..test_config(&repo)
        },
        Config {
            command_access: CommandAccess::None,
            ..test_config(&repo)
        },
    ] {
        let engine = WorkspaceEngine::new(config);
        let orchestrator = &engine.validation_orchestrator;
        for command in COMMANDS {
            let classification = orchestrator.classify_command(&repo, command);
            let proposal = orchestrator
                .propose_command(&repo, command, "Spec 63")
                .unwrap();
            assert_eq!(proposal.command, classification.command, "{command}");
            assert_eq!(proposal.risk, classification.risk, "{command}");
            assert_eq!(proposal.blocked, classification.blocked, "{command}");
            assert_eq!(
                proposal.requires_approval, classification.requires_approval,
                "{command}"
            );
            assert_eq!(proposal.reasons, classification.reasons, "{command}");
            assert_eq!(
                proposal.expected_effects, classification.expected_effects,
                "{command}"
            );
            assert_eq!(
                proposal.may_use_network, classification.may_use_network,
                "{command}"
            );
            assert_eq!(
                orchestrator.command_needs_approval(&repo, command),
                proposal.requires_approval || proposal.blocked,
                "{command}"
            );
        }
    }
    fs::remove_dir_all(repo).unwrap();
}

#[test]
fn a_proposal_records_its_purpose() {
    let repo = temp_dir("proposal");
    let engine = WorkspaceEngine::new(test_config(&repo));
    let purpose = |command: &str| {
        engine
            .validation_orchestrator
            .propose_command(&repo, command, "Spec 63")
            .unwrap()
            .purpose
    };

    assert_eq!(
        purpose("cd .claude/worktrees/x && grep -rn 'ContextManager::new' --include=*.rs ."),
        CommandPurpose::Exploration
    );
    assert_eq!(
        purpose("ls -d target; du -sh target; ls -d ../../target"),
        CommandPurpose::Exploration
    );
    assert_eq!(purpose("cargo test"), CommandPurpose::Check);
    assert_eq!(purpose("cargo clippy"), CommandPurpose::Check);
    assert_eq!(purpose("npm run lint"), CommandPurpose::Check);
    assert_eq!(purpose("python3 script.py"), CommandPurpose::Other);

    fs::remove_dir_all(repo).unwrap();
}

#[test]
fn detected_validations_are_checks() {
    let repo = temp_dir("detected");
    fs::write(
        repo.join("package.json"),
        "{\"scripts\":{\"test\":\"node --test\",\"lint\":\"eslint .\"}}\n",
    )
    .unwrap();
    let engine = WorkspaceEngine::new(test_config(&repo));
    let (map, _) = engine.repository_map(&repo).unwrap();
    let proposals = engine
        .validation_orchestrator
        .propose_detected_validations(&repo, &map)
        .unwrap();

    assert!(!proposals.is_empty());
    for proposal in &proposals {
        assert_eq!(
            proposal.purpose,
            CommandPurpose::Check,
            "{}",
            proposal.command
        );
        // Stored, not recomputed from the text on the way back.
        let stored = engine
            .validation_orchestrator
            .load_proposal(&proposal.id)
            .unwrap();
        assert_eq!(
            stored.purpose,
            CommandPurpose::Check,
            "{}",
            proposal.command
        );
    }

    fs::remove_dir_all(repo).unwrap();
}

#[test]
fn a_proposal_stored_before_purposes_reads_as_other() {
    let repo = temp_dir("legacy");
    let engine = WorkspaceEngine::new(test_config(&repo));
    let proposal = engine
        .validation_orchestrator
        .propose_command(&repo, "grep -rn x .", "Spec 63")
        .unwrap();
    assert_eq!(proposal.purpose, CommandPurpose::Exploration);

    let path = pending_path(&repo, &proposal);
    let stored = fs::read_to_string(&path).unwrap();
    let legacy = stored.replace("PURPOSE 11\nexploration\n", "");
    assert_ne!(legacy, stored, "the field should have been written");
    fs::write(&path, legacy).unwrap();

    let loaded = engine
        .validation_orchestrator
        .load_proposal(&proposal.id)
        .unwrap();
    assert_eq!(loaded.purpose, CommandPurpose::Other);
    assert_eq!(loaded.command, proposal.command);

    fs::remove_dir_all(repo).unwrap();
}

#[test]
fn the_execution_carries_the_stored_purpose() {
    let repo = temp_dir("execution");
    let engine = WorkspaceEngine::new(test_config(&repo));
    let proposal = engine
        .validation_orchestrator
        .propose_command(&repo, "ls -d missing-directory", "Spec 63")
        .unwrap();
    assert!(!proposal.requires_approval);

    let record = engine
        .validation_orchestrator
        .run_proposal(
            &proposal.id,
            false,
            "tester",
            None,
            &CancelToken::new(),
            &mut |_line: &str| {},
        )
        .unwrap();

    assert_ne!(record.execution.exit_code, Some(0));
    assert_eq!(record.execution.purpose, CommandPurpose::Exploration);

    fs::remove_dir_all(repo).unwrap();
}

// ---------------------------------------------------------------------------
// Task 2: no findings for exploration, through the chat loop.
// ---------------------------------------------------------------------------

/// Delegates to a scripted mock and keeps every request, so a test can see
/// the tool result the model was given.
struct Recording {
    inner: MockModelAdapter,
    requests: Vec<ModelRequest>,
}

impl ModelAdapter for Recording {
    fn stream_response(
        &mut self,
        request: &ModelRequest,
        cancel: &CancelToken,
        on_token: &mut dyn FnMut(&str),
        on_wait: &mut dyn FnMut(u64),
    ) -> workspace_engine::Result<ModelRun> {
        self.requests.push(request.clone());
        self.inner
            .stream_response(request, cancel, on_token, on_wait)
    }
}

/// One `run_command` round, then a plain answer.
fn one_command(command: &str) -> MockModelAdapter {
    let arguments = serde_json::json!({ "command": command, "reason": "Look around" });
    MockModelAdapter::new_sequence_with_tool_calls(
        vec![String::new(), "Done.".to_string()],
        vec![
            vec![ToolCall {
                id: "call_run_command".to_string(),
                name: "run_command".to_string(),
                arguments_json: arguments.to_string(),
            }],
            Vec::new(),
        ],
    )
}

/// What one agent command left behind: its session's findings, and the
/// tool result the model received for it.
struct AgentRun {
    findings: Vec<Finding>,
    tool_result: String,
}

/// Runs `command` as the agent would, approving it when the policy asks, so
/// a test covers whichever of `record_command_findings`' two call paths the
/// policy picks.
fn run_as_agent(engine: &WorkspaceEngine, repo: &Path, command: &str) -> AgentRun {
    let cancel = CancelToken::new();
    let (mut on_token, mut on_progress) = (|_: &str| {}, |_: TurnProgress| {});
    let mut sink = TurnSink {
        on_token: &mut on_token,
        on_progress: &mut on_progress,
        cancel: &cancel,
        steering: None,
    };
    let mut adapter = Recording {
        inner: one_command(command),
        requests: Vec::new(),
    };
    let result = engine
        .chat_orchestrator
        .ask_with_session(repo, "Look around", &[], None, &mut adapter, &mut sink)
        .expect("the turn should run");
    if let Some(proposal) = &result.command_proposal {
        assert!(!proposal.blocked, "{command}: {proposal:?}");
        adapter.inner = MockModelAdapter::new("Done.");
        engine
            .chat_orchestrator
            .resume_after_command_decision(&proposal.id, true, "tester", &mut adapter, &mut sink)
            .expect("the resumed turn should run");
    }

    let header = format!("Command: {command}\n");
    let tool_result = adapter
        .requests
        .iter()
        .flat_map(|request| &request.messages)
        .map(|message| message.content.clone())
        .find(|content| content.contains(&header))
        .unwrap_or_else(|| panic!("{command}: the model never saw the command's result"));
    let findings = engine
        .session_store
        .read_findings(&result.session.id, repo)
        .expect("findings should read");
    AgentRun {
        findings,
        tool_result,
    }
}

fn failed_exit(tool_result: &str) -> bool {
    tool_result.contains("Exit code: ") && !tool_result.contains("Exit code: 0\n")
}

/// Requirement 2 and the first two acceptance criteria. The commands are
/// the two from proposal §1 and an unsuccessful search. `/bin/sh` has no
/// `nomatch`, so the glob command fails here because `grep` found nothing,
/// not because the shell refused it; it is the same exploration either way.
#[test]
fn failed_exploration_commands_record_no_finding_and_still_reach_the_model() {
    let repo = temp_dir("exploration");
    let worktree = repo.join(".claude/worktrees/spec25-task8/src");
    fs::create_dir_all(&worktree).unwrap();
    fs::write(worktree.join("a.rs"), "fn main() {}\n").unwrap();
    // Outside the repository, or `grep -rn nothing-matches .` finds its own
    // command in the session log.
    let data = temp_dir("exploration-data");
    let engine = WorkspaceEngine::new(Config {
        data_dir: data.clone(),
        ..test_config(&repo)
    });

    for (command, says) in [
        (
            "cd .claude/worktrees/spec25-task8 && grep -rn 'ContextManager::new' --include=*.rs .",
            "",
        ),
        ("ls -d target; du -sh target; ls -d ../../target", "target"),
        ("grep -rn nothing-matches .", ""),
    ] {
        let proposal = engine
            .validation_orchestrator
            .propose_command(&repo, command, "Spec 63")
            .unwrap();
        assert_eq!(proposal.purpose, CommandPurpose::Exploration, "{command}");

        let run = run_as_agent(&engine, &repo, command);
        assert!(
            failed_exit(&run.tool_result),
            "{command}: {}",
            run.tool_result
        );
        assert!(run.tool_result.contains(says), "{}", run.tool_result);
        assert!(run.findings.is_empty(), "{command}: {:?}", run.findings);
    }

    fs::remove_dir_all(repo).unwrap();
    fs::remove_dir_all(data).unwrap();
}

/// Requirement 4: an unrecognised command keeps today's generic finding.
/// The script fails whether `python3` is installed or not.
#[test]
fn a_failed_other_command_records_a_generic_finding() {
    let repo = temp_dir("other");
    fs::write(repo.join("script.py"), "raise SystemExit('boom')\n").unwrap();
    let engine = WorkspaceEngine::new(test_config(&repo));

    let run = run_as_agent(&engine, &repo, "python3 script.py");

    assert!(failed_exit(&run.tool_result), "{}", run.tool_result);
    assert_eq!(run.findings.len(), 1, "{:?}", run.findings);
    assert_eq!(run.findings[0].source(), FindingSource::Command);
    assert!(
        run.findings[0].summary().starts_with("python3 script.py"),
        "{}",
        run.findings[0].summary()
    );

    fs::remove_dir_all(repo).unwrap();
}

/// Requirement 3: checks record findings as before. Neither project exists
/// in the fixture, so both fail before running anything: the Rust test
/// parser reads cargo's `error:` line, and `npm` gets the generic fallback.
#[test]
fn failed_checks_record_findings_as_before() {
    let repo = temp_dir("checks");
    let engine = WorkspaceEngine::new(test_config(&repo));

    for command in ["cargo test", "npm run lint"] {
        let run = run_as_agent(&engine, &repo, command);
        assert!(
            failed_exit(&run.tool_result),
            "{command}: {}",
            run.tool_result
        );
        assert!(
            run.findings
                .iter()
                .any(|finding| finding.severity() == Severity::Error),
            "{command}: {:?}",
            run.findings
        );
    }

    fs::remove_dir_all(repo).unwrap();
}

fn pending_path(repo: &Path, proposal: &CommandProposal) -> PathBuf {
    repo.join(".damaian")
        .join("commands")
        .join("pending")
        .join(format!("{}.dcmd", proposal.id))
}
