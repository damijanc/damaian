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
    CancelToken, ChatTurnResult, ClientError, Config, MockModelAdapter, PhaseKind,
    RoundBudgetDecision, TaskStatus, ToolCall, TurnProgress, TurnSink, WorkspaceEngine,
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

/// The `turn_budget_extended` audit records, parsed in order, so a test can
/// assert each continuation's round, delta and running total.
fn turn_budget_extended_events(repo: &Path) -> Vec<serde_json::Value> {
    audit_events(repo)
        .lines()
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
        .filter(|event| {
            event.get("eventType").and_then(|value| value.as_str()) == Some("turn_budget_extended")
        })
        .collect()
}

fn pending_file(repo: &Path, proposal_id: &str) -> String {
    fs::read_to_string(
        repo.join(".damaian/chat/pending")
            .join(format!("{proposal_id}.json")),
    )
    .expect("the pending turn should be on disk")
}

/// A mutating command the policy pauses for approval, so a test can park a
/// `command` pending turn and try the wrong resume entry point against it.
fn command_round() -> String {
    "DAMAIAN_COMMAND_V1\nCOMMAND: sed -i '' 's/1/2/' src/a.rs\nREASON: Bump the constant.\nEND_COMMAND\n"
        .to_string()
}

fn resume_round_budget(
    engine: &WorkspaceEngine,
    proposal_id: &str,
    decision: RoundBudgetDecision,
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
        .resume_round_budget(proposal_id, decision, "tester", adapter, &mut sink)
        .expect("the round-budget resume should run")
}

/// Like `resume_round_budget`, but returns the result so a test can assert a
/// refusal (e.g. *Continue* at the hard cap).
fn resume_round_budget_attempt(
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
    let mut adapter = MockModelAdapter::new("Done.");
    engine.chat_orchestrator.resume_after_command_decision(
        proposal_id,
        true,
        "tester",
        &mut adapter,
        &mut sink,
    )
}

fn resume_fallback_decision(
    engine: &WorkspaceEngine,
    proposal_id: &str,
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
    let mut adapter = MockModelAdapter::new("Done.");
    engine.chat_orchestrator.resume_after_fallback_decision(
        proposal_id,
        true,
        "tester",
        &mut adapter,
        &mut sink,
    )
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

#[test]
fn continue_resumes_the_same_task_with_more_rounds() {
    let repo = temp_repo("continue");
    let engine = engine(&repo, 8, None);
    // Eight read-only rounds, then the answer the resumed turn produces. The
    // ninth response is never consumed until *Continue* re-enters the turn.
    let responses: Vec<String> = (0..8)
        .map(|_| read_only_round())
        .chain(std::iter::once("Done looking.".to_string()))
        .collect();
    let mut adapter = MockModelAdapter::new_sequence(responses);
    let (paused, calls) = ask(&engine, &repo, "Look at the source", &mut adapter);
    assert_eq!(calls, 8, "the turn pauses before its ninth call");
    let proposal = paused
        .round_budget_proposal
        .as_ref()
        .expect("the pause surfaces a proposal");

    let resumed = resume_round_budget(
        &engine,
        &proposal.id,
        RoundBudgetDecision::Continue,
        &mut adapter,
    );

    assert_eq!(
        resumed.task.id, paused.task.id,
        "a continued turn is the same task, never a new one"
    );
    assert_eq!(
        adapter.requests.len(),
        9,
        "the next model request is the ninth — the seeded budget must not re-pause"
    );
    assert_eq!(resumed.task.status, TaskStatus::Complete);

    // The ninth request replays the saved messages, so it still carries the
    // user's original request and the earlier tool results (C6).
    let ninth = &adapter.requests[8];
    let replayed = ninth
        .messages
        .iter()
        .map(|message| message.content.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        replayed.contains("Look at the source"),
        "the user's original request must be replayed: {replayed}"
    );
    assert!(
        replayed.contains("a.rs"),
        "the earlier tool results must be replayed: {replayed}"
    );

    // The continuation is recorded, with the segment granted and the running
    // total (`rounds_granted` grew by `agent_max_tool_rounds`).
    let events = audit_events(&repo);
    assert!(events.contains("turn_budget_extended"), "{events}");
    assert!(events.contains("\"roundsGranted\":\"8\""), "{events}");
    assert!(events.contains("\"roundsTotal\":\"8\""), "{events}");
}

#[test]
fn answer_now_makes_exactly_one_forced_final_call() {
    let repo = temp_repo("answernow");
    let engine = engine(&repo, 8, None);
    // Eight read-only rounds, then a ninth that still asks for a tool: on
    // *Answer now* the forced call has no tools, so the turn ends exhausted
    // after exactly one more call.
    let responses: Vec<String> = (0..8)
        .map(|_| read_only_round())
        .chain(std::iter::once(read_only_round()))
        .collect();
    let mut adapter = MockModelAdapter::new_sequence(responses);
    let (paused, calls) = ask(&engine, &repo, "Look at the source", &mut adapter);
    assert_eq!(calls, 8, "the pause precedes the forced call");
    let proposal = paused
        .round_budget_proposal
        .as_ref()
        .expect("the pause surfaces a proposal");

    let resumed = resume_round_budget(
        &engine,
        &proposal.id,
        RoundBudgetDecision::AnswerNow,
        &mut adapter,
    );

    assert_eq!(
        adapter.requests.len(),
        9,
        "Answer now makes exactly one forced final call on resume"
    );
    assert_eq!(
        resumed.task.id, paused.task.id,
        "the answered turn is the same task"
    );
    assert_eq!(
        resumed.task.status,
        TaskStatus::ToolBudgetExhausted,
        "the model still asks for a tool, so the turn ends exhausted"
    );
}

#[test]
fn resume_round_budget_refuses_a_foreign_pause() {
    let repo = temp_repo("foreign");
    let engine = engine(&repo, 8, None);
    let mut adapter = MockModelAdapter::new(command_round());
    let (paused, _) = ask(&engine, &repo, "Bump the constant with sed", &mut adapter);
    let proposal = paused
        .command_proposal
        .as_ref()
        .expect("the command should need approval");

    let cancel = CancelToken::new();
    let mut on_token = |_token: &str| {};
    let mut on_progress = |_event: TurnProgress| {};
    let mut sink = TurnSink {
        on_token: &mut on_token,
        on_progress: &mut on_progress,
        cancel: &cancel,
        steering: None,
    };
    let mut resume_adapter = MockModelAdapter::new("Done.");
    let error = engine
        .chat_orchestrator
        .resume_round_budget(
            &proposal.id,
            RoundBudgetDecision::Continue,
            "tester",
            &mut resume_adapter,
            &mut sink,
        )
        .expect_err("a command pause must not resume through the round-budget entry point");
    assert!(matches!(error, ClientError::InvalidInput(_)), "{error}");
    // The pending turn is put back, so the right resume can still find it.
    assert!(
        !pending_file(&repo, &proposal.id).is_empty(),
        "the command pause must remain on disk for its own entry point"
    );
}

#[test]
fn command_and_fallback_resume_refuse_a_round_budget_pause() {
    let repo = temp_repo("refused");
    let engine = engine(&repo, 8, None);
    let mut adapter =
        MockModelAdapter::new_sequence((0..8).map(|_| read_only_round()).collect::<Vec<_>>());
    let (paused, _) = ask(&engine, &repo, "Look at the source", &mut adapter);
    let proposal = paused
        .round_budget_proposal
        .as_ref()
        .expect("the turn pauses at the budget");

    let command_error = resume_command_decision(&engine, &proposal.id)
        .expect_err("a round-budget pause must not resume as a command");
    assert!(
        matches!(command_error, ClientError::InvalidInput(_)),
        "{command_error}"
    );

    let fallback_error = resume_fallback_decision(&engine, &proposal.id)
        .expect_err("a round-budget pause must not resume as a fallback offer");
    assert!(
        matches!(fallback_error, ClientError::InvalidInput(_)),
        "{fallback_error}"
    );

    // Still on disk after both refusals, so `resume_round_budget` can find it.
    assert!(
        !pending_file(&repo, &proposal.id).is_empty(),
        "the round-budget pause must remain on disk for its own entry point"
    );
}

#[test]
fn each_continuation_is_recorded() {
    // Two consecutive *Continue*s must each actually gain a segment of rounds
    // (the running total grows 0 -> 8 -> 16), and each is recorded with the
    // round it was granted at, the delta, and the running total. This pins the
    // gap that `ABSOLUTE_TURN_ROUND_CAP` closes: with the per-segment cap the
    // second Continue would grant nothing and the turn would re-pause at once.
    let repo = temp_repo("recorded");
    let engine = engine(&repo, 8, None);
    let mut adapter = MockModelAdapter::new(read_only_round());

    let (result, _) = ask(&engine, &repo, "Look at the source", &mut adapter);
    let first = result
        .round_budget_proposal
        .clone()
        .expect("the turn pauses at its first segment");
    assert_eq!(first.rounds_granted, 0);

    let second = resume_round_budget(
        &engine,
        &first.id,
        RoundBudgetDecision::Continue,
        &mut adapter,
    )
    .round_budget_proposal
    .expect("the first Continue pauses again");
    assert_eq!(
        second.rounds_granted, 8,
        "the first Continue grants one segment"
    );

    let third = resume_round_budget(
        &engine,
        &second.id,
        RoundBudgetDecision::Continue,
        &mut adapter,
    )
    .round_budget_proposal
    .expect("the second Continue pauses again");
    assert_eq!(
        third.rounds_granted, 16,
        "the second Continue grants another segment"
    );
    assert_eq!(
        third.rounds_used, 24,
        "the second Continue actually gains eight more rounds"
    );

    let extended = turn_budget_extended_events(&repo);
    assert_eq!(extended.len(), 2, "one record per continuation");
    assert_eq!(extended[0]["actor"], "tester");
    assert_eq!(extended[0]["round"], "8");
    assert_eq!(extended[0]["roundsGranted"], "8");
    assert_eq!(extended[0]["roundsTotal"], "8");
    assert_eq!(extended[1]["actor"], "tester");
    assert_eq!(extended[1]["round"], "16");
    assert_eq!(extended[1]["roundsGranted"], "8");
    assert_eq!(extended[1]["roundsTotal"], "16");
}

#[test]
fn a_repeatedly_continued_turn_still_ends() {
    // Requirement 4 / §5.3: `ABSOLUTE_TURN_ROUND_CAP` bounds the rounds of one
    // turn across all continuations. With the default eight-round segment the
    // turn takes seven continuations to reach the cap (64 = 8 + 7×8), and the
    // eighth pause reports `at_hard_cap` and refuses *Continue* — only *Answer
    // now* remains.
    let repo = temp_repo("hardcap");
    let engine = engine(&repo, 8, None);
    let mut adapter = MockModelAdapter::new(read_only_round());

    let (mut result, _) = ask(&engine, &repo, "Look at the source", &mut adapter);
    let mut proposal = result
        .round_budget_proposal
        .clone()
        .expect("the turn pauses at its first segment");

    let mut continuations = 0;
    while !proposal.at_hard_cap {
        assert!(
            continuations < 16,
            "the turn must reach the hard cap in bounded steps"
        );
        result = resume_round_budget(
            &engine,
            &proposal.id,
            RoundBudgetDecision::Continue,
            &mut adapter,
        );
        proposal = result
            .round_budget_proposal
            .clone()
            .expect("each continuation pauses again");
        continuations += 1;
    }

    assert_eq!(continuations, 7, "64 = 8 + 7 continuations of 8");
    assert_eq!(proposal.rounds_used, 64);
    assert_eq!(proposal.rounds_granted, 56);
    assert!(proposal.at_hard_cap);

    // At the bound only *Answer now* is offered: Continue is refused, and the
    // pending turn stays on disk so the answer-now entry point can still find it.
    let error = resume_round_budget_attempt(
        &engine,
        &proposal.id,
        RoundBudgetDecision::Continue,
        &mut adapter,
    )
    .expect_err("Continue must be refused at the hard cap");
    assert!(matches!(error, ClientError::InvalidInput(_)), "{error}");
    assert!(
        !pending_file(&repo, &proposal.id).is_empty(),
        "the pending turn must remain for Answer now"
    );
}

/// Reaches a stored `ToolBudgetExhausted` task: propose a plan, run read-only
/// rounds to the budget, then answer now with a model that still wants a tool.
fn exhausted_turn_with_a_plan(repo: &Path, engine: &WorkspaceEngine) -> (ChatTurnResult, String) {
    let responses: Vec<String> = std::iter::once(String::new())
        .chain((0..8).map(|_| read_only_round()))
        .collect();
    let mut tool_calls = vec![vec![ToolCall {
        id: "plan_1".to_string(),
        name: "propose_plan".to_string(),
        arguments_json: r#"{"steps":[{"title":"Look around"},{"title":"Report back"}]}"#
            .to_string(),
    }]];
    tool_calls.extend((0..8).map(|_| Vec::new()));
    let mut adapter = MockModelAdapter::new_sequence_with_tool_calls(responses, tool_calls);

    let (paused, _) = ask(engine, repo, "Look at the source", &mut adapter);
    let proposal = paused
        .round_budget_proposal
        .as_ref()
        .expect("the pause surfaces a proposal");
    let exhausted = resume_round_budget(
        engine,
        &proposal.id,
        RoundBudgetDecision::AnswerNow,
        &mut adapter,
    );
    assert_eq!(
        exhausted.task.status,
        TaskStatus::ToolBudgetExhausted,
        "the answer-now forced call still wants a tool"
    );
    let prompt = engine
        .chat_orchestrator
        .tool_budget_ask_again_prompt(&exhausted.session.id, &exhausted.task.id)
        .expect("the ask-again prompt should build");
    (exhausted, prompt)
}

#[test]
fn asking_again_after_a_tool_budget_stop_carries_the_request_and_plan() {
    // §5.7: a stored `ToolBudgetExhausted` task keeps a way on, and that way
    // carries the original request (built by the engine from `task.user_prompt`,
    // not the client) and the plan — a tool-round stop now hands the plan over
    // exactly as a token-ceiling stop does.
    let repo = temp_repo("askagain");
    let engine = engine(&repo, 8, None);
    let (exhausted, prompt) = exhausted_turn_with_a_plan(&repo, &engine);

    let first_plan = engine
        .session_store
        .read_task_plan(&exhausted.session.id, &exhausted.task.id)
        .unwrap()
        .expect("the exhausted turn had a plan");
    assert_eq!(first_plan.steps.len(), 2);

    assert!(prompt.contains("Look at the source"), "{prompt}");
    assert!(
        prompt.starts_with("Continue this earlier request:"),
        "{prompt}"
    );

    let cancel = CancelToken::new();
    let mut on_token = |_token: &str| {};
    let mut on_progress = |_event: TurnProgress| {};
    let mut sink = TurnSink {
        on_token: &mut on_token,
        on_progress: &mut on_progress,
        cancel: &cancel,
        steering: None,
    };
    let resumed = engine
        .chat_orchestrator
        .ask_with_session(
            &repo,
            &prompt,
            &[],
            Some(&exhausted.session.id),
            &mut MockModelAdapter::new("Done."),
            &mut sink,
        )
        .expect("the asked-again turn should run");

    assert_ne!(
        resumed.task.id, exhausted.task.id,
        "ask-again is a new task, never the stopped one"
    );
    let carried = engine
        .session_store
        .read_task_plan(&resumed.session.id, &resumed.task.id)
        .unwrap()
        .expect("the plan carried onto the asked-again task");
    assert_eq!(carried.steps.len(), 2);
    assert_eq!(carried.steps[0].title, first_plan.steps[0].title);
}

#[test]
fn a_continued_turn_stays_out_of_web_debug_mode() {
    // Requirement 3 / C4: continuing must not switch on web debug mode. The old
    // *Continue debugging* button forced it on through the `continue_debugging`
    // turn option; the *Ask again* path must let the round limit come from the
    // prompt alone, so a plain request keeps the default limit — not the
    // web-debug limit (12) or the absolute cap (16).
    let repo = temp_repo("nowebdebug");
    let engine = WorkspaceEngine::new(Config {
        data_dir: repo.join(".damaian"),
        agent_max_tool_rounds: 8,
        agent_web_debug_max_tool_rounds: 12,
        enable_index_watcher: false,
        ..Config::default()
    });

    let responses: Vec<String> = (0..8)
        .map(|_| read_only_round())
        .chain(std::iter::once(read_only_round()))
        .collect();
    let mut adapter = MockModelAdapter::new_sequence(responses);
    let (paused, _) = ask(&engine, &repo, "Look at the source", &mut adapter);
    let proposal = paused
        .round_budget_proposal
        .as_ref()
        .expect("the pause surfaces a proposal");
    let exhausted = resume_round_budget(
        &engine,
        &proposal.id,
        RoundBudgetDecision::AnswerNow,
        &mut adapter,
    );
    assert_eq!(exhausted.task.status, TaskStatus::ToolBudgetExhausted);

    let prompt = engine
        .chat_orchestrator
        .tool_budget_ask_again_prompt(&exhausted.session.id, &exhausted.task.id)
        .expect("the ask-again prompt should build");

    let cancel = CancelToken::new();
    let mut progress = Vec::new();
    let mut on_token = |_token: &str| {};
    let mut on_progress = |event: TurnProgress| progress.push(event);
    let mut sink = TurnSink {
        on_token: &mut on_token,
        on_progress: &mut on_progress,
        cancel: &cancel,
        steering: None,
    };
    engine
        .chat_orchestrator
        .ask_with_session(
            &repo,
            &prompt,
            &[],
            Some(&exhausted.session.id),
            &mut MockModelAdapter::new("Done."),
            &mut sink,
        )
        .expect("the asked-again turn should run");

    let max_rounds = progress
        .iter()
        .find_map(|event| match event {
            TurnProgress::Phase(phase) if phase.kind == PhaseKind::Model => Some(phase.max_rounds),
            _ => None,
        })
        .expect("a model phase reports the round limit");
    assert_eq!(
        max_rounds, 8,
        "a continued turn keeps the default limit, not web-debug or the cap"
    );
}
