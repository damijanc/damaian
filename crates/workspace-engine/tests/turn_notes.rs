//! Exploration memory within and across turns, per
//! `docs/specs/65_exploration_memory_within_a_turn/proposal.md`.
//!
//! The record is model-written and bounded: it is pinned into every later
//! request of the turn and carried into the next turn of the session, while
//! the old rounds are still dropped from the request window. `0` restores the
//! pre-spec request exactly, which is what the last tests pin.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use workspace_engine::{
    CancelToken, ChatTurnResult, Config, ExportFormat, MockModelAdapter, ModelProviderConfig,
    RoundBudgetDecision, SecretScanner, TaskStatus, ToolCall, TurnProgress, TurnSink,
    WorkspaceEngine,
};

static COUNTER: AtomicU64 = AtomicU64::new(1);

fn temp_repo(name: &str) -> PathBuf {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock should work")
        .as_nanos();
    let repo = std::env::temp_dir().join(format!(
        "damaian-notes-{name}-{now}-{}",
        COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir_all(repo.join("src")).expect("repository should be created");
    fs::write(repo.join("src/a.rs"), "fn one() {}\n").expect("a file to read");
    fs::write(repo.join("src/b.rs"), "fn bee() {}\n").expect("a file to read");
    fs::write(repo.join("src/c.rs"), "fn sea() {}\n").expect("a file to read");
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
        context_window: None,
        provider_reports_usage: true,
        price_per_million_input_tokens: None,
        price_per_million_output_tokens: None,
        price_per_million_cached_input_tokens: None,
        supports_explicit_cache_breakpoints: false,
    }
}

fn engine_with(
    repo: &Path,
    max_tool_rounds: u32,
    max_exploration_rounds: u32,
    max_messages: usize,
    notes_chars: usize,
) -> WorkspaceEngine {
    let mut config = Config {
        data_dir: repo.join(".damaian"),
        agent_max_tool_rounds: max_tool_rounds,
        agent_max_exploration_rounds: max_exploration_rounds,
        agent_max_turn_messages: max_messages,
        agent_max_turn_notes_chars: notes_chars,
        // Throwaway repository: a watcher would only cost FSEvents registration.
        enable_index_watcher: false,
        ..Config::default()
    };
    config.model_providers.push(native_tool_provider());
    WorkspaceEngine::new(config)
}

/// An engine with the defaults this file is about: eight action rounds, the
/// full read allowance, the default request window and the default notes bound.
fn engine(repo: &Path) -> WorkspaceEngine {
    engine_with(repo, 8, 24, 24, 8_000)
}

fn ask(
    engine: &WorkspaceEngine,
    repo: &Path,
    prompt: &str,
    session_id: Option<&str>,
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
        .ask_with_session(repo, prompt, &[], session_id, adapter, &mut sink)
        .expect("the turn should run")
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

fn search_content(pattern: &str) -> ToolCall {
    tool_call("search_content", format!(r#"{{"pattern":"{pattern}"}}"#))
}

fn run_command(command: &str) -> ToolCall {
    tool_call(
        "run_command",
        format!(r#"{{"command":"{command}","reason":"Look."}}"#),
    )
}

fn notes(text: &str) -> ToolCall {
    tool_call(
        "turn_notes",
        serde_json::json!({ "text": text }).to_string(),
    )
}

fn notes_replace(text: &str) -> ToolCall {
    tool_call(
        "turn_notes",
        serde_json::json!({ "text": text, "replace": true }).to_string(),
    )
}

/// Every message content of every request the adapter received, in order. The
/// separator keeps one request's tail from joining the next one's head.
fn request_text(adapter: &MockModelAdapter) -> Vec<String> {
    adapter
        .requests
        .iter()
        .map(|request| {
            request
                .messages
                .iter()
                .map(|message| message.content.as_str())
                .collect::<Vec<_>>()
                .join("\n---\n")
        })
        .collect()
}

fn last_request(adapter: &MockModelAdapter) -> String {
    adapter
        .requests
        .last()
        .expect("at least one request")
        .messages
        .iter()
        .map(|message| message.content.as_str())
        .collect::<Vec<_>>()
        .join("\n---\n")
}

fn audit_events(repo: &Path) -> String {
    fs::read_to_string(repo.join(".damaian/audit/events.jsonl")).unwrap_or_default()
}

fn session_log(engine: &WorkspaceEngine, session_id: &str) -> String {
    let path = engine
        .config
        .data_dir
        .join("sessions")
        .join(format!("{session_id}.jsonl"));
    fs::read_to_string(path).unwrap_or_default()
}

fn pending(repo: &Path, proposal_id: &str) -> serde_json::Value {
    let path = repo
        .join(".damaian/chat/pending")
        .join(format!("{proposal_id}.json"));
    let text = fs::read_to_string(path).expect("the pending turn should exist");
    serde_json::from_str(&text).expect("the pending turn should be JSON")
}

// ---------------------------------------------------------------------------
// Requirement 2: notes are pinned into every later request of the turn.
// ---------------------------------------------------------------------------

#[test]
fn a_note_from_round_two_is_in_every_later_request_including_answer_now() {
    let repo = temp_repo("pinned");
    // One action round per segment, so the turn pauses through #61 and the
    // note has to survive the pause as well as the request window.
    let engine = engine_with(&repo, 1, 24, 8, 8_000);
    let note = "Approvals are decided in command_policy.rs.";

    let mut adapter = MockModelAdapter::new_sequence_with_tool_calls(
        vec![String::new(), String::new(), String::new()],
        vec![
            vec![read_file("src/a.rs"), notes(note)],
            vec![run_command("ls src")],
            vec![run_command("ls src")],
        ],
    );
    let first = ask(&engine, &repo, "Investigate approvals", None, &mut adapter);
    let proposal = first
        .round_budget_proposal
        .as_ref()
        .expect("one action round reaches the segment");
    assert_eq!(
        pending(&repo, &proposal.id)["turn_notes"],
        note,
        "the pause persists the notes"
    );

    // Continue: one more action round, which pauses again. The note must be in
    // that request.
    let mut continuation = MockModelAdapter::new_sequence_with_tool_calls(
        vec![String::new()],
        vec![vec![run_command("ls src")]],
    );
    let second = resume_round_budget(
        &engine,
        &proposal.id,
        RoundBudgetDecision::Continue,
        &mut continuation,
    );
    assert!(
        last_request(&continuation).contains(note),
        "the note survives the #61 pause: {}",
        last_request(&continuation)
    );

    // Answer now: the forced final call carries the note too.
    let proposal = second
        .round_budget_proposal
        .as_ref()
        .expect("the continued segment reaches the limit again");
    let mut answering = MockModelAdapter::new_sequence(vec!["Answered from my notes.".to_string()]);
    let answered = resume_round_budget(
        &engine,
        &proposal.id,
        RoundBudgetDecision::AnswerNow,
        &mut answering,
    );
    assert!(answered.round_budget_proposal.is_none());
    assert!(
        last_request(&answering).contains(note),
        "the forced final request carries the notes: {}",
        last_request(&answering)
    );
    assert_eq!(
        answered
            .turn_notes
            .as_ref()
            .map(|notes| notes.text.as_str()),
        Some(note)
    );
}

#[test]
fn notes_survive_the_narrowest_window_as_the_first_block_after_the_seed() {
    for max_messages in [3, 4, 10, 24] {
        let repo = temp_repo("narrow");
        let engine = engine_with(&repo, 8, 24, max_messages, 8_000);
        let note = "The retry helper lives in retry.rs.";
        let mut responses = vec![String::new()];
        let mut calls = vec![vec![read_file("src/a.rs"), notes(note)]];
        for index in 1..6 {
            responses.push(String::new());
            calls.push(vec![search_content(&format!("fn_{index}"))]);
        }
        responses.push("Done.".to_string());
        calls.push(Vec::new());
        let mut adapter = MockModelAdapter::new_sequence_with_tool_calls(responses, calls);

        ask(&engine, &repo, "Look around", None, &mut adapter);

        for request in adapter.requests.iter().skip(1) {
            let message = request
                .messages
                .get(2)
                .expect("the notes block is the first message after the seed");
            assert_eq!(message.role, "system");
            assert!(
                message.content.contains(note),
                "at max_messages={max_messages} the notes are still pinned: {}",
                message.content
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Requirement 3: the bound, its refusal, and `0` restoring today's request.
// ---------------------------------------------------------------------------

#[test]
fn an_append_over_the_bound_is_refused_and_a_shorter_replace_is_accepted() {
    let repo = temp_repo("bound");
    let engine = engine_with(&repo, 8, 24, 24, 20);
    let mut adapter = MockModelAdapter::new_sequence_with_tool_calls(
        vec![String::new(), String::new(), String::new()],
        vec![
            vec![notes("1234567890")],
            // 10 + 2 + 10 > 20: refused, and the log result says why.
            vec![notes("abcdefghij")],
            vec![notes_replace("short")],
        ],
    );

    let result = ask(&engine, &repo, "Take notes", None, &mut adapter);

    let log = session_log(&engine, &result.session.id);
    assert!(
        log.contains("notes: 10 / 20 characters."),
        "the refusal names the current size and the limit: {log}"
    );
    assert!(
        log.contains("replace"),
        "the refusal suggests replace: {log}"
    );
    assert!(
        !log.contains("abcdefghij"),
        "the refused text is not stored: {log}"
    );
    assert_eq!(
        result.turn_notes.as_ref().map(|notes| notes.text.as_str()),
        Some("short"),
        "the replace shortens the record"
    );
}

#[test]
fn the_boundary_is_inclusive() {
    let repo = temp_repo("boundary");
    let engine = engine_with(&repo, 8, 24, 24, 10);

    let mut accepted = MockModelAdapter::new_sequence_with_tool_calls(
        vec![String::new()],
        vec![vec![notes("0123456789")]],
    );
    let result = ask(&engine, &repo, "Take notes", None, &mut accepted);
    assert_eq!(
        result.turn_notes.as_ref().map(|notes| notes.text.len()),
        Some(10)
    );

    let repo = temp_repo("boundary-over");
    let engine = engine_with(&repo, 8, 24, 24, 10);
    let mut refused = MockModelAdapter::new_sequence_with_tool_calls(
        vec![String::new()],
        vec![vec![notes("0123456789x")]],
    );
    let result = ask(&engine, &repo, "Take notes", None, &mut refused);
    assert!(result.turn_notes.is_none(), "one over the limit is refused");
}

#[test]
fn a_replace_clears_the_carried_header_flag() {
    let repo = temp_repo("replace-carried");
    let engine = engine_with(&repo, 8, 24, 24, 8_000);

    let first = ask(
        &engine,
        &repo,
        "First",
        None,
        &mut MockModelAdapter::new_sequence_with_tool_calls(
            vec![String::new()],
            vec![vec![notes("carried finding")]],
        ),
    );
    let session = first.session.id.clone();

    // The second turn starts carried, then replaces the whole record.
    let mut second = MockModelAdapter::new_sequence_with_tool_calls(
        vec![String::new(), "Replaced.".to_string()],
        vec![vec![notes_replace("second finding")], Vec::new()],
    );
    let second_result = ask(&engine, &repo, "Second", Some(&session), &mut second);
    assert!(
        request_text(&second)
            .first()
            .is_some_and(|request| request.contains("Notes carried from earlier turns")),
        "the turn starts under the carried header"
    );
    assert_eq!(
        second_result
            .turn_notes
            .as_ref()
            .map(|notes| (notes.text.as_str(), notes.carried)),
        Some(("second finding", false)),
        "a replace clears the carried flag"
    );
}

#[test]
fn zero_notes_leaves_every_request_as_todays() {
    let repo = temp_repo("off");
    let engine = engine_with(&repo, 8, 24, 8, 0);
    let mut responses = vec![String::new()];
    let mut calls = vec![vec![read_file("src/a.rs")]];
    for index in 1..6 {
        responses.push(String::new());
        calls.push(vec![search_content(&format!("fn_{index}"))]);
    }
    responses.push("Done.".to_string());
    calls.push(Vec::new());
    let mut adapter = MockModelAdapter::new_sequence_with_tool_calls(responses, calls);

    let result = ask(&engine, &repo, "Look around", None, &mut adapter);

    let text = request_text(&adapter).join("\n=====\n");
    assert!(
        !text.contains("Notes you recorded earlier"),
        "no notes block with the key at zero"
    );
    assert!(
        !text.contains("Calls already made, oldest first"),
        "no calls-aware notice with the key at zero"
    );
    assert!(
        !text.contains("The window is full."),
        "no pre-cut warning with the key at zero"
    );
    // And the tool was not offered at all.
    assert!(
        !adapter.requests.iter().any(|request| request
            .tools
            .as_ref()
            .is_some_and(|tools| tools.iter().any(|tool| tool.name == "turn_notes"))),
        "turn_notes is not offered when the bound is zero"
    );
    assert!(result.turn_notes.is_none());
}

// ---------------------------------------------------------------------------
// Requirements 4 and 5: the notice names the cut calls, and the warning names
// the round about to leave.
// ---------------------------------------------------------------------------

#[test]
fn the_elision_notice_lists_every_elided_call_oldest_first() {
    let repo = temp_repo("notice");
    let engine = engine_with(&repo, 8, 24, 8, 8_000);
    let mut responses = Vec::new();
    let mut calls = Vec::new();
    let reads = [
        read_file("src/a.rs"),
        read_file("src/b.rs"),
        read_file("src/c.rs"),
        search_content("fn_one"),
        search_content("fn_bee"),
    ];
    for read in reads {
        responses.push(String::new());
        calls.push(vec![read]);
    }
    responses.push("Done.".to_string());
    calls.push(Vec::new());
    let mut adapter = MockModelAdapter::new_sequence_with_tool_calls(responses, calls);

    ask(&engine, &repo, "Look around", None, &mut adapter);

    let text = request_text(&adapter).join("\n=====\n");
    let notice = text
        .find("Calls already made, oldest first:")
        .expect("a cut turn has the calls-aware notice");
    let after = &text[notice..];
    let a = after
        .find("read_file src/a.rs")
        .expect("the first elided read is listed");
    let b = after
        .find("read_file src/b.rs")
        .expect("the second elided read is listed");
    assert!(a < b, "oldest first: {after}");
}

#[test]
fn the_warning_names_the_oldest_round_and_the_next_notice_lists_it() {
    let repo = temp_repo("warning");
    let engine = engine_with(&repo, 8, 24, 8, 8_000);
    let mut responses = Vec::new();
    let mut calls = Vec::new();
    let reads = [
        read_file("src/a.rs"),
        read_file("src/b.rs"),
        read_file("src/c.rs"),
        search_content("fn_one"),
        search_content("fn_bee"),
        search_content("fn_sea"),
    ];
    for read in reads {
        responses.push(String::new());
        calls.push(vec![read]);
    }
    responses.push("Done.".to_string());
    calls.push(Vec::new());
    let mut adapter = MockModelAdapter::new_sequence_with_tool_calls(responses, calls);

    ask(&engine, &repo, "Look around", None, &mut adapter);

    let texts = request_text(&adapter);
    assert!(
        texts
            .iter()
            .any(|text| text.contains("The window is full.")),
        "once the tail fills the window every request warns"
    );
    let warning_index = texts
        .iter()
        .position(|text| text.contains("The window is full."))
        .expect("a warning");
    let warned = texts[warning_index]
        .split("The window is full.")
        .nth(1)
        .and_then(|rest| {
            rest.lines()
                .find(|line| line.trim_start().starts_with("read_file "))
        })
        .map(|line| line.trim().to_string());
    if let Some(warned) = warned {
        assert!(
            texts[warning_index + 1..]
                .iter()
                .any(|text| text.contains(&warned)),
            "the next request's notice lists the call the warning named: {warned}"
        );
    }
}

// ---------------------------------------------------------------------------
// Requirement 6: notes carry into the next turn of the session, and a rewind
// carries from the turn before the rewound one.
// ---------------------------------------------------------------------------

#[test]
fn the_next_turn_starts_with_the_previous_turns_final_notes() {
    let repo = temp_repo("carry");
    let engine = engine(&repo);
    let finding = "Approvals are decided in command_policy.rs.";

    let first = ask(
        &engine,
        &repo,
        "Find out how approvals work",
        None,
        &mut MockModelAdapter::new_sequence_with_tool_calls(
            vec![
                String::new(),
                "Approvals are decided in command_policy.rs.".to_string(),
            ],
            vec![vec![notes(finding)], Vec::new()],
        ),
    );
    let session = first.session.id.clone();

    let mut second = MockModelAdapter::new_sequence(vec!["Now add a cause.".to_string()]);
    let second_result = ask(&engine, &repo, "Add a cause", Some(&session), &mut second);

    let first_request = request_text(&second)
        .first()
        .cloned()
        .expect("the second turn's first request");
    assert!(
        first_request.contains("Notes carried from earlier turns"),
        "the carried header is used: {first_request}"
    );
    assert!(first_request.contains(finding), "{first_request}");
    assert!(
        second_result
            .turn_notes
            .as_ref()
            .is_some_and(|notes| notes.carried && notes.text.contains(finding)),
        "the result marks the notes as carried"
    );

    let log = session_log(&engine, &session);
    assert!(
        log.contains("\"eventType\":\"turn_notes\"") && log.contains("\"carried\":true"),
        "a carry event is written before the first request: {log}"
    );
    assert!(
        audit_events(&repo).contains("\"eventType\":\"turn_notes_carried\""),
        "the carry is audited"
    );
}

#[test]
fn a_third_turn_starts_with_the_second_turns_replacement() {
    let repo = temp_repo("carry-chain");
    let engine = engine(&repo);

    let first = ask(
        &engine,
        &repo,
        "First",
        None,
        &mut MockModelAdapter::new_sequence_with_tool_calls(
            vec![String::new(), "First answer.".to_string()],
            vec![vec![notes("first finding")], Vec::new()],
        ),
    );
    let session = first.session.id.clone();

    ask(
        &engine,
        &repo,
        "Second",
        Some(&session),
        &mut MockModelAdapter::new_sequence_with_tool_calls(
            vec![String::new(), "Second answer.".to_string()],
            vec![vec![notes_replace("second finding")], Vec::new()],
        ),
    );

    let mut third = MockModelAdapter::new_sequence(vec!["Third answer.".to_string()]);
    ask(&engine, &repo, "Third", Some(&session), &mut third);
    let request = request_text(&third)
        .first()
        .cloned()
        .expect("the third turn's first request");
    assert!(request.contains("second finding"), "{request}");
    assert!(!request.contains("first finding"), "{request}");
}

#[test]
fn a_rewind_carries_from_the_turn_before_the_rewound_one() {
    let repo = temp_repo("rewind");
    let engine = engine(&repo);

    let first = ask(
        &engine,
        &repo,
        "First",
        None,
        &mut MockModelAdapter::new_sequence_with_tool_calls(
            vec![String::new(), "First answer.".to_string()],
            vec![vec![notes("first finding")], Vec::new()],
        ),
    );
    let session = first.session.id.clone();
    let before_second = engine
        .session_store
        .latest_event_seq(&session)
        .expect("the log has a position");

    ask(
        &engine,
        &repo,
        "Second",
        Some(&session),
        &mut MockModelAdapter::new_sequence_with_tool_calls(
            vec![String::new(), "Second answer.".to_string()],
            vec![vec![notes_replace("second finding")], Vec::new()],
        ),
    );

    engine
        .session_store
        .rewind_conversation(&session, before_second)
        .expect("the rewind should be recorded");

    let mut after_rewind = MockModelAdapter::new_sequence(vec!["After rewind.".to_string()]);
    ask(&engine, &repo, "Third", Some(&session), &mut after_rewind);
    let request = request_text(&after_rewind)
        .first()
        .cloned()
        .expect("the new turn's first request");
    assert!(request.contains("first finding"), "{request}");
    assert!(!request.contains("second finding"), "{request}");
}

#[test]
fn a_new_session_starts_with_no_notes() {
    let repo = temp_repo("new-session");
    let engine = engine(&repo);
    ask(
        &engine,
        &repo,
        "First",
        None,
        &mut MockModelAdapter::new_sequence_with_tool_calls(
            vec![String::new(), "Answer.".to_string()],
            vec![vec![notes("first finding")], Vec::new()],
        ),
    );

    let mut other = MockModelAdapter::new_sequence(vec!["Other answer.".to_string()]);
    ask(&engine, &repo, "Other", None, &mut other);
    let request = request_text(&other).first().cloned().expect("a request");
    assert!(!request.contains("first finding"), "{request}");
    assert!(!request.contains("Notes"), "{request}");
}

// ---------------------------------------------------------------------------
// Requirements 7 and 8: pauses, resume, repeat detection, and the budget class.
// ---------------------------------------------------------------------------

#[test]
fn a_command_approval_resume_keeps_the_notes() {
    let repo = temp_repo("approval");
    let engine = engine(&repo);
    let finding = "The classifier is in validation.rs.";

    let first = ask(
        &engine,
        &repo,
        "Change the constant",
        None,
        &mut MockModelAdapter::new_sequence_with_tool_calls(
            vec![String::new()],
            vec![vec![
                notes(finding),
                run_command("sed -i '' 's/1/2/' src/a.rs"),
            ]],
        ),
    );
    let proposal = first
        .command_proposal
        .as_ref()
        .expect("a command needing approval pauses the turn");
    assert_eq!(pending(&repo, &proposal.id)["turn_notes"], finding);

    let mut resumed = MockModelAdapter::new_sequence(vec!["Done.".to_string()]);
    let result = resume_command_decision(&engine, &proposal.id, &mut resumed);
    assert!(
        last_request(&resumed).contains(finding),
        "the resumed request keeps the notes: {}",
        last_request(&resumed)
    );
    assert_eq!(
        result.turn_notes.as_ref().map(|notes| notes.text.as_str()),
        Some(finding)
    );
}

#[test]
fn a_pending_turn_written_before_notes_loads_with_none() {
    let repo = temp_repo("old-pending");
    let engine = engine(&repo);
    let first = ask(
        &engine,
        &repo,
        "Change the constant",
        None,
        &mut MockModelAdapter::new_sequence_with_tool_calls(
            vec![String::new()],
            vec![vec![
                notes("finding"),
                run_command("sed -i '' 's/1/2/' src/a.rs"),
            ]],
        ),
    );
    let proposal = first.command_proposal.as_ref().expect("a pause");
    let path = repo
        .join(".damaian/chat/pending")
        .join(format!("{}.json", proposal.id));
    let text = fs::read_to_string(&path).expect("the pending file");
    let mut value: serde_json::Value = serde_json::from_str(&text).expect("JSON");
    let object = value.as_object_mut().expect("an object");
    object.remove("turn_notes");
    object.remove("turn_notes_carried");
    object.remove("calls");
    fs::write(&path, serde_json::to_string(&value).expect("serialize")).expect("write");

    let mut resumed = MockModelAdapter::new_sequence(vec!["Done.".to_string()]);
    let result = resume_command_decision(&engine, &proposal.id, &mut resumed);
    assert!(result.turn_notes.is_none());
    assert!(
        !last_request(&resumed).contains("Notes you recorded earlier"),
        "an old pending turn resumes with no notes block"
    );
}

#[test]
fn a_round_of_reads_and_notes_is_an_exploration_round() {
    let repo = temp_repo("notes-exploration");
    // A single action round: if reads+notes were an action round this would
    // pause immediately.
    let engine = engine_with(&repo, 1, 24, 24, 8_000);
    let mut adapter = MockModelAdapter::new_sequence_with_tool_calls(
        vec![String::new(), "Done.".to_string()],
        vec![vec![read_file("src/a.rs"), notes("a finding")], Vec::new()],
    );

    let result = ask(&engine, &repo, "Look", None, &mut adapter);

    assert_eq!(result.task.status, TaskStatus::Complete);
    assert!(result.round_budget_proposal.is_none());
    assert_eq!(adapter.requests.len(), 2);
}

#[test]
fn an_identical_append_is_a_repeat_and_is_charged_as_an_action_round() {
    let repo = temp_repo("notes-repeat");
    let engine = engine_with(&repo, 1, 24, 24, 8_000);
    let mut adapter = MockModelAdapter::new_sequence_with_tool_calls(
        vec![String::new(), String::new()],
        vec![vec![notes("same finding")], vec![notes("same finding")]],
    );

    let result = ask(&engine, &repo, "Look", None, &mut adapter);

    assert_eq!(
        result.task.status,
        TaskStatus::WaitingForApproval,
        "the identical append is an action round, reaching the one-round budget"
    );
    assert_eq!(
        result.round_budget_proposal.as_ref().unwrap().rounds_used,
        1
    );
}

#[test]
fn a_repeat_made_before_a_continue_is_still_a_repeat_after_it() {
    let repo = temp_repo("resume-repeat");
    // One action round per segment. The command spends it, Continue grants
    // one more; the first post-resume read is a new exploration round, and the
    // second identical read must be caught as a repeat — which makes it an
    // action round and reaches the budget again.
    let engine = engine_with(&repo, 1, 24, 24, 8_000);
    let first = ask(
        &engine,
        &repo,
        "Look",
        None,
        &mut MockModelAdapter::new_sequence_with_tool_calls(
            vec![String::new()],
            vec![vec![run_command("ls src")]],
        ),
    );
    let proposal = first.round_budget_proposal.as_ref().expect("a pause");

    let mut resumed = MockModelAdapter::new_sequence_with_tool_calls(
        vec![String::new(), String::new()],
        vec![vec![read_file("src/a.rs")], vec![read_file("src/a.rs")]],
    );
    let result = resume_round_budget(
        &engine,
        &proposal.id,
        RoundBudgetDecision::Continue,
        &mut resumed,
    );

    assert_eq!(
        result.task.status,
        TaskStatus::WaitingForApproval,
        "the pre-pause read is still in the rebuilt seen-call set"
    );
    assert!(result.round_budget_proposal.is_some());
}

#[test]
fn an_identical_replace_is_never_a_repeat() {
    let repo = temp_repo("replace-repeat");
    // One action round: if a second identical replace were a repeat it would be
    // charged as an action round and the turn would pause.
    let engine = engine_with(&repo, 1, 24, 24, 8_000);
    let mut adapter = MockModelAdapter::new_sequence_with_tool_calls(
        vec![String::new(), String::new(), "Done.".to_string()],
        vec![
            vec![notes_replace("same")],
            vec![notes_replace("same")],
            Vec::new(),
        ],
    );

    let result = ask(&engine, &repo, "Look", None, &mut adapter);

    assert_eq!(result.task.status, TaskStatus::Complete);
    assert!(result.round_budget_proposal.is_none());
}

#[test]
fn a_carried_note_is_redacted_and_audited_with_sizes_only() {
    const SECRET: &str = "AKIAIOSFODNN7EXAMPLE";
    let repo = temp_repo("redaction");
    let engine = engine(&repo);
    let first = ask(
        &engine,
        &repo,
        "First",
        None,
        &mut MockModelAdapter::new_sequence_with_tool_calls(
            vec![String::new(), "Answer.".to_string()],
            vec![vec![notes(&format!("the key is {SECRET}"))], Vec::new()],
        ),
    );
    let session = first.session.id.clone();

    let mut second = MockModelAdapter::new_sequence(vec!["Second answer.".to_string()]);
    let result = ask(&engine, &repo, "Second", Some(&session), &mut second);

    let log = session_log(&engine, &session);
    assert!(!log.contains(SECRET), "the log never holds the secret");
    let audit = audit_events(&repo);
    assert!(!audit.contains(SECRET), "the audit never holds the secret");
    assert!(
        audit.contains("\"eventType\":\"turn_notes_recorded\"") && audit.contains("\"totalChars\""),
        "the notes audit carries sizes only: {audit}"
    );
    let request = request_text(&second).join("\n");
    assert!(!request.contains(SECRET), "the request never holds it");
    assert!(
        result
            .turn_notes
            .as_ref()
            .is_some_and(|notes| !notes.text.contains(SECRET))
    );

    let markdown = engine
        .session_store
        .export_session(&session, ExportFormat::Markdown, &SecretScanner::default())
        .expect("the export should render");
    assert!(!markdown.contains(SECRET), "the export never holds it");
    assert!(markdown.contains("Notes"), "the export names the notes");
}

#[test]
fn a_notes_call_is_offered_in_every_mode() {
    for mode in [
        workspace_engine::SessionMode::Ask,
        workspace_engine::SessionMode::Plan,
        workspace_engine::SessionMode::Code,
        workspace_engine::SessionMode::Review,
    ] {
        let repo = temp_repo("mode");
        let engine = engine(&repo);
        let session = engine
            .session_store
            .create_session(
                &workspace_engine::repository_id_for_root(&repo),
                "Mode test",
            )
            .expect("a session");
        engine
            .session_store
            .set_session_mode(&session.id, mode, "user")
            .expect("the mode should be set");

        let mut adapter = MockModelAdapter::new_sequence(vec!["Done.".to_string()]);
        ask(
            &engine,
            &repo,
            "Look around",
            Some(&session.id),
            &mut adapter,
        );

        let names: Vec<&str> = adapter.requests[0]
            .tools
            .as_ref()
            .expect("native tools are offered")
            .iter()
            .map(|tool| tool.name.as_str())
            .collect();
        assert!(
            names.contains(&"turn_notes"),
            "turn_notes is offered in {mode:?}: {names:?}"
        );
    }
}
#[test]
fn the_latest_notes_event_wins_after_a_second_turn() {
    let repo = temp_repo("latest-event");
    let engine = engine(&repo);
    let first = ask(
        &engine,
        &repo,
        "First",
        None,
        &mut MockModelAdapter::new_sequence_with_tool_calls(
            vec![String::new(), "Answer.".to_string()],
            vec![vec![notes("first")], Vec::new()],
        ),
    );
    let session = first.session.id.clone();

    ask(
        &engine,
        &repo,
        "Second",
        Some(&session),
        &mut MockModelAdapter::new_sequence_with_tool_calls(
            vec![String::new(), "Answer.".to_string()],
            vec![vec![notes_replace("second")], Vec::new()],
        ),
    );

    let notes = engine
        .session_store
        .read_turn_notes(&session)
        .expect("notes should read");
    let tasks = engine
        .session_store
        .read_tasks(&session)
        .expect("tasks should read");
    let latest = tasks
        .last()
        .and_then(|task| notes.get(&task.id))
        .expect("the last task has notes");
    assert_eq!(latest.text, "second", "the latest event is the record");
}
