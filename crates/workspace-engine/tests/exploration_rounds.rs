//! The exploration round allowance, per
//! `docs/specs/59_exploration_round_allowance/proposal.md` §5.1–§5.2.
//!
//! A round whose every dispatched call is `ActionEffect::ReadOnly` is an
//! *exploration round* and is charged to `agent_max_exploration_rounds`
//! instead of `agent_max_tool_rounds`, while that allowance lasts. Every other
//! round — including one with a command beside reads — is an action round.
//!
//! These tests walk the loop with native tool calls, so a round's class is
//! decided by `action_effect` exactly as production does. Command-only rounds
//! use a command the policy runs without approval (`ls src`), so the turn walks
//! the budget without stopping for a command approval.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use workspace_engine::{
    CancelToken, ChatTurnResult, Config, MockModelAdapter, ModelProviderConfig, PhaseKind,
    RoundBudgetDecision, TaskStatus, ToolCall, TurnProgress, TurnSink, WorkspaceEngine,
};

static COUNTER: AtomicU64 = AtomicU64::new(1);

fn temp_repo(name: &str) -> PathBuf {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock should work")
        .as_nanos();
    let repo = std::env::temp_dir().join(format!(
        "damaian-exploration-{name}-{now}-{}",
        COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir_all(repo.join("src")).expect("repository should be created");
    fs::write(repo.join("src/a.rs"), "fn one() {}\nfn two() {}\n").expect("a file to read");
    fs::write(repo.join("src/b.rs"), "fn bee() {}\n").expect("another file to read");
    fs::write(repo.join("src/c.rs"), "fn sea() {}\n").expect("a third file to read");
    repo
}

/// Read tools are native-only, so the active provider must opt in. `openai` is
/// `Config::default().model_provider`, so this makes `supports_native_tools()`
/// true without changing which provider is active.
fn native_tool_provider() -> ModelProviderConfig {
    ModelProviderConfig {
        id: "openai".to_string(),
        label: "OpenAI".to_string(),
        base_url: String::new(),
        api_key_env: String::new(),
        models: Vec::new(),
        supports_native_tools: true,
        max_output_tokens: None,
        context_token_budget: None,
        provider_reports_usage: true,
        price_per_million_input_tokens: None,
        price_per_million_output_tokens: None,
        price_per_million_cached_input_tokens: None,
        supports_explicit_cache_breakpoints: false,
    }
}

fn engine(repo: &Path, max_tool_rounds: u32, max_exploration_rounds: u32) -> WorkspaceEngine {
    let mut config = Config {
        data_dir: repo.join(".damaian"),
        agent_max_tool_rounds: max_tool_rounds,
        agent_max_exploration_rounds: max_exploration_rounds,
        // Throwaway repository: a watcher would only cost FSEvents registration.
        enable_index_watcher: false,
        ..Config::default()
    };
    config.model_providers.push(native_tool_provider());
    WorkspaceEngine::new(config)
}

/// Runs one turn, returning the result and every progress event it emitted. Call
/// counts come from `adapter.requests`.
fn run_turn(
    engine: &WorkspaceEngine,
    repo: &Path,
    prompt: &str,
    adapter: &mut MockModelAdapter,
) -> (ChatTurnResult, Vec<TurnProgress>) {
    let cancel = CancelToken::new();
    let mut on_token = |_token: &str| {};
    let mut progress = Vec::new();
    let result;
    {
        let mut on_progress = |event: TurnProgress| progress.push(event);
        let mut sink = TurnSink {
            on_token: &mut on_token,
            on_progress: &mut on_progress,
            cancel: &cancel,
            steering: None,
        };
        result = engine
            .chat_orchestrator
            .ask_with_session(repo, prompt, &[], None, adapter, &mut sink)
            .expect("the turn should run");
    }
    (result, progress)
}

fn resume_round_budget(
    engine: &WorkspaceEngine,
    proposal_id: &str,
    decision: RoundBudgetDecision,
    adapter: &mut MockModelAdapter,
) -> ChatTurnResult {
    resume_round_budget_result(engine, proposal_id, decision, adapter)
        .expect("the round-budget resume should run")
}

fn resume_round_budget_result(
    engine: &WorkspaceEngine,
    proposal_id: &str,
    decision: RoundBudgetDecision,
    adapter: &mut MockModelAdapter,
) -> workspace_engine::Result<ChatTurnResult> {
    let cancel = CancelToken::new();
    let mut on_token = |_token: &str| {};
    let mut on_progress = |_event: TurnProgress| {};
    let mut sink = TurnSink {
        on_token: &mut on_token,
        on_progress: &mut on_progress,
        cancel: &cancel,
        steering: None,
    };
    engine.chat_orchestrator.resume_round_budget(
        proposal_id,
        decision,
        "tester",
        adapter,
        &mut sink,
    )
}

fn resume_command_decision(
    engine: &WorkspaceEngine,
    proposal_id: &str,
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
        .resume_after_command_decision(proposal_id, true, "tester", adapter, &mut sink)
        .expect("the command decision should resume the turn")
}

fn tool_call(name: &str, arguments_json: String) -> ToolCall {
    static ID: AtomicU64 = AtomicU64::new(0);
    ToolCall {
        id: format!("call_{}", ID.fetch_add(1, Ordering::Relaxed)),
        name: name.to_string(),
        arguments_json,
    }
}

fn read_file(path: &str) -> ToolCall {
    tool_call("read_file", format!(r#"{{"path":"{path}"}}"#))
}

fn list_directory() -> ToolCall {
    tool_call("list_directory", "{}".to_string())
}

fn search_content(pattern: &str) -> ToolCall {
    tool_call("search_content", format!(r#"{{"pattern":"{pattern}"}}"#))
}

/// `ls src` is read-only to the command policy, so it runs without approval and
/// a round of it walks the budget like `round_budget.rs`'s `read_only_round`.
fn run_command(command: &str) -> ToolCall {
    tool_call(
        "run_command",
        format!(r#"{{"command":"{command}","reason":"Look."}}"#),
    )
}

/// A command that needs approval, so a turn can pause mid-round and resume.
fn command_needing_approval() -> String {
    "DAMAIAN_COMMAND_V1\nCOMMAND: sed -i '' 's/1/2/' src/a.rs\nREASON: Bump the constant.\nEND_COMMAND\n"
        .to_string()
}

fn pending_path(repo: &Path, proposal_id: &str) -> PathBuf {
    repo.join(".damaian/chat/pending")
        .join(format!("{proposal_id}.json"))
}

// ---------------------------------------------------------------------------
// Requirement 1: an all-read round is exploration and does not spend the
// action budget.
// ---------------------------------------------------------------------------

#[test]
fn ten_read_only_rounds_complete_past_the_action_budget() {
    let repo = temp_repo("ten-reads");
    let engine = engine(&repo, 8, 24);

    // Ten read-only rounds, cycling the three read tools, then an answer.
    let mut responses = Vec::new();
    let mut tool_calls = Vec::new();
    for index in 0..10 {
        responses.push(String::new());
        tool_calls.push(vec![match index % 3 {
            0 => read_file("src/a.rs"),
            1 => list_directory(),
            _ => search_content("fn"),
        }]);
    }
    responses.push("Done looking.".to_string());
    tool_calls.push(Vec::new());
    let mut adapter = MockModelAdapter::new_sequence_with_tool_calls(responses, tool_calls);

    let (result, _) = run_turn(&engine, &repo, "Look at the source", &mut adapter);

    assert_eq!(
        result.task.status,
        TaskStatus::Complete,
        "an all-read turn is charged to the exploration allowance, so the eight-round action budget does not stop it"
    );
    assert!(
        result.round_budget_proposal.is_none(),
        "the action budget was never reached"
    );
    assert_eq!(
        adapter.requests.len(),
        11,
        "ten read rounds plus the answer"
    );
}

// ---------------------------------------------------------------------------
// Requirement 2: a round with a command is an action round, whatever reads
// accompany it.
// ---------------------------------------------------------------------------

#[test]
fn mixed_rounds_pause_at_the_action_budget_counting_only_action_rounds() {
    let repo = temp_repo("mixed");
    let engine = engine(&repo, 8, 24);

    // Each round has a read beside the command, so a model stuck on reading is
    // not the only way to spend rounds. All eight are action rounds.
    let mut responses = Vec::new();
    let mut tool_calls = Vec::new();
    for index in 0..8 {
        responses.push(String::new());
        let path = match index % 3 {
            0 => "src/a.rs",
            1 => "src/b.rs",
            _ => "src/c.rs",
        };
        tool_calls.push(vec![read_file(path), run_command("ls src")]);
    }
    let mut adapter = MockModelAdapter::new_sequence_with_tool_calls(responses, tool_calls);

    let (result, _) = run_turn(&engine, &repo, "Look at the source", &mut adapter);

    assert_eq!(result.task.status, TaskStatus::WaitingForApproval);
    let proposal = result
        .round_budget_proposal
        .as_ref()
        .expect("the action budget pauses the turn");
    assert_eq!(
        proposal.rounds_used, 8,
        "only the eight command rounds are charged; the reads beside them are not separate rounds"
    );
    assert_eq!(adapter.requests.len(), 8);
}

#[test]
fn one_command_and_three_reads_charge_one_action_round() {
    let repo = temp_repo("one-mixed-round");
    // A budget of one round: if the mixed round were exploration, the action
    // budget would not be reached and the turn would keep going instead of
    // pausing after a single model call.
    let engine = engine(&repo, 1, 24);
    let mut adapter = MockModelAdapter::new_sequence_with_tool_calls(
        vec![String::new()],
        vec![vec![
            read_file("src/a.rs"),
            read_file("src/b.rs"),
            read_file("src/c.rs"),
            run_command("ls src"),
        ]],
    );

    let (result, _) = run_turn(&engine, &repo, "Look at the source", &mut adapter);

    assert_eq!(result.task.status, TaskStatus::WaitingForApproval);
    let proposal = result
        .round_budget_proposal
        .as_ref()
        .expect("the mixed round is an action round, so the budget is reached");
    assert_eq!(
        proposal.rounds_used, 1,
        "one command and three reads are one round, charged once to the action budget"
    );
    assert_eq!(
        adapter.requests.len(),
        1,
        "the pause follows the single mixed round"
    );
}

// ---------------------------------------------------------------------------
// Requirements 3 and 6: the allowance can be spent, and zero restores today.
// ---------------------------------------------------------------------------

#[test]
fn zero_exploration_allowance_reproduces_the_action_budget() {
    let repo = temp_repo("zero");
    let engine = engine(&repo, 8, 0);

    let mut adapter = MockModelAdapter::new_sequence_with_tool_calls(
        (0..8).map(|_| String::new()).collect(),
        (0..8).map(|_| vec![read_file("src/a.rs")]).collect(),
    );

    let (result, _) = run_turn(&engine, &repo, "Look at the source", &mut adapter);

    assert_eq!(result.task.status, TaskStatus::WaitingForApproval);
    let proposal = result
        .round_budget_proposal
        .as_ref()
        .expect("with no allowance every read round is an action round");
    assert_eq!(
        proposal.rounds_used, 8,
        "a zero allowance means the eight-round action budget is where the turn stops"
    );
    assert_eq!(adapter.requests.len(), 8);
}

#[test]
fn a_spent_allowance_charges_reads_as_action_rounds_without_pausing() {
    let repo = temp_repo("spent");
    // Two exploration rounds, then reads fall back to the action budget until
    // it is reached — eight action rounds later.
    let engine = engine(&repo, 8, 2);

    // Ten read rounds: two exploration and eight action. The turn must not
    // pause when the allowance is exhausted (after two), only at the action
    // budget (after ten).
    let mut adapter = MockModelAdapter::new_sequence_with_tool_calls(
        (0..10).map(|_| String::new()).collect(),
        (0..10).map(|_| vec![read_file("src/a.rs")]).collect(),
    );

    let (result, _) = run_turn(&engine, &repo, "Look at the source", &mut adapter);

    assert_eq!(
        result.task.status,
        TaskStatus::WaitingForApproval,
        "exhausting the allowance never stops a turn by itself"
    );
    let proposal = result
        .round_budget_proposal
        .as_ref()
        .expect("the action budget is what finally pauses the turn");
    assert_eq!(proposal.rounds_used, 8);
    assert_eq!(
        adapter.requests.len(),
        10,
        "two exploration rounds plus eight action rounds, then the pause"
    );
}

// ---------------------------------------------------------------------------
// Requirement 5: the allowance is per turn and survives every resume.
// ---------------------------------------------------------------------------

#[test]
fn the_allowance_survives_a_round_budget_continue() {
    let repo = temp_repo("continue");
    // Two exploration rounds and two action rounds, then Continue grants two
    // more action rounds. The reads after the pause must be action rounds
    // because the allowance was not refreshed.
    let engine = engine(&repo, 2, 2);
    let mut adapter = MockModelAdapter::new_sequence_with_tool_calls(
        vec![String::new()],
        vec![vec![read_file("src/a.rs")]],
    );

    let (paused, _) = run_turn(&engine, &repo, "Look at the source", &mut adapter);
    let first = paused
        .round_budget_proposal
        .as_ref()
        .expect("the action budget pauses the turn");
    assert_eq!(first.rounds_used, 2);
    assert_eq!(
        adapter.requests.len(),
        4,
        "two exploration rounds plus two action rounds"
    );

    let resumed = resume_round_budget(
        &engine,
        &first.id,
        RoundBudgetDecision::Continue,
        &mut adapter,
    );

    let second = resumed
        .round_budget_proposal
        .as_ref()
        .expect("the resumed turn reaches the extended action budget");
    assert_eq!(
        second.rounds_used, 4,
        "the two reads after Continue are action rounds, so the allowance was not refreshed"
    );
    assert_eq!(
        adapter.requests.len(),
        6,
        "two more action rounds after the continuation, then the pause"
    );
}

#[test]
fn the_allowance_survives_a_command_approval() {
    let repo = temp_repo("approval");
    // One exploration round, then a command needing approval. On resume the
    // exploration count is carried, so the next read is an action round and the
    // two-round action budget is reached after one more call.
    let engine = engine(&repo, 2, 1);
    let mut adapter = MockModelAdapter::new_sequence_with_tool_calls(
        vec![String::new(), command_needing_approval(), String::new()],
        vec![
            vec![read_file("src/a.rs")],
            Vec::new(),
            vec![read_file("src/b.rs")],
        ],
    );

    let (paused, _) = run_turn(&engine, &repo, "Look at the source", &mut adapter);
    let proposal = paused
        .command_proposal
        .as_ref()
        .expect("the mutating command needs approval");
    assert_eq!(adapter.requests.len(), 2, "a read, then the command");

    let resumed = resume_command_decision(&engine, &proposal.id, &mut adapter);

    let round_budget = resumed
        .round_budget_proposal
        .as_ref()
        .expect("the resumed read reaches the action budget");
    assert_eq!(
        round_budget.rounds_used, 2,
        "the command round and the read after it are the two action rounds"
    );
    assert_eq!(
        adapter.requests.len(),
        3,
        "if the approval had refreshed the allowance, the read after it would have been exploration and needed another call"
    );
}

// ---------------------------------------------------------------------------
// Persistence: an older pending turn loads with a zero allowance.
// ---------------------------------------------------------------------------

#[test]
fn a_pending_turn_without_exploration_rounds_still_loads() {
    let repo = temp_repo("old-pending");
    // One exploration round, then one action round reaches the budget, so the
    // saved turn carries a non-zero exploration count worth stripping.
    let engine = engine(&repo, 1, 1);
    let mut adapter = MockModelAdapter::new_sequence_with_tool_calls(
        vec![String::new(), String::new()],
        vec![vec![read_file("src/a.rs")], vec![read_file("src/b.rs")]],
    );

    let (paused, _) = run_turn(&engine, &repo, "Look at the source", &mut adapter);
    let proposal = paused
        .round_budget_proposal
        .as_ref()
        .expect("the action budget pauses the turn");

    let path = pending_path(&repo, &proposal.id);
    let text = fs::read_to_string(&path).expect("the pending turn is on disk");
    let mut json: serde_json::Value =
        serde_json::from_str(&text).expect("the pending turn is valid JSON");
    assert!(
        json.get("exploration_rounds").is_some(),
        "the pending turn must persist the exploration count: {text}"
    );
    json.as_object_mut()
        .expect("a pending turn is an object")
        .remove("exploration_rounds");
    fs::write(&path, serde_json::to_string(&json).expect("valid JSON")).expect("rewrite the turn");

    // A turn written before this field existed must still load, with the
    // allowance defaulting to zero, rather than failing to parse.
    let resumed = resume_round_budget_result(
        &engine,
        &proposal.id,
        RoundBudgetDecision::Continue,
        &mut adapter,
    );
    assert!(
        resumed.is_ok(),
        "a pending turn without exploration_rounds must load: {:?}",
        resumed.err()
    );
}

// ---------------------------------------------------------------------------
// C5: the progress phase's round is the action count.
// ---------------------------------------------------------------------------

#[test]
fn phase_round_is_the_action_count() {
    let repo = temp_repo("phase");
    let engine = engine(&repo, 8, 24);
    let mut adapter = MockModelAdapter::new_sequence_with_tool_calls(
        vec![
            String::new(),
            String::new(),
            String::new(),
            "Done.".to_string(),
        ],
        vec![
            vec![read_file("src/a.rs")],
            vec![list_directory()],
            vec![search_content("fn")],
            Vec::new(),
        ],
    );

    let (result, progress) = run_turn(&engine, &repo, "Look at the source", &mut adapter);
    assert_eq!(result.task.status, TaskStatus::Complete);

    let rounds: Vec<u32> = progress
        .iter()
        .filter_map(|event| match event {
            TurnProgress::Phase(phase) if phase.kind != PhaseKind::Context => Some(phase.round),
            _ => None,
        })
        .collect();
    assert!(!rounds.is_empty(), "the turn should report progress");
    // `TurnPhase.round` is 1-based, so an action count of zero displays as 1 and
    // never advances for an all-read turn.
    assert!(
        rounds.iter().all(|&round| round == 1),
        "an all-read turn must never advance the action count: {rounds:?}"
    );
}
