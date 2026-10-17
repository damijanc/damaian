//! Spec 63 Task 1: a command's purpose is decided when it is proposed,
//! stored with the proposal, and carried to its execution — and it never
//! changes what the policy decided (requirement 6).

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};
use workspace_engine::config::CommandAccess;
use workspace_engine::{CancelToken, CommandProposal, CommandPurpose, Config, WorkspaceEngine};

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

fn pending_path(repo: &Path, proposal: &CommandProposal) -> PathBuf {
    repo.join(".damaian")
        .join("commands")
        .join("pending")
        .join(format!("{}.dcmd", proposal.id))
}
