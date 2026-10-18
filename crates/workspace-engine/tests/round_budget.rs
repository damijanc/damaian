//! The round-budget pause, per
//! `docs/specs/61_continue_the_stopped_turn/proposal.md` §5.1.
//!
//! When a turn reaches its round limit with no token ceiling to continue
//! under, it pauses as a `round_budget` approval *before* the forced final
//! call, instead of spending that call on text nobody sees. The forced call is
//! made only when the user chooses *Answer now* (Task 2). This file holds the
//! rule the whole feature exists to hold: no model call is made between
//! reaching the limit and the user's choice.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use workspace_engine::{
    CancelToken, ChatTurnResult, Config, MockModelAdapter, TaskStatus, TurnProgress, TurnSink,
    WorkspaceEngine,
};

static COUNTER: AtomicU64 = AtomicU64::new(1);

fn temp_repo(name: &str) -> PathBuf {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock should work")
        .as_nanos();
    let repo = std::env::temp_dir().join(format!(
        "damaian-roundbudget-{name}-{now}-{}",
        COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir_all(repo.join("src")).expect("repository should be created");
    fs::write(repo.join("src/a.rs"), "fn main() {}\n").expect("a file to read");
    repo
}

fn engine(repo: &Path, rounds: u32, ceiling: Option<u64>) -> WorkspaceEngine {
    WorkspaceEngine::new(Config {
        data_dir: repo.join(".damaian"),
        agent_max_tool_rounds: rounds,
        agent_max_task_tokens: ceiling,
        enable_index_watcher: false,
        ..Config::default()
    })
}

fn ask(
    engine: &WorkspaceEngine,
    repo: &Path,
    prompt: &str,
    adapter: &mut MockModelAdapter,
) -> (ChatTurnResult, usize) {
    let cancel = CancelToken::new();
    let mut on_token = |_token: &str| {};
    let mut on_progress = |_event: TurnProgress| {};
    let mut sink = TurnSink {
        on_token: &mut on_token,
        on_progress: &mut on_progress,
        cancel: &cancel,
        steering: None,
    };
    let result = engine
        .chat_orchestrator
        .ask_with_session(repo, prompt, &[], None, adapter, &mut sink)
        .expect("the turn should run");
    let calls = adapter.requests.len();
    (result, calls)
}

/// A read-only command envelope the policy runs automatically, so a turn of it
/// walks the round budget without ever pausing for a command approval.
fn read_only_round() -> String {
    "DAMAIAN_COMMAND_V1\nCOMMAND: ls src\nREASON: Look again.\nEND_COMMAND\n".to_string()
}

fn audit_events(repo: &Path) -> String {
    fs::read_to_string(repo.join(".damaian/audit/events.jsonl")).unwrap_or_default()
}

fn pending_file(repo: &Path, proposal_id: &str) -> String {
    fs::read_to_string(
        repo.join(".damaian/chat/pending")
            .join(format!("{proposal_id}.json")),
    )
    .expect("the pending turn should be on disk")
}

#[test]
fn a_turn_that_hits_the_limit_wants_a_tool_and_pauses() {
    let repo = temp_repo("pauses");
    let engine = engine(&repo, 8, None);
    // Eight read-only rounds, then the model would have asked for a ninth. The
    // turn pauses before making that ninth call, so the ninth response is never
    // consumed; it is scripted only to make the "still wants a tool" state real.
    let mut adapter =
        MockModelAdapter::new_sequence((0..8).map(|_| read_only_round()).collect::<Vec<_>>());
    let (result, calls) = ask(&engine, &repo, "Look at the source", &mut adapter);

    assert_eq!(
        result.task.status,
        TaskStatus::WaitingForApproval,
        "the turn pauses rather than ending"
    );
    assert_eq!(
        calls, 8,
        "the forced final call must not be made before the choice"
    );
    let proposal = result
        .round_budget_proposal
        .as_ref()
        .expect("the pause surfaces a round-budget proposal");
    assert_eq!(proposal.rounds_used, 8);
    assert_eq!(proposal.rounds_total, 8);
    assert_eq!(proposal.rounds_granted, 0);
    assert!(!proposal.at_hard_cap);

    let pending = engine
        .session_store
        .pending_approval_for(&result.session.id, &result.task.id)
        .expect("the link should read")
        .expect("the task is waiting on an approval");
    assert_eq!(pending.kind, "round_budget");
    assert_eq!(pending.proposal_id, proposal.id);

    let json: serde_json::Value =
        serde_json::from_str(&pending_file(&repo, &proposal.id)).expect("valid pending turn");
    assert!(
        json.get("messages")
            .is_some_and(|m| !m.as_array().unwrap().is_empty())
    );
}

#[test]
fn no_round_budget_pause_when_the_model_answers_at_the_forced_call() {
    // §5.1 moves the forced call to the *Answer now* path, so a turn that would
    // have been answered by the forced call now simply completes before the
    // limit. The complement of `a_turn_that_hits_the_limit_wants_a_tool_and_pauses`:
    // a model that answers instead of requesting a tool never pauses.
    let repo = temp_repo("answers");
    let engine = engine(&repo, 8, None);
    let mut adapter = MockModelAdapter::new_sequence(vec![
        read_only_round(),
        read_only_round(),
        "Done looking.".to_string(),
    ]);
    let (result, calls) = ask(&engine, &repo, "Look at the source", &mut adapter);

    assert_eq!(result.task.status, TaskStatus::Complete);
    assert!(result.round_budget_proposal.is_none());
    assert_eq!(calls, 3, "two read-only rounds, then an answer");

    let pending = engine
        .session_store
        .pending_approval_for(&result.session.id, &result.task.id)
        .expect("the link should read");
    assert!(pending.is_none(), "a completed turn waits on nothing");
    let pending_dir = repo.join(".damaian/chat/pending");
    if pending_dir.exists() {
        assert_eq!(
            fs::read_dir(&pending_dir)
                .expect("pending dir should list")
                .count(),
            0,
            "no pending turn file should be written"
        );
    }
}

#[test]
fn no_round_budget_pause_with_a_token_ceiling_set() {
    // Requirement 8: a token ceiling keeps #47's automatic continuation, so the
    // round cap is not the end and no round-budget pause fires.
    let repo = temp_repo("ceiling");
    let engine = engine(&repo, 2, Some(10_000_000));
    let command = read_only_round();
    let mut adapter = MockModelAdapter::new_sequence(vec![
        command.clone(),
        command.clone(),
        command,
        "Done looking.".to_string(),
    ]);
    let (result, calls) = ask(&engine, &repo, "Look at the source", &mut adapter);

    assert_eq!(result.task.status, TaskStatus::Complete);
    assert!(result.round_budget_proposal.is_none());
    assert_eq!(calls, 4, "two rounds, one continuation, one answer");
    assert!(
        audit_events(&repo).contains("turn_continued"),
        "the continuation must still be recorded"
    );
}

#[test]
fn a_paused_round_budget_persists_messages_round_and_turn_options() {
    let repo = temp_repo("persists");
    let engine = engine(&repo, 8, None);
    let mut adapter =
        MockModelAdapter::new_sequence((0..8).map(|_| read_only_round()).collect::<Vec<_>>());
    let (result, _) = ask(&engine, &repo, "Look at the source", &mut adapter);

    let proposal = result
        .round_budget_proposal
        .as_ref()
        .expect("the pause surfaces a proposal");
    let json: serde_json::Value =
        serde_json::from_str(&pending_file(&repo, &proposal.id)).expect("valid pending turn");

    let messages = json
        .get("messages")
        .and_then(|m| m.as_array())
        .expect("messages should be saved");
    assert!(
        messages.len() > 1,
        "the conversation, not just the system prompt, should be saved"
    );
    assert_eq!(
        json.get("round").and_then(|r| r.as_u64()),
        Some(8),
        "the round that hit the limit should be saved"
    );
    assert!(
        json.get("turn_options").is_some(),
        "turn options should be saved"
    );
    let round_budget = json
        .get("round_budget")
        .expect("the round-budget pause should be saved");
    assert_eq!(
        round_budget.get("rounds_used").and_then(|r| r.as_u64()),
        Some(8)
    );
    assert_eq!(
        round_budget.get("rounds_granted").and_then(|r| r.as_u64()),
        Some(0)
    );
}
