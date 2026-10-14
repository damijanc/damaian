//! Model-initiated clarification, per
//! `docs/specs/50_model_initiated_clarification/proposal.md`.
//!
//! Task 1 pins the store half: the question bounds, the typed link that a
//! waiting question leaves on its task (never an approval link), and the two
//! question events that put the exchange in the transcript. Nothing in the
//! chat loop writes these yet.
//!
//! Task 2 pins the per-turn cap, `agent_max_questions_per_turn`.
//!
//! Task 3 pins where the tool is offered: only with a user present and a cap
//! above zero, in every mode, and refused as a tool result anywhere else.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};
use workspace_engine::{
    ANSWER_MAX_CHARS, AskUserInput, CancelToken, ChatTurnOptions, ChatTurnResult, Config,
    ConfigOverlay, MAX_QUESTIONS_PER_TURN_CEILING, MockModelAdapter, ModelAdapter,
    ModelProviderConfig, OPTION_MAX_CHARS, OPTIONS_MAX, OPTIONS_MIN, PendingApprovalRef,
    PendingDecision, PendingQuestionRef, QUESTION_MAX_CHARS, RepositoryConfigReport,
    RepositoryKeyClass, SessionMode, SessionStore, Task, TaskStatus, ToolCall, TurnProgress,
    TurnSink, WorkspaceEngine, validate_answer,
};

static COUNTER: AtomicU64 = AtomicU64::new(1);

fn temp_data_dir(name: &str) -> PathBuf {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock should work")
        .as_nanos();
    let dir = std::env::temp_dir().join(format!(
        "damaian-clarification-{name}-{now}-{}-{}",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir_all(&dir).expect("temp dir should be created");
    dir
}

fn session_log(data_dir: &Path, session_id: &str) -> String {
    fs::read_to_string(
        data_dir
            .join("sessions")
            .join(format!("{session_id}.jsonl")),
    )
    .expect("session log should exist")
}

fn input(question: &str, options: &[&str], allow_free_text: bool) -> AskUserInput {
    AskUserInput {
        question: question.to_string(),
        options: options.iter().map(|option| option.to_string()).collect(),
        allow_free_text,
    }
}

fn cli_or_shell() -> AskUserInput {
    input(
        "Did you mean the CLI or the shell?",
        &["The CLI", "The shell"],
        false,
    )
}

/// A store with one session and one task in it.
fn fixture(name: &str) -> (PathBuf, SessionStore, Task) {
    let data_dir = temp_data_dir(name);
    let store = SessionStore::new(&data_dir);
    let session = store.create_session("repo_1", "Clarify").unwrap();
    let task = store
        .create_task(&session.id, "clean this up", "mock", "mock")
        .unwrap();
    (data_dir, store, task)
}

/// The last `task_status_updated` line in the log, raw.
fn last_status_line(data_dir: &Path, session_id: &str) -> String {
    session_log(data_dir, session_id)
        .lines()
        .rfind(|line| line.contains("\"eventType\":\"task_status_updated\""))
        .expect("a status event should have been written")
        .to_string()
}

#[test]
fn the_bounds_are_enforced_as_errors_not_truncation() {
    assert_eq!(QUESTION_MAX_CHARS, 300, "context.md §8 records this value");
    assert_eq!(OPTION_MAX_CHARS, 60, "context.md §8 records this value");
    assert_eq!((OPTIONS_MIN, OPTIONS_MAX), (2, 4), "context.md §8");
    assert_eq!(ANSWER_MAX_CHARS, 2_000, "context.md §8 records this value");

    // Exactly at each bound passes.
    let at_bound = input(
        &"q".repeat(QUESTION_MAX_CHARS),
        &[&"a".repeat(OPTION_MAX_CHARS), "b", "c", "d"],
        true,
    );
    assert_eq!(at_bound.validate(), Ok(()));
    // Characters, not bytes: a non-ASCII question at the bound still passes.
    assert_eq!(
        input(&"é".repeat(QUESTION_MAX_CHARS), &["a", "b"], false).validate(),
        Ok(())
    );

    let long_question = input(&"q".repeat(QUESTION_MAX_CHARS + 1), &["a", "b"], false);
    let error = long_question.validate().unwrap_err();
    assert!(error.contains("300"), "{error}");

    let too_many = input("Which?", &["a", "b", "c", "d", "e"], false);
    let error = too_many.validate().unwrap_err();
    assert!(error.contains('4'), "{error}");

    let long_option = input("Which?", &[&"a".repeat(OPTION_MAX_CHARS + 1), "b"], false);
    let error = long_option.validate().unwrap_err();
    assert!(error.contains("60"), "{error}");

    // The input is reported, never quietly shortened.
    assert_eq!(
        long_question.question.chars().count(),
        QUESTION_MAX_CHARS + 1
    );
}

#[test]
fn options_are_required() {
    for options in [&[][..], &["Only one"][..]] {
        let error = input("Which?", options, true).validate().unwrap_err();
        assert!(error.contains('2'), "{error}");
    }
}

#[test]
fn an_empty_question_or_option_or_a_repeated_option_is_refused() {
    assert!(input("   ", &["a", "b"], false).validate().is_err());
    assert!(input("Which?", &["a", " "], false).validate().is_err());
    assert!(
        input("Which?", &["Same", "Same"], false)
            .validate()
            .is_err()
    );
}

#[test]
fn an_answer_must_match_its_option_or_be_permitted_free_text() {
    let closed = cli_or_shell();
    assert_eq!(validate_answer(&closed, "The shell", Some(1)), Ok(()));
    // The label must be the option's own label.
    assert!(validate_answer(&closed, "The CLI", Some(1)).is_err());
    assert!(validate_answer(&closed, "The CLI", Some(2)).is_err());
    // Free text needs `allow_free_text`.
    assert!(validate_answer(&closed, "Both, actually", None).is_err());

    let open = input("Which?", &["a", "b"], true);
    assert_eq!(validate_answer(&open, "Both, actually", None), Ok(()));
    assert!(validate_answer(&open, "   ", None).is_err());
    assert!(validate_answer(&open, &"x".repeat(ANSWER_MAX_CHARS + 1), None).is_err());
    assert_eq!(
        validate_answer(&open, &"x".repeat(ANSWER_MAX_CHARS), None),
        Ok(())
    );
}

#[test]
fn a_question_link_is_not_an_approval_link() {
    let (data_dir, store, task) = fixture("link");
    let question = PendingQuestionRef::new();
    assert!(question.question_id().starts_with("question_"));
    store
        .record_question_asked(&task.session_id, &task.id, &question, &cli_or_shell())
        .unwrap();

    let waiting = store.await_answer(&task, &question).unwrap();

    assert_eq!(waiting.status, TaskStatus::WaitingForApproval);
    assert_eq!(
        store
            .pending_approval_for(&task.session_id, &task.id)
            .unwrap(),
        None,
        "a waiting question must never read as a pending approval"
    );
    match store
        .pending_decision_for(&task.session_id, &task.id)
        .unwrap()
    {
        Some(PendingDecision::Question(found)) => assert_eq!(found, question),
        other => panic!("expected a question, got {other:?}"),
    }
    let line = last_status_line(&data_dir, &task.session_id);
    assert!(line.contains("\"pendingQuestion\""), "{line}");
    assert!(!line.contains("pendingApproval"), "{line}");
    assert!(!line.contains("\"kind\""), "{line}");
}

#[test]
fn a_question_must_be_recorded_before_a_task_can_wait_on_it() {
    let (_data_dir, store, task) = fixture("unrecorded");
    assert!(
        store
            .await_answer(&task, &PendingQuestionRef::new())
            .is_err()
    );
}

#[test]
fn an_approval_link_still_reads_as_before() {
    let (_data_dir, store, task) = fixture("approval");
    let approval = PendingApprovalRef {
        kind: "command".to_string(),
        proposal_id: "cmd_1".to_string(),
    };
    store.await_approval(&task, &approval).unwrap();

    assert_eq!(
        store
            .pending_approval_for(&task.session_id, &task.id)
            .unwrap(),
        Some(approval.clone())
    );
    assert_eq!(
        store
            .pending_decision_for(&task.session_id, &task.id)
            .unwrap(),
        Some(PendingDecision::Approval(approval))
    );
}

#[test]
fn the_newest_link_wins_across_kinds() {
    let (_data_dir, store, task) = fixture("newest");
    let approval = PendingApprovalRef {
        kind: "plan".to_string(),
        proposal_id: "plan_1".to_string(),
    };
    store.await_approval(&task, &approval).unwrap();

    let question = PendingQuestionRef::new();
    store
        .record_question_asked(&task.session_id, &task.id, &question, &cli_or_shell())
        .unwrap();
    store.await_answer(&task, &question).unwrap();
    assert_eq!(
        store
            .pending_approval_for(&task.session_id, &task.id)
            .unwrap(),
        None,
        "the question replaced the approval"
    );
    assert_eq!(
        store
            .pending_decision_for(&task.session_id, &task.id)
            .unwrap(),
        Some(PendingDecision::Question(question))
    );

    store
        .update_task_status(&task, TaskStatus::PreparingContext, None)
        .unwrap();
    assert_eq!(
        store
            .pending_decision_for(&task.session_id, &task.id)
            .unwrap(),
        None,
        "a later status event without a link clears it"
    );
}

#[test]
fn an_answered_question_cannot_be_answered_again() {
    let (_data_dir, store, task) = fixture("twice");
    let question = PendingQuestionRef::new();
    store
        .record_question_asked(&task.session_id, &task.id, &question, &cli_or_shell())
        .unwrap();

    let state = store
        .record_question_answered(
            &task.session_id,
            question.question_id(),
            "The shell",
            Some(1),
        )
        .unwrap();
    assert_eq!(state.answer, Some(("The shell".to_string(), Some(1))));

    assert!(
        store
            .record_question_answered(&task.session_id, question.question_id(), "The CLI", Some(0))
            .is_err()
    );
    let state = store
        .question_state(&task.session_id, question.question_id())
        .unwrap()
        .expect("the question is recorded");
    assert_eq!(state.answer, Some(("The shell".to_string(), Some(1))));
}

#[test]
fn an_answer_to_an_unknown_question_or_an_invalid_answer_is_refused() {
    let (_data_dir, store, task) = fixture("unknown");
    assert!(
        store
            .record_question_answered(&task.session_id, "question_missing", "The CLI", Some(0))
            .is_err()
    );

    let question = PendingQuestionRef::new();
    store
        .record_question_asked(&task.session_id, &task.id, &question, &cli_or_shell())
        .unwrap();
    assert!(
        store
            .record_question_answered(&task.session_id, question.question_id(), "Neither", None)
            .is_err(),
        "free text is not allowed on this question"
    );
    let state = store
        .question_state(&task.session_id, question.question_id())
        .unwrap()
        .unwrap();
    assert_eq!(state.answer, None, "a refused answer records nothing");
}

#[test]
fn an_invalid_question_is_not_recorded() {
    let (_data_dir, store, task) = fixture("invalid");
    let question = PendingQuestionRef::new();
    assert!(
        store
            .record_question_asked(
                &task.session_id,
                &task.id,
                &question,
                &input("Which?", &["Only one"], false),
            )
            .is_err()
    );
    assert_eq!(
        store
            .question_state(&task.session_id, question.question_id())
            .unwrap(),
        None
    );
}

#[test]
fn a_question_state_reads_back_what_was_asked() {
    let (_data_dir, store, task) = fixture("state");
    let question = PendingQuestionRef::new();
    let asked = input("Which crate?", &["engine", "cli", "shell"], true);
    store
        .record_question_asked(&task.session_id, &task.id, &question, &asked)
        .unwrap();

    let state = store
        .question_state(&task.session_id, question.question_id())
        .unwrap()
        .unwrap();
    assert_eq!(state.question_id, question.question_id());
    assert_eq!(state.task_id, task.id);
    assert_eq!(state.input(), asked);
    assert_eq!(state.answer, None);
}

#[test]
fn a_question_rewound_past_is_gone() {
    let (_data_dir, store, task) = fixture("rewind");
    store
        .append_message(&task.session_id, Some(&task.id), "user", "clean this up")
        .unwrap();
    let through = store.latest_event_seq(&task.session_id).unwrap();

    let question = PendingQuestionRef::new();
    store
        .record_question_asked(&task.session_id, &task.id, &question, &cli_or_shell())
        .unwrap();
    store
        .record_question_answered(&task.session_id, question.question_id(), "The CLI", Some(0))
        .unwrap();
    store
        .rewind_conversation(&task.session_id, through)
        .unwrap();

    assert_eq!(
        store
            .question_state(&task.session_id, question.question_id())
            .unwrap(),
        None
    );
    assert_eq!(
        store
            .questions_asked_in_task(&task.session_id, &task.id)
            .unwrap(),
        0
    );
    let roles: Vec<String> = store
        .read_messages(&task.session_id)
        .unwrap()
        .into_iter()
        .map(|message| message.role)
        .collect();
    assert_eq!(roles, vec!["user"]);
}

#[test]
fn questions_appear_in_the_transcript_in_order() {
    let (_data_dir, store, task) = fixture("transcript");
    store
        .append_message(&task.session_id, Some(&task.id), "user", "clean this up")
        .unwrap();
    let question = PendingQuestionRef::new();
    store
        .record_question_asked(&task.session_id, &task.id, &question, &cli_or_shell())
        .unwrap();
    store
        .record_question_answered(&task.session_id, question.question_id(), "The CLI", Some(0))
        .unwrap();
    store
        .append_message(&task.session_id, Some(&task.id), "assistant", "Done.")
        .unwrap();

    let messages = store.read_messages(&task.session_id).unwrap();
    let shape: Vec<(&str, &str)> = messages
        .iter()
        .map(|message| (message.role.as_str(), message.content.as_str()))
        .collect();
    assert_eq!(
        shape,
        vec![
            ("user", "clean this up"),
            ("question", "Did you mean the CLI or the shell?"),
            ("answer", "The CLI"),
            ("assistant", "Done."),
        ]
    );
    assert_eq!(messages[1].id, question.question_id());
    assert_eq!(messages[1].task_id.as_deref(), Some(task.id.as_str()));
    assert_eq!(messages[2].task_id.as_deref(), Some(task.id.as_str()));

    let with_seq = store.read_messages_with_seq(&task.session_id).unwrap();
    assert_eq!(with_seq.len(), 4);
    assert!(with_seq.windows(2).all(|pair| pair[0].0 < pair[1].0));
}

#[test]
fn a_session_with_no_questions_reads_exactly_as_before() {
    let (_data_dir, store, task) = fixture("unchanged");
    store
        .append_message(&task.session_id, Some(&task.id), "user", "hello")
        .unwrap();
    store
        .append_message(&task.session_id, Some(&task.id), "assistant", "hi")
        .unwrap();
    let messages = store.read_messages(&task.session_id).unwrap();
    assert_eq!(
        messages
            .iter()
            .map(|message| (message.role.as_str(), message.content.as_str()))
            .collect::<Vec<_>>(),
        vec![("user", "hello"), ("assistant", "hi")]
    );
    assert_eq!(
        store
            .pending_decision_for(&task.session_id, &task.id)
            .unwrap(),
        None
    );
}

#[test]
fn questions_are_counted_per_task() {
    let (_data_dir, store, task) = fixture("count");
    let other = store
        .create_task(&task.session_id, "another turn", "mock", "mock")
        .unwrap();
    for _ in 0..2 {
        store
            .record_question_asked(
                &task.session_id,
                &task.id,
                &PendingQuestionRef::new(),
                &cli_or_shell(),
            )
            .unwrap();
    }
    store
        .record_question_asked(
            &task.session_id,
            &other.id,
            &PendingQuestionRef::new(),
            &cli_or_shell(),
        )
        .unwrap();

    assert_eq!(
        store
            .questions_asked_in_task(&task.session_id, &task.id)
            .unwrap(),
        2
    );
    assert_eq!(
        store
            .questions_asked_in_task(&task.session_id, &other.id)
            .unwrap(),
        1
    );
}

// Task 2: the per-turn cap is a Preference with a hard ceiling (`context.md`
// §8). Over the ceiling is an error at every scope, never a clamp: a clamp
// would let a repository ask for an interrogation and quietly get the most
// allowed.

/// Loads `user` then `repository` at their real scopes, as a desktop request
/// does. The user file is trusted, so a bad value there fails the load.
fn load_config(
    name: &str,
    user: &str,
    repository: &str,
) -> workspace_engine::Result<(Config, RepositoryConfigReport)> {
    let root = temp_data_dir(name);
    let data_dir = root.join(".damaian");
    let user_config = data_dir.join("config").join("user.conf");
    let repository_config = Config::repository_config_path(&root);
    fs::create_dir_all(user_config.parent().unwrap()).unwrap();
    fs::write(&user_config, user).unwrap();
    fs::create_dir_all(repository_config.parent().unwrap()).unwrap();
    fs::write(&repository_config, repository).unwrap();
    let base = Config {
        data_dir,
        ..Config::default()
    };
    let loaded = Config::load_scoped(
        base,
        Some(&user_config),
        Some(&repository_config),
        None,
        Some(&root),
    );
    let _ = fs::remove_dir_all(&root);
    loaded
}

#[test]
fn the_question_cap_defaults_to_two_under_a_ceiling_of_four() {
    assert_eq!(Config::default().agent_max_questions_per_turn, 2);
    assert_eq!(MAX_QUESTIONS_PER_TURN_CEILING, 4);
    let (config, report) = load_config("cap-default", "", "").unwrap();
    assert_eq!(config.agent_max_questions_per_turn, 2);
    assert!(report.rejected_keys.is_empty());
}

#[test]
fn a_cap_over_the_ceiling_fails_a_user_config_and_names_the_ceiling() {
    let error = ConfigOverlay::parse("agent_max_questions_per_turn=5\n")
        .expect_err("5 is over the ceiling")
        .to_string();
    assert!(error.contains("agent_max_questions_per_turn"), "{error}");
    let ceiling = MAX_QUESTIONS_PER_TURN_CEILING.to_string();
    assert!(
        error.contains(&ceiling),
        "the error must name the ceiling: {error}"
    );

    let loaded = load_config("cap-user-over", "agent_max_questions_per_turn=5\n", "");
    assert!(loaded.is_err(), "a user value over the ceiling must fail");
}

#[test]
fn a_cap_over_the_ceiling_is_refused_at_repository_scope_not_clamped() {
    let (config, report) =
        load_config("cap-repo-over", "", "agent_max_questions_per_turn=5\n").unwrap();
    assert_eq!(
        config.agent_max_questions_per_turn, 2,
        "the default holds; a clamp would read 4"
    );
    assert_eq!(
        report.rejected_key_names(),
        vec!["agent_max_questions_per_turn"]
    );
    assert_eq!(
        report.rejected_keys[0].class,
        RepositoryKeyClass::Unparsable
    );
}

#[test]
fn zero_and_the_ceiling_are_accepted_at_both_scopes() {
    for value in [0, MAX_QUESTIONS_PER_TURN_CEILING] {
        let line = format!("agent_max_questions_per_turn={value}\n");
        let (config, _) = load_config("cap-user-ok", &line, "").unwrap();
        assert_eq!(config.agent_max_questions_per_turn, value, "user {value}");

        let (config, report) = load_config("cap-repo-ok", "", &line).unwrap();
        assert_eq!(config.agent_max_questions_per_turn, value, "repo {value}");
        assert!(
            report.rejected_keys.is_empty(),
            "{:?}",
            report.rejected_keys
        );
    }
}

#[test]
fn a_repository_may_lower_or_raise_the_cap_within_the_ceiling() {
    let (config, report) = load_config(
        "cap-repo-raise",
        "agent_max_questions_per_turn=1\n",
        "agent_max_questions_per_turn=3\n",
    )
    .unwrap();
    assert_eq!(config.agent_max_questions_per_turn, 3);
    assert!(report.rejected_keys.is_empty());
}

#[test]
fn the_cap_round_trips_through_both_policy_texts() {
    let overlay = ConfigOverlay::parse("agent_max_questions_per_turn=0\n").unwrap();
    assert!(
        overlay
            .to_policy_text()
            .contains("agent_max_questions_per_turn=0"),
        "{}",
        overlay.to_policy_text()
    );
    let config = Config {
        agent_max_questions_per_turn: 3,
        ..Config::default()
    };
    assert!(
        config
            .to_policy_text()
            .contains("agent_max_questions_per_turn=3")
    );
}

#[test]
fn a_profile_may_not_carry_the_cap() {
    let overlay = ConfigOverlay::parse("agent_max_questions_per_turn=1\n").unwrap();
    let (carried, refused) = workspace_engine::split_profile_keys(overlay);
    assert_eq!(carried.agent_max_questions_per_turn, None);
    assert_eq!(refused.len(), 1);
    assert_eq!(refused[0].key, "agent_max_questions_per_turn");
    assert_eq!(refused[0].class, RepositoryKeyClass::Forbidden);
}

// Task 3: `ask_user` is offered only where a user can answer it, and refused
// as a tool result everywhere else. Nothing pauses yet; that is Task 4.

fn temp_repo(name: &str) -> PathBuf {
    let repo = temp_data_dir(&format!("repo-{name}"));
    fs::write(repo.join("README.md"), "# Clarification test\n").expect("a file to read");
    repo
}

/// The active provider with native tools on: `ask_user` is a native tool, and
/// without them no tool list is sent at all.
fn native_provider(id: &str) -> ModelProviderConfig {
    ModelProviderConfig {
        id: id.to_string(),
        label: id.to_string(),
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

fn engine_for(repo: &Path, max_questions: u32) -> WorkspaceEngine {
    let mut config = Config {
        data_dir: repo.join(".damaian"),
        // Throwaway repository: a watcher would only cost FSEvents registration.
        enable_index_watcher: false,
        agent_max_questions_per_turn: max_questions,
        ..Config::default()
    };
    config
        .model_providers
        .push(native_provider(&config.model_provider));
    assert!(config.supports_native_tools());
    WorkspaceEngine::new(config)
}

fn turn(
    engine: &WorkspaceEngine,
    repo: &Path,
    session_id: Option<&str>,
    user_present: bool,
    adapter: &mut dyn ModelAdapter,
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
        .ask_with_session_with_options(
            repo,
            "Clean this up.",
            &[],
            session_id,
            adapter,
            &mut sink,
            ChatTurnOptions {
                user_present,
                ..ChatTurnOptions::default()
            },
        )
        .expect("the turn should run")
}

/// The tool names the first model request of a turn offered, in a session set
/// to `mode` beforehand.
fn offered_tools(repo: &Path, mode: SessionMode, user_present: bool, cap: u32) -> Vec<String> {
    let engine = engine_for(repo, cap);
    let mut warm = MockModelAdapter::new("Ready.");
    let session = turn(&engine, repo, None, user_present, &mut warm)
        .session
        .id;
    engine
        .session_store
        .set_session_mode(&session, mode, "user")
        .unwrap();
    let mut adapter = MockModelAdapter::new("Done.");
    turn(&engine, repo, Some(&session), user_present, &mut adapter);
    adapter.requests[0]
        .tools
        .as_ref()
        .expect("the default provider offers native tools")
        .iter()
        .map(|tool| tool.name.clone())
        .collect()
}

fn ask_user_call(arguments_json: &str) -> ToolCall {
    ToolCall {
        id: "call_ask".to_string(),
        name: "ask_user".to_string(),
        arguments_json: arguments_json.to_string(),
    }
}

const CLI_OR_SHELL_CALL: &str =
    r#"{"question":"Did you mean the CLI or the shell?","options":["The CLI","The shell"]}"#;

/// The model asks once, then answers in plain text.
fn asks_then_answers(arguments_json: &str) -> MockModelAdapter {
    MockModelAdapter::new_sequence_with_tool_calls(
        vec![String::new(), "Assuming the CLI.".to_string()],
        vec![vec![ask_user_call(arguments_json)], Vec::new()],
    )
}

fn tool_results(engine: &WorkspaceEngine, session_id: &str) -> Vec<String> {
    engine
        .session_store
        .read_messages(session_id)
        .unwrap()
        .into_iter()
        .filter(|message| message.role == "tool")
        .map(|message| message.content)
        .collect()
}

#[test]
fn ask_user_is_offered_when_a_user_is_present() {
    let repo = temp_repo("offered");
    let names = offered_tools(&repo, SessionMode::Code, true, 2);
    let ask = names.iter().position(|name| name == "ask_user");
    let complete = names.iter().position(|name| name == "complete_step");
    assert!(ask.is_some(), "{names:?}");
    assert_eq!(
        ask,
        complete.map(|index| index + 1),
        "after complete_step: {names:?}"
    );
    fs::remove_dir_all(repo).unwrap();
}

#[test]
fn ask_user_is_not_offered_without_a_user() {
    let repo = temp_repo("headless");
    let names = offered_tools(&repo, SessionMode::Code, false, 2);
    assert!(!names.iter().any(|name| name == "ask_user"), "{names:?}");
    // The rest of the list is still there: this is not an empty offer.
    assert!(names.iter().any(|name| name == "read_file"), "{names:?}");
    fs::remove_dir_all(repo).unwrap();
}

#[test]
fn a_cap_of_zero_means_ask_user_is_not_offered() {
    let repo = temp_repo("cap-zero");
    let names = offered_tools(&repo, SessionMode::Code, true, 0);
    assert!(!names.iter().any(|name| name == "ask_user"), "{names:?}");
    fs::remove_dir_all(repo).unwrap();
}

#[test]
fn ask_user_is_offered_in_every_mode() {
    for mode in [
        SessionMode::Ask,
        SessionMode::Plan,
        SessionMode::Code,
        SessionMode::Review,
    ] {
        let repo = temp_repo(mode.as_str());
        let names = offered_tools(&repo, mode, true, 2);
        assert!(
            names.iter().any(|name| name == "ask_user"),
            "{mode:?}: {names:?}"
        );
        fs::remove_dir_all(repo).unwrap();
    }
}

#[test]
fn an_unoffered_ask_user_call_is_refused_as_a_tool_result_and_the_turn_continues() {
    let repo = temp_repo("unoffered");
    let engine = engine_for(&repo, 2);
    let mut adapter = asks_then_answers(CLI_OR_SHELL_CALL);
    let result = turn(&engine, &repo, None, false, &mut adapter);

    let results = tool_results(&engine, &result.session.id);
    assert_eq!(results.len(), 1, "{results:?}");
    assert!(results[0].contains("No user can answer"), "{results:?}");
    assert!(results[0].contains("assumption"), "{results:?}");
    // The turn went on to a second model call and finished on its answer.
    assert_eq!(adapter.requests.len(), 2);
    assert_eq!(result.response, "Assuming the CLI.");
    assert_eq!(result.task.status, TaskStatus::Complete);
    assert!(
        engine
            .session_store
            .pending_decision_for(&result.session.id, &result.task.id)
            .unwrap()
            .is_none()
    );
    fs::remove_dir_all(repo).unwrap();
}

/// Until Task 4 builds the pause, an offered call is refused the same way, so
/// nothing waits on an answer no route can deliver. Task 4 replaces this test.
#[test]
fn an_offered_ask_user_call_does_not_pause_yet() {
    let repo = temp_repo("offered-no-pause");
    let engine = engine_for(&repo, 2);
    let mut adapter = asks_then_answers(CLI_OR_SHELL_CALL);
    let result = turn(&engine, &repo, None, true, &mut adapter);

    let results = tool_results(&engine, &result.session.id);
    assert_eq!(results.len(), 1, "{results:?}");
    assert!(results[0].contains("No user can answer"), "{results:?}");
    assert_eq!(result.task.status, TaskStatus::Complete);
    assert!(result.command_proposal.is_none() && result.plan_proposal.is_none());
    fs::remove_dir_all(repo).unwrap();
}

#[test]
fn an_ask_user_call_over_a_bound_is_a_tool_error_naming_the_bound() {
    let repo = temp_repo("over-bound");
    let engine = engine_for(&repo, 2);
    let long = "x".repeat(QUESTION_MAX_CHARS + 1);
    let arguments = format!(r#"{{"question":"{long}","options":["a","b"]}}"#);
    let mut adapter = asks_then_answers(&arguments);
    let result = turn(&engine, &repo, None, true, &mut adapter);

    let results = tool_results(&engine, &result.session.id);
    assert_eq!(results.len(), 1, "{results:?}");
    assert!(
        results[0].contains(&format!("the limit is {QUESTION_MAX_CHARS}")),
        "{results:?}"
    );
    // A rejection, not a silent drop: the model was told and answered after.
    assert_eq!(result.response, "Assuming the CLI.");
    fs::remove_dir_all(repo).unwrap();
}

#[test]
fn an_ask_user_call_with_malformed_arguments_is_a_tool_error_not_a_drop() {
    let repo = temp_repo("malformed");
    let engine = engine_for(&repo, 2);
    let mut adapter = asks_then_answers(r#"{"question":"Which one?"}"#);
    let result = turn(&engine, &repo, None, true, &mut adapter);

    let results = tool_results(&engine, &result.session.id);
    assert_eq!(results.len(), 1, "{results:?}");
    assert!(
        results[0].contains("rejected before execution"),
        "{results:?}"
    );
    assert!(results[0].contains("options"), "{results:?}");
    fs::remove_dir_all(repo).unwrap();
}

/// Spec 21 §5.5's gate holds back the first step that changes anything until
/// the plan is reviewed. Asking changes nothing, so it must not raise a plan
/// review: the turn has a plan, it is not yet approved, and the call goes
/// through to its (for now, refusing) arm.
#[test]
fn asking_does_not_wait_for_plan_review() {
    let repo = temp_repo("plan-gate");
    let engine = engine_for(&repo, 2);
    let plan = ToolCall {
        id: "call_plan".to_string(),
        name: "propose_plan".to_string(),
        arguments_json: r#"{"steps":[{"title":"Look"},{"title":"Change"}]}"#.to_string(),
    };
    let mut adapter = MockModelAdapter::new_sequence_with_tool_calls(
        vec![
            String::new(),
            String::new(),
            "Assuming the CLI.".to_string(),
        ],
        vec![
            vec![plan],
            vec![ask_user_call(CLI_OR_SHELL_CALL)],
            Vec::new(),
        ],
    );
    let result = turn(&engine, &repo, None, true, &mut adapter);

    assert!(result.plan_proposal.is_none(), "{:?}", result.plan_proposal);
    let results = tool_results(&engine, &result.session.id);
    assert!(
        results
            .iter()
            .any(|text| text.contains("No user can answer")),
        "{results:?}"
    );
    fs::remove_dir_all(repo).unwrap();
}
