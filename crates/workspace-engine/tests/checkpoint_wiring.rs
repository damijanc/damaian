//! Checkpoints as the turn flow uses them, per
//! `docs/specs/16_session_checkpoints_and_rewind.md` requirements 1 and 3.
//!
//! `checkpoints.rs` covers the store on its own. These tests go through the
//! orchestrators: a turn takes a checkpoint before it runs, an accepted patch
//! and an approved command put their paths in it, and rewinding the turn puts
//! the repository and the conversation back.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};
use workspace_engine::{
    CancelToken, CheckpointOrigin, CheckpointRestoreOptions, Config, MockModelAdapter,
    ModelAdapter, TurnProgress, TurnSink, WorkspaceEngine,
};

static COUNTER: AtomicU64 = AtomicU64::new(1);

fn temp_repo(name: &str) -> PathBuf {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock should work")
        .as_nanos();
    let repo = std::env::temp_dir().join(format!(
        "damaian-cp-wiring-{name}-{now}-{}",
        COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir_all(&repo).expect("repository should be created");
    repo
}

fn engine_for(repo: &Path) -> WorkspaceEngine {
    WorkspaceEngine::new(Config {
        data_dir: repo.join(".damaian"),
        ..Config::default()
    })
}

fn write(repo: &Path, relative_path: &str, content: &str) {
    let path = repo.join(relative_path);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, content).unwrap();
}

fn ask(
    engine: &WorkspaceEngine,
    repo: &Path,
    prompt: &str,
    adapter: &mut dyn ModelAdapter,
) -> workspace_engine::ChatTurnResult {
    let cancel = CancelToken::new();
    let mut on_token = |_token: &str| {};
    let mut on_progress = |_event: TurnProgress| {};
    let mut sink = TurnSink {
        on_token: &mut on_token,
        on_progress: &mut on_progress,
        cancel: &cancel,
    };
    engine
        .chat_orchestrator
        .ask_with_session(repo, prompt, &[], None, adapter, &mut sink)
        .expect("the turn should run")
}

fn resume(
    engine: &WorkspaceEngine,
    proposal_id: &str,
    adapter: &mut dyn ModelAdapter,
) -> workspace_engine::ChatTurnResult {
    let cancel = CancelToken::new();
    let mut on_token = |_token: &str| {};
    let mut on_progress = |_event: TurnProgress| {};
    let mut sink = TurnSink {
        on_token: &mut on_token,
        on_progress: &mut on_progress,
        cancel: &cancel,
    };
    engine
        .chat_orchestrator
        .resume_after_command_decision(proposal_id, true, "tester", adapter, &mut sink)
        .expect("the approved command should resume the turn")
}

fn git(repo: &Path, args: &[&str]) {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .output()
        .expect("git should run");
    assert!(
        output.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn git_repo(repo: &Path) {
    git(repo, &["init", "--quiet"]);
    git(repo, &["config", "user.email", "test@damaian.local"]);
    git(repo, &["config", "user.name", "Damaian Test"]);
    git(repo, &["config", "gc.auto", "0"]);
    git(repo, &["config", "maintenance.auto", "false"]);
}

#[test]
fn a_chat_turn_takes_a_checkpoint_before_it_runs() {
    let repo = temp_repo("chat-turn");
    write(&repo, "src/a.rs", "fn main() {}\n");
    let engine = engine_for(&repo);
    let mut adapter = MockModelAdapter::new("Nothing to change.");

    let result = ask(&engine, &repo, "Explain this file", &mut adapter);

    let checkpoints = engine
        .checkpoint_store
        .list_checkpoints(&result.session.repository_id)
        .expect("checkpoints should list");
    assert_eq!(checkpoints.len(), 1);
    assert_eq!(checkpoints[0].session_id, result.session.id);
    assert_eq!(
        checkpoints[0].task_id.as_deref(),
        Some(result.task.id.as_str())
    );
    assert!(
        checkpoints[0].summary.contains("Explain this file"),
        "the checkpoint should name the turn it precedes: {}",
        checkpoints[0].summary
    );
    // The position is the one before the turn's own events, so rewinding it
    // takes the user's prompt with it.
    assert!(checkpoints[0].conversation.last_event_seq > 0);
    assert!(checkpoints[0].user_message_id.is_some());
}

// The whole feature, end to end: a turn proposes a patch, the user applies it,
// and rewinding puts both the file and the conversation back.
#[test]
fn rewinding_a_turn_restores_an_applied_patch_and_the_conversation() {
    let repo = temp_repo("rewind-turn");
    write(&repo, "src/a.rs", "let a = 1;\n");
    let engine = engine_for(&repo);
    let mut adapter = MockModelAdapter::new(
        "DAMAIAN_EDIT_V1\nSUMMARY: Bump a\nFILE: src/a.rs\nSTATUS: modified\nCONTENT:\nlet a = 2;\nEND_FILE\nEND_PATCH\n",
    );

    let proposal = engine
        .edit_orchestrator
        .propose_edit(&repo, "Bump a", &[], &mut adapter)
        .expect("the edit should be proposed");
    engine
        .edit_orchestrator
        .apply_stored_patch(&repo, &proposal.patch.id, None, None, "tester", false)
        .expect("the patch should apply");
    assert_eq!(
        fs::read_to_string(repo.join("src/a.rs")).unwrap(),
        "let a = 2;\n"
    );

    let checkpoint = engine
        .checkpoint_store
        .list_checkpoints(&proposal.session.repository_id)
        .unwrap()
        .into_iter()
        .next()
        .expect("the edit turn should have a checkpoint");
    assert_eq!(
        checkpoint
            .files
            .iter()
            .map(|file| (file.path.as_str(), file.origin))
            .collect::<Vec<_>>(),
        vec![("src/a.rs", CheckpointOrigin::Patch)]
    );

    let result = engine
        .checkpoint_store
        .restore(
            &repo,
            &checkpoint,
            CheckpointRestoreOptions {
                files: true,
                conversation: true,
                only_path: None,
            },
            "tester",
        )
        .expect("the rewind should succeed");

    assert_eq!(result.restored_files, vec!["src/a.rs".to_string()]);
    assert!(result.conversation_restored);
    assert_eq!(
        fs::read_to_string(repo.join("src/a.rs")).unwrap(),
        "let a = 1;\n"
    );
    // The prompt that produced the wrong direction is out of the conversation,
    // and the log still has every byte of it.
    assert!(
        engine
            .session_store
            .read_messages(&proposal.session.id)
            .unwrap()
            .is_empty()
    );
}

#[test]
fn an_approved_command_that_changes_a_file_is_covered_by_the_turns_checkpoint() {
    let repo = temp_repo("command-coverage");
    write(&repo, "src/a.rs", "let a = 1;\n");
    git_repo(&repo);
    git(&repo, &["add", "-A"]);
    git(&repo, &["commit", "--quiet", "-m", "initial"]);
    let engine = engine_for(&repo);
    let mut adapter = MockModelAdapter::new(
        "DAMAIAN_COMMAND_V1\nCOMMAND: sed -i '' 's/1/2/' src/a.rs\nREASON: Bump the constant.\nEND_COMMAND\n",
    );

    let paused = ask(&engine, &repo, "Bump the constant with sed", &mut adapter);
    let proposal = paused
        .command_proposal
        .as_ref()
        .expect("the command should need approval");
    let mut answer = MockModelAdapter::new("Done.");
    let result = resume(&engine, &proposal.id, &mut answer);

    assert_eq!(
        fs::read_to_string(repo.join("src/a.rs")).unwrap(),
        "let a = 2;\n"
    );
    let checkpoint = engine
        .checkpoint_store
        .list_checkpoints(&result.session.repository_id)
        .unwrap()
        .into_iter()
        .next()
        .expect("the turn should have a checkpoint");
    let covered = checkpoint
        .files
        .iter()
        .find(|file| file.path == "src/a.rs")
        .expect("the file the command changed should be covered");
    assert_eq!(covered.origin, CheckpointOrigin::Command);
    assert!(checkpoint.command_effects_covered);

    let restore = engine
        .checkpoint_store
        .restore(
            &repo,
            &checkpoint,
            CheckpointRestoreOptions {
                files: true,
                conversation: false,
                only_path: None,
            },
            "tester",
        )
        .expect("the rewind should succeed");
    assert_eq!(restore.restored_files, vec!["src/a.rs".to_string()]);
    assert_eq!(
        fs::read_to_string(repo.join("src/a.rs")).unwrap(),
        "let a = 1;\n"
    );
}

// Most repositories Damaian is pointed at are git checkouts, but a turn in one
// that is not must still run. Coverage is then reported as absent rather than
// the turn failing.
#[test]
fn a_repository_without_git_still_runs_the_turn_and_reports_no_command_coverage() {
    let repo = temp_repo("no-git");
    write(&repo, "src/a.rs", "let a = 1;\n");
    let engine = engine_for(&repo);
    let mut adapter = MockModelAdapter::new(
        "DAMAIAN_COMMAND_V1\nCOMMAND: sed -i '' 's/1/2/' src/a.rs\nREASON: Bump the constant.\nEND_COMMAND\n",
    );

    let paused = ask(&engine, &repo, "Bump the constant with sed", &mut adapter);
    let proposal = paused
        .command_proposal
        .as_ref()
        .expect("the command should need approval");
    let mut answer = MockModelAdapter::new("Done.");
    let result = resume(&engine, &proposal.id, &mut answer);

    let checkpoint = engine
        .checkpoint_store
        .list_checkpoints(&result.session.repository_id)
        .unwrap()
        .into_iter()
        .next()
        .expect("the turn should still have a checkpoint");
    assert!(!checkpoint.command_effects_covered);
}

// Requirement 2: a checkpoint records the approvals the turn was waiting on.
// A turn paused on a command is the state a user is most likely to rewind out
// of, and "waiting on what?" is part of describing it.
#[test]
fn a_turn_paused_on_an_approval_records_it_on_the_checkpoint() {
    let repo = temp_repo("pending-approval");
    write(&repo, "src/a.rs", "let a = 1;\n");
    let engine = engine_for(&repo);
    let mut adapter = MockModelAdapter::new(
        "DAMAIAN_COMMAND_V1\nCOMMAND: sed -i '' 's/1/2/' src/a.rs\nREASON: Bump the constant.\nEND_COMMAND\n",
    );

    let paused = ask(&engine, &repo, "Bump the constant with sed", &mut adapter);

    let proposal = paused
        .command_proposal
        .as_ref()
        .expect("the command should need approval");
    let checkpoint = engine
        .checkpoint_store
        .read_checkpoint_for_task(&paused.session.repository_id, &paused.task.id)
        .unwrap()
        .expect("the turn should have a checkpoint");
    assert_eq!(checkpoint.pending_approvals.len(), 1);
    assert_eq!(checkpoint.pending_approvals[0].kind, "command");
    assert_eq!(checkpoint.pending_approvals[0].proposal_id, proposal.id);
}

// Once the turn resumes there is nothing pending, and a checkpoint that still
// claimed there was would describe a state that no longer exists.
#[test]
fn resuming_the_turn_clears_the_recorded_pending_approval() {
    let repo = temp_repo("pending-cleared");
    write(&repo, "src/a.rs", "let a = 1;\n");
    let engine = engine_for(&repo);
    let mut adapter = MockModelAdapter::new(
        "DAMAIAN_COMMAND_V1\nCOMMAND: sed -i '' 's/1/2/' src/a.rs\nREASON: Bump the constant.\nEND_COMMAND\n",
    );
    let paused = ask(&engine, &repo, "Bump the constant with sed", &mut adapter);
    let proposal_id = paused
        .command_proposal
        .as_ref()
        .expect("the command should need approval")
        .id
        .clone();

    let mut answer = MockModelAdapter::new("Done.");
    let result = resume(&engine, &proposal_id, &mut answer);

    let checkpoint = engine
        .checkpoint_store
        .read_checkpoint_for_task(&result.session.repository_id, &result.task.id)
        .unwrap()
        .expect("the turn should have a checkpoint");
    assert!(checkpoint.pending_approvals.is_empty());
}
