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
use std::process::Command;
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

fn read_file_range(path: &str, start_line: usize, end_line: usize) -> ToolCall {
    tool_call(
        "read_file",
        format!(r#"{{"path":"{path}","start_line":{start_line},"end_line":{end_line}}}"#),
    )
}

fn read_file_in_worktree(path: &str, worktree: &str) -> ToolCall {
    tool_call(
        "read_file",
        format!(r#"{{"path":"{path}","worktree":"{worktree}"}}"#),
    )
}

fn read_git_status() -> ToolCall {
    tool_call("read_git_status", "{}".to_string())
}

fn read_git_diff(arguments_json: String) -> ToolCall {
    tool_call("read_git_diff", arguments_json)
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

/// A committed repository, so the git read tools and worktree resolution have
/// something to read.
fn temp_git_repo(name: &str) -> PathBuf {
    let repo = temp_repo(name);
    git(&repo, &["init", "-q", "--initial-branch=main"]);
    git(&repo, &["config", "user.name", "Damaian Test"]);
    git(&repo, &["config", "user.email", "test@example.invalid"]);
    git(&repo, &["config", "commit.gpgsign", "false"]);
    git(&repo, &["add", "."]);
    git(&repo, &["commit", "-q", "-m", "initial"]);
    repo
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

fn add_worktree(repo: &Path, name: &str) {
    git(
        repo,
        &[
            "worktree",
            "add",
            "-q",
            &format!(".claude/worktrees/{name}"),
            "-b",
            &format!("wt-{name}"),
        ],
    );
}

// ---------------------------------------------------------------------------
// Requirement 1: an all-read round is exploration and does not spend the
// action budget.
// ---------------------------------------------------------------------------

#[test]
fn ten_read_only_rounds_complete_past_the_action_budget() {
    let repo = temp_repo("ten-reads");
    let engine = engine(&repo, 8, 24);

    // Ten read-only rounds, each reading something new, then an answer. A
    // repeated call would be charged to the action budget (spec 59 §5.3), so
    // every pattern differs.
    let mut responses = Vec::new();
    let mut tool_calls = Vec::new();
    for index in 0..10 {
        responses.push(String::new());
        tool_calls.push(vec![search_content(&format!("fn_{index}"))]);
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
    // budget (after ten). Cycling the three files keeps the first two rounds
    // new; after that the reads repeat, which is harmless here because they are
    // action rounds either way — the allowance is already spent.
    let mut adapter = MockModelAdapter::new_sequence_with_tool_calls(
        (0..10).map(|_| String::new()).collect(),
        (0..10)
            .map(|index| {
                vec![read_file(match index % 3 {
                    0 => "src/a.rs",
                    1 => "src/b.rs",
                    _ => "src/c.rs",
                })]
            })
            .collect(),
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
    // Alternating files: the first two rounds are new exploration, the next two
    // repeat them and are action rounds. The reads after *Continue* are action
    // rounds because the allowance was not refreshed.
    let mut adapter = MockModelAdapter::new_sequence_with_tool_calls(
        (0..6).map(|_| String::new()).collect(),
        (0..6)
            .map(|index| {
                vec![read_file(if index % 2 == 0 {
                    "src/a.rs"
                } else {
                    "src/b.rs"
                })]
            })
            .collect(),
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

// ---------------------------------------------------------------------------
// Requirement 4: a read that repeats an earlier call is an action round.
// ---------------------------------------------------------------------------

#[test]
fn identical_reads_spend_the_action_budget_after_one_exploration_round() {
    let repo = temp_repo("repeat");
    let engine = engine(&repo, 8, 24);

    // Round 1 is new, so it is an exploration round. Rounds 2–9 repeat it, so
    // all eight are action rounds and the budget is reached. The tenth call is
    // the forced final one after *Answer now*.
    let mut adapter = MockModelAdapter::new_sequence_with_tool_calls(
        vec![String::new(); 10],
        vec![vec![read_file_range("src/a.rs", 1, 1)]; 10],
    );

    let (paused, _) = run_turn(&engine, &repo, "Look at the source", &mut adapter);

    assert_eq!(paused.task.status, TaskStatus::WaitingForApproval);
    let proposal = paused
        .round_budget_proposal
        .as_ref()
        .expect("the repeated reads spend the action budget");
    assert_eq!(proposal.rounds_used, 8);
    assert_eq!(
        adapter.requests.len(),
        9,
        "one exploration round plus eight action rounds, then the pause"
    );

    let exhausted = resume_round_budget(
        &engine,
        &proposal.id,
        RoundBudgetDecision::AnswerNow,
        &mut adapter,
    );

    assert_eq!(
        exhausted.task.status,
        TaskStatus::ToolBudgetExhausted,
        "the forced final call still asks for a read, so the turn ends at the budget"
    );
    assert_eq!(
        adapter.requests.len(),
        10,
        "one exploration round, eight action rounds and the forced final call"
    );
}

#[test]
fn a_continued_line_range_is_not_a_repeat() {
    let repo = temp_repo("continued-range");
    let long: String = (1..=500).map(|line| format!("line {line}\n")).collect();
    fs::write(repo.join("src/long.rs"), long).expect("a file long enough to continue");
    // One action round: if the second range counted as a repeat, it would be an
    // action round and pause the turn before the answer.
    let engine = engine(&repo, 1, 24);
    let mut adapter = MockModelAdapter::new_sequence_with_tool_calls(
        vec![String::new(), String::new(), "Done.".to_string()],
        vec![
            vec![read_file_range("src/long.rs", 1, 400)],
            // Continuing a truncated read is how a window is extended, not a
            // loop (§5.3).
            vec![read_file_range("src/long.rs", 401, 468)],
            Vec::new(),
        ],
    );

    let (result, _) = run_turn(&engine, &repo, "Look at the source", &mut adapter);

    assert_eq!(
        result.task.status,
        TaskStatus::Complete,
        "the second window reads new lines, so it is not a repeat"
    );
    assert!(result.round_budget_proposal.is_none());
    assert_eq!(adapter.requests.len(), 3);
}

#[test]
fn two_identical_calls_in_one_round_make_it_an_action_round() {
    let repo = temp_repo("same-round");
    let engine = engine(&repo, 1, 24);
    let mut adapter = MockModelAdapter::new_sequence_with_tool_calls(
        vec![String::new()],
        vec![vec![read_file("src/a.rs"), read_file("src/a.rs")]],
    );

    let (result, _) = run_turn(&engine, &repo, "Look at the source", &mut adapter);

    assert_eq!(result.task.status, TaskStatus::WaitingForApproval);
    let proposal = result
        .round_budget_proposal
        .as_ref()
        .expect("two identical reads in one round are one action round");
    assert_eq!(proposal.rounds_used, 1);
    assert_eq!(adapter.requests.len(), 1);
}

#[test]
fn the_same_path_in_two_checkouts_is_not_a_repeat() {
    let repo = temp_git_repo("two-checkouts");
    add_worktree(&repo, "a");
    let engine = engine(&repo, 1, 24);
    let mut adapter = MockModelAdapter::new_sequence_with_tool_calls(
        vec![String::new(), String::new(), "Done.".to_string()],
        vec![
            vec![read_file("src/a.rs")],
            vec![read_file_in_worktree("src/a.rs", ".claude/worktrees/a")],
            Vec::new(),
        ],
    );

    let (result, _) = run_turn(&engine, &repo, "Look at the source", &mut adapter);

    assert_eq!(
        result.task.status,
        TaskStatus::Complete,
        "the same path in another checkout is a different call"
    );
    assert_eq!(adapter.requests.len(), 3);
}

#[test]
fn two_spellings_of_the_same_worktree_are_a_repeat() {
    let repo = temp_git_repo("worktree-spellings");
    add_worktree(&repo, "a");
    // One action round: if the spellings resolved differently, the second read
    // would be exploration and the turn would continue.
    let engine = engine(&repo, 1, 24);
    let mut adapter = MockModelAdapter::new_sequence_with_tool_calls(
        vec![String::new(); 2],
        vec![
            vec![read_file_in_worktree("src/a.rs", ".claude/worktrees/a")],
            vec![read_file_in_worktree("src/a.rs", ".claude/worktrees/a/")],
        ],
    );

    let (result, _) = run_turn(&engine, &repo, "Look at the source", &mut adapter);

    assert_eq!(result.task.status, TaskStatus::WaitingForApproval);
    let proposal = result
        .round_budget_proposal
        .as_ref()
        .expect("both spellings resolve to the same registered worktree");
    assert_eq!(proposal.rounds_used, 1);
    assert_eq!(adapter.requests.len(), 2);
}

#[test]
fn read_git_diff_with_the_same_paths_in_a_different_order_is_a_repeat() {
    let repo = temp_git_repo("diff-order");
    let engine = engine(&repo, 1, 24);
    let mut adapter = MockModelAdapter::new_sequence_with_tool_calls(
        vec![String::new(); 2],
        vec![
            vec![read_git_diff(
                r#"{"paths":["src/a.rs","src/b.rs"]}"#.to_string(),
            )],
            vec![read_git_diff(
                r#"{"paths":["src/b.rs","src/a.rs"]}"#.to_string(),
            )],
        ],
    );

    let (result, _) = run_turn(&engine, &repo, "Look at the source", &mut adapter);

    assert_eq!(result.task.status, TaskStatus::WaitingForApproval);
    let proposal = result
        .round_budget_proposal
        .as_ref()
        .expect("path order does not change the diff");
    assert_eq!(proposal.rounds_used, 1);
    assert_eq!(adapter.requests.len(), 2);
}

#[test]
fn read_git_diff_with_a_different_option_is_not_a_repeat() {
    let variants = [r#""base":"main""#, r#""stat":true"#, r#""staged":true"#];
    for (index, extra) in variants.into_iter().enumerate() {
        let repo = temp_git_repo(&format!("diff-option-{index}"));
        let engine = engine(&repo, 1, 24);
        let mut adapter = MockModelAdapter::new_sequence_with_tool_calls(
            vec![String::new(), String::new(), "Done.".to_string()],
            vec![
                vec![read_git_diff(r#"{"paths":["src/a.rs"]}"#.to_string())],
                vec![read_git_diff(format!(
                    r#"{{"paths":["src/a.rs"],{extra}}}"#
                ))],
                Vec::new(),
            ],
        );

        let (result, _) = run_turn(&engine, &repo, "Look at the source", &mut adapter);

        assert_eq!(
            result.task.status,
            TaskStatus::Complete,
            "a different {extra} changes what the diff returns, so it is not a repeat"
        );
        assert_eq!(adapter.requests.len(), 3, "variant {extra}");
    }
}

#[test]
fn read_git_status_after_a_command_is_not_a_repeat() {
    let repo = temp_git_repo("status-after-command");
    // Two action rounds: the command, then the repeated status after the write
    // clears the seen-call set.
    let engine = engine(&repo, 2, 24);
    let mut adapter = MockModelAdapter::new_sequence_with_tool_calls(
        vec![String::new(); 5],
        vec![
            vec![read_git_status()],
            vec![run_command("ls src")],
            vec![read_git_status()],
            vec![read_git_status()],
            Vec::new(),
        ],
    );

    let (result, _) = run_turn(&engine, &repo, "Look at the source", &mut adapter);

    assert_eq!(result.task.status, TaskStatus::WaitingForApproval);
    let proposal = result
        .round_budget_proposal
        .as_ref()
        .expect("the second status after the command repeats, so it is an action round");
    assert_eq!(
        proposal.rounds_used, 2,
        "the command and the repeated status are the two action rounds"
    );
    assert_eq!(
        adapter.requests.len(),
        4,
        "checking status again after a write is the ordinary verify step"
    );
}

#[test]
fn read_git_status_repeated_with_only_reads_between_is_a_repeat() {
    let repo = temp_git_repo("status-reads");
    // One action round: the status after only reads is a repeat, so it reaches
    // the budget; if it were exploration the turn would continue to the answer.
    let engine = engine(&repo, 1, 24);
    let mut adapter = MockModelAdapter::new_sequence_with_tool_calls(
        vec![
            String::new(),
            String::new(),
            String::new(),
            "Done.".to_string(),
        ],
        vec![
            vec![read_git_status()],
            vec![read_file("src/a.rs")],
            vec![read_git_status()],
            Vec::new(),
        ],
    );

    let (result, _) = run_turn(&engine, &repo, "Look at the source", &mut adapter);

    assert_eq!(result.task.status, TaskStatus::WaitingForApproval);
    let proposal = result
        .round_budget_proposal
        .as_ref()
        .expect("a read loop with no write between is caught");
    assert_eq!(proposal.rounds_used, 1);
    assert_eq!(adapter.requests.len(), 3);
}

#[test]
fn different_read_arguments_are_not_repeats() {
    let cases: Vec<(ToolCall, ToolCall)> = vec![
        (read_file("src/a.rs"), read_file("src/b.rs")),
        (search_content("fn"), search_content("bee")),
        (
            search_content("fn"),
            tool_call(
                "search_content",
                r#"{"pattern":"fn","path_glob":"src/*.rs"}"#.to_string(),
            ),
        ),
        (
            search_content("fn"),
            tool_call(
                "search_content",
                r#"{"pattern":"fn","max_matches":5}"#.to_string(),
            ),
        ),
        (
            list_directory(),
            tool_call("list_directory", r#"{"depth":2}"#.to_string()),
        ),
        (
            tool_call("search_codebase", r#"{"query":"one"}"#.to_string()),
            tool_call("search_codebase", r#"{"query":"two"}"#.to_string()),
        ),
    ];
    for (index, (first, second)) in cases.into_iter().enumerate() {
        let repo = temp_repo(&format!("different-args-{index}"));
        let engine = engine(&repo, 1, 24);
        let mut adapter = MockModelAdapter::new_sequence_with_tool_calls(
            vec![String::new(), String::new(), "Done.".to_string()],
            vec![vec![first], vec![second], Vec::new()],
        );

        let (result, _) = run_turn(&engine, &repo, "Look at the source", &mut adapter);

        assert_eq!(
            result.task.status,
            TaskStatus::Complete,
            "case {index}: a different argument reads something else, so it is not a repeat"
        );
        assert_eq!(adapter.requests.len(), 3, "case {index}");
    }
}
