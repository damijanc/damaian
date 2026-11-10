//! An ordinary Code-mode turn continues while it makes progress, per
//! `docs/specs/107_visible_waiting_and_working/context.md` D3 (option A).
//!
//! The 16-round **Continue** card used to interrupt every ordinary session.
//! With spec 103's segment predicate and the default task budget, an ordinary
//! Code-mode turn crosses its round checkpoint on its own while the progress
//! digest advances, and still stops on the existing card when it stops making
//! progress or reaches a ceiling. Ask, Plan and Review keep the pause.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use workspace_engine::{
    CancelToken, ChatTurnResult, Config, ConfigOverlay, ConfigScope, MockModelAdapter, SessionMode,
    TaskStatus, ToolCall, TurnProgress, TurnSink, WorkspaceEngine,
};

static COUNTER: AtomicU64 = AtomicU64::new(1);

fn temp_repo(name: &str) -> PathBuf {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock should work")
        .as_nanos();
    let repo = std::env::temp_dir().join(format!(
        "damaian-ordinary-continuation-{name}-{now}-{}",
        COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir_all(repo.join("src")).expect("repository should be created");
    fs::write(repo.join("src/a.rs"), "fn main() {}\n").expect("a file to read");
    repo
}

fn config(repo: &Path) -> Config {
    Config {
        data_dir: repo.join(".damaian"),
        enable_index_watcher: false,
        ..Config::default()
    }
}

fn ask(
    engine: &WorkspaceEngine,
    repo: &Path,
    session: Option<&str>,
    prompt: &str,
    adapter: &mut MockModelAdapter,
) -> ChatTurnResult {
    let cancel = CancelToken::new();
    let mut on_token = |_token: &str| {};
    let mut on_progress = |_event: TurnProgress| {};
    let mut sink = TurnSink {
        on_token: &mut on_token,
        on_progress: &mut on_progress,
        cancel: &cancel,
        steering: None,
    };
    engine
        .chat_orchestrator
        .ask_with_session(repo, prompt, &[], session, adapter, &mut sink)
        .expect("the turn should run")
}

/// A read-only command the policy runs automatically, as a native tool call.
/// Each `index` names a different command, so every round adds a new typed
/// tool fact and the progress digest advances.
fn progressing_call(index: usize) -> ToolCall {
    ToolCall {
        id: format!("call_ls_{index}"),
        name: "run_command".to_string(),
        arguments_json: format!(r#"{{"command":"ls src/{index}","reason":"Look further."}}"#),
    }
}

/// The same read-only command every round, in the text envelope: each round
/// is an action round, and repeating it adds nothing to the progress digest.
fn repeating_round() -> String {
    "DAMAIAN_COMMAND_V1\nCOMMAND: ls src\nREASON: Look again.\nEND_COMMAND\n".to_string()
}

/// `rounds` scripted tool rounds followed by a plain answer.
fn turn_of(rounds: usize, call: fn(usize) -> ToolCall) -> MockModelAdapter {
    let mut responses: Vec<String> = (0..rounds).map(|_| String::new()).collect();
    let mut calls: Vec<Vec<ToolCall>> = (0..rounds).map(|index| vec![call(index)]).collect();
    responses.push("Done.".to_string());
    calls.push(Vec::new());
    MockModelAdapter::new_sequence_with_tool_calls(responses, calls)
}

fn progressing_turn(rounds: usize) -> MockModelAdapter {
    turn_of(rounds, progressing_call)
}

fn audit_events(repo: &Path, event_type: &str) -> Vec<serde_json::Value> {
    fs::read_to_string(repo.join(".damaian/audit/events.jsonl"))
        .unwrap_or_default()
        .lines()
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
        .filter(|event| event.get("eventType").and_then(|value| value.as_str()) == Some(event_type))
        .collect()
}

fn field<'a>(event: &'a serde_json::Value, name: &str) -> &'a str {
    event
        .get("fields")
        .and_then(|fields| fields.get(name))
        .or_else(|| event.get(name))
        .and_then(|value| value.as_str())
        .unwrap_or_default()
}

#[test]
fn forty_progressing_rounds_pause_zero_times() {
    let repo = temp_repo("forty");
    let engine = WorkspaceEngine::new(config(&repo));
    let mut adapter = progressing_turn(40);

    let result = ask(&engine, &repo, None, "Survey the source tree", &mut adapter);

    assert!(
        result.round_budget_proposal.is_none(),
        "a progressing ordinary turn never shows the Continue card"
    );
    assert_ne!(result.task.status, TaskStatus::WaitingForApproval);
    assert_eq!(
        adapter.requests.len(),
        41,
        "every round ran, then the answer"
    );
    assert_eq!(result.response.trim(), "Done.");
    // Two checkpoints crossed (16 and 32), each on progress.
    let continued = audit_events(&repo, "ordinary_continuation");
    assert_eq!(continued.len(), 2, "{continued:?}");
    assert!(audit_events(&repo, "ordinary_continuation_declined").is_empty());

    let _ = fs::remove_dir_all(&repo);
}

#[test]
fn a_turn_that_stops_making_progress_pauses_with_the_existing_card() {
    let repo = temp_repo("no-progress");
    let engine = WorkspaceEngine::new(config(&repo));
    let mut adapter = MockModelAdapter::new_sequence((0..20).map(|_| repeating_round()).collect());

    let result = ask(&engine, &repo, None, "Look at the source", &mut adapter);

    let proposal = result
        .round_budget_proposal
        .as_ref()
        .expect("no progress keeps the round-budget card");
    assert_eq!(result.task.status, TaskStatus::WaitingForApproval);
    assert_eq!(proposal.rounds_used, 16);
    assert_eq!(adapter.requests.len(), 16, "no call after the checkpoint");
    let declined = audit_events(&repo, "ordinary_continuation_declined");
    assert_eq!(declined.len(), 1, "{declined:?}");
    assert_eq!(field(&declined[0], "reason"), "no_progress");

    let _ = fs::remove_dir_all(&repo);
}

#[test]
fn ask_mode_still_pauses_at_sixteen_rounds() {
    let repo = temp_repo("ask-mode");
    let engine = WorkspaceEngine::new(config(&repo));
    let mut first = MockModelAdapter::new("Hello.");
    let session = ask(&engine, &repo, None, "Hello", &mut first).session.id;
    engine
        .session_store
        .set_session_mode(&session, SessionMode::Ask, "user")
        .expect("the mode should change");
    let mut adapter = progressing_turn(40);

    let result = ask(
        &engine,
        &repo,
        Some(&session),
        "Survey the source tree",
        &mut adapter,
    );

    let proposal = result
        .round_budget_proposal
        .as_ref()
        .expect("Ask mode keeps the 16-round pause, progress or not");
    assert_eq!(proposal.rounds_used, 16);
    assert_eq!(adapter.requests.len(), 16);
    assert!(audit_events(&repo, "ordinary_continuation").is_empty());

    let _ = fs::remove_dir_all(&repo);
}

#[test]
fn a_lowered_task_budget_stops_continuation_at_its_ceiling() {
    let repo = temp_repo("lowered");
    // A repository may lower the default task budget, never raise it.
    let mut config = config(&repo);
    let raise = ConfigOverlay::parse("worker_task_budget_action_rounds=200\n")
        .expect("the overlay should parse");
    let _ = config.apply_overlay_scoped(raise, ConfigScope::Repository);
    assert_eq!(
        config.task_budget_policy().default.action_rounds,
        Config::default().task_budget_policy().default.action_rounds,
        "a repository cannot raise the budget"
    );
    let lower = ConfigOverlay::parse("worker_task_budget_action_rounds=20\n")
        .expect("the overlay should parse");
    let _ = config.apply_overlay_scoped(lower, ConfigScope::Repository);
    assert_eq!(config.task_budget_policy().default.action_rounds, 20);
    let engine = WorkspaceEngine::new(config);
    let mut adapter = progressing_turn(40);

    let result = ask(&engine, &repo, None, "Survey the source tree", &mut adapter);

    // 16 rounds, one checkpoint crossed on progress with the 4 rounds left,
    // then the ceiling declines and the ordinary card takes over.
    let proposal = result
        .round_budget_proposal
        .as_ref()
        .expect("the ceiling ends in the existing card");
    assert_eq!(result.task.status, TaskStatus::WaitingForApproval);
    assert_eq!(proposal.rounds_used, 4);
    assert_eq!(adapter.requests.len(), 20);
    assert_eq!(audit_events(&repo, "ordinary_continuation").len(), 1);
    let declined = audit_events(&repo, "ordinary_continuation_declined");
    assert_eq!(declined.len(), 1, "{declined:?}");
    assert_eq!(field(&declined[0], "reason"), "action_rounds_exhausted");
    assert_eq!(field(&declined[0], "totalActionRounds"), "20");

    let _ = fs::remove_dir_all(&repo);
}
