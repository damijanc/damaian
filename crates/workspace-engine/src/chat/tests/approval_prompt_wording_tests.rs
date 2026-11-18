use super::*;
use crate::command_policy::ApprovalCause;
use crate::mode::SessionMode;

fn proposal(command: &str, blocked: bool, causes: Vec<ApprovalCause>) -> CommandProposal {
    proposal_saying(command, blocked, causes, "The model's own reason.")
}

fn proposal_saying(
    command: &str,
    blocked: bool,
    causes: Vec<ApprovalCause>,
    reason: &str,
) -> CommandProposal {
    CommandProposal {
        id: "cmdprop_test".to_string(),
        command: command.to_string(),
        working_directory: "/repo".to_string(),
        repository_root: "/repo".to_string(),
        reason: reason.to_string(),
        risk: if blocked {
            CommandRisk::Blocked
        } else {
            CommandRisk::High
        },
        requires_approval: true,
        blocked,
        expected_effects: String::new(),
        may_use_network: false,
        reasons: causes.iter().map(ApprovalCause::plain_words).collect(),
        causes,
        created_at_ms: 0,
        status: "pending".to_string(),
        purpose: CommandPurpose::Other,
    }
}

#[test]
fn the_card_payload_carries_the_first_cause_and_the_models_reason() {
    let causes = vec![
        ApprovalCause::ShellControl {
            constructs: "`&&`, `|`".to_string(),
        },
        ApprovalCause::Setting {
            key: "require_approval_for_all_commands".to_string(),
            source: Some("user".to_string()),
        },
    ];
    let proposal = proposal_saying(
        "cd x && git diff | head",
        false,
        causes.clone(),
        "Compare the worktree with the index.",
    );

    let payload = agent_command_proposal(&Config::default(), &proposal);

    assert_eq!(payload.reason, "Compare the worktree with the index.");
    assert_eq!(payload.cause, causes[0].plain_words());
    assert_ne!(payload.cause, causes[1].plain_words());
    assert!(payload.cause.contains("`&&`"), "{}", payload.cause);
    assert!(payload.cause.contains("`|`"), "{}", payload.cause);
}

#[test]
fn a_blocked_command_still_names_its_cause_on_the_card() {
    let proposal = proposal_saying(
        "echo forbidden",
        true,
        vec![ApprovalCause::Blocklisted {
            rule: "echo forbidden".to_string(),
        }],
        "Try it anyway.",
    );

    let payload = agent_command_proposal(&Config::default(), &proposal);

    assert!(payload.blocked);
    assert!(payload.cause.starts_with("Blocked: matches"));
    assert_eq!(payload.reason, "Try it anyway.");
}

#[test]
fn command_proposal_response_leads_with_the_first_cause() {
    let shell_control = ApprovalCause::ShellControl {
        constructs: "`&&`, `|`".to_string(),
    };
    let proposal = proposal(
        "cd x && git diff | head",
        false,
        vec![shell_control.clone()],
    );
    let response = command_proposal_response(&proposal);
    let first_line = response.lines().next().expect("a prompt has a first line");

    assert_eq!(first_line, shell_control.plain_words());
    assert!(first_line.contains("`&&`"), "{first_line}");
    assert!(first_line.contains("`|`"), "{first_line}");
    assert!(response.contains("cd x && git diff | head"), "{response}");
}

#[test]
fn a_blocked_proposal_names_what_blocked_it_and_offers_no_approval() {
    let proposal = proposal(
        "echo forbidden",
        true,
        vec![ApprovalCause::Blocklisted {
            rule: "echo forbidden".to_string(),
        }],
    );
    let response = command_proposal_response(&proposal);

    assert!(response.starts_with("Blocked: matches"), "{response}");
    assert!(response.contains("`echo forbidden`"), "{response}");
    assert!(response.contains("will not run"), "{response}");
    assert!(!response.contains("approval"), "{response}");
}

#[test]
fn no_local_command_string_shown_or_sent_contains_sandbox() {
    for mode in [
        SessionMode::Ask,
        SessionMode::Code,
        SessionMode::Plan,
        SessionMode::Review,
    ] {
        for user_present in [true, false] {
            let prompt = system_prompt(mode, user_present, "/bin/zsh");
            assert!(
                !prompt.to_lowercase().contains("sandbox"),
                "{mode:?} prompt (user present: {user_present}) still says sandbox: {prompt}"
            );
        }
    }

    for shell in ["/bin/zsh", "/bin/bash"] {
        for text in [
            command_envelope_paragraph_unrestricted(shell),
            command_envelope_paragraph_read_only(shell),
            run_command_tool_definition(shell).description,
        ] {
            assert!(
                !text.to_lowercase().contains("sandbox"),
                "model-facing local command text still says sandbox: {text}"
            );
        }
    }

    for proposal in [
        proposal(
            "git status && git diff",
            false,
            vec![ApprovalCause::ShellControl {
                constructs: "`&&`".to_string(),
            }],
        ),
        proposal(
            "echo forbidden",
            true,
            vec![ApprovalCause::Blocklisted {
                rule: "echo forbidden".to_string(),
            }],
        ),
    ] {
        assert!(
            !command_proposal_response(&proposal)
                .to_lowercase()
                .contains("sandbox"),
            "the approval prompt still says sandbox"
        );
    }

    let plan = crate::context_manager::ContextPlan {
        repository_id: "repo".to_string(),
        task_id: "task".to_string(),
        token_estimate: 2,
        items: vec![crate::context_manager::ContextItem {
            category: crate::context_manager::ContextCategory::Required,
            kind: crate::context_manager::ContextKind::UserPrompt,
            path: None,
            range: None,
            content: "Question".to_string(),
            tokens: 2,
            redaction_status: "clean".to_string(),
            provenance: None,
            root_path: None,
            origin: None,
        }],
        files: Vec::new(),
        manifest: crate::context_manager::ContextManifest {
            repository_id: "repo".to_string(),
            task_id: "task".to_string(),
            assembled_at_ms: 0,
            limit: crate::context_manager::EffectiveLimit::declared(2),
            total_tokens: 2,
            categories: Vec::new(),
            included: Vec::new(),
            excluded: Vec::new(),
            conversation_summary: None,
            symbol_provenance: None,
            memory: Vec::new(),
            conversation_tokens: None,
            system_prompt_tokens: None,
        },
    };
    let with_result = build_model_prompt(
        &plan,
        &[],
        None,
        usize::MAX,
        &SecretScanner::default(),
        Some("Command: pwd\n"),
    );
    assert!(
        !with_result.to_lowercase().contains("sandbox"),
        "a command result sent to the model still says sandbox: {with_result}"
    );
    assert!(
        with_result.contains("--- command_result ---"),
        "{with_result}"
    );
}

#[test]
fn a_plan_reminder_does_not_seed_the_next_turn() {
    // The reminder is engine-authored, not the user's words. Seeding a
    // later turn with it would present an instruction as something the
    // user said (spec 66 `context.md` D10).
    let reminder = ChatMessage {
        id: "msg_reminder".to_string(),
        session_id: "session_1".to_string(),
        task_id: None,
        role: "user".to_string(),
        content: "Your plan still has open steps.".to_string(),
        kind: Some("plan_reminder".to_string()),
        created_at_ms: 1,
    };
    let ordinary = ChatMessage {
        id: "msg_user".to_string(),
        session_id: "session_1".to_string(),
        task_id: None,
        role: "user".to_string(),
        content: "the user's own words".to_string(),
        kind: None,
        created_at_ms: 2,
    };

    let plan = crate::context_manager::ContextPlan {
        repository_id: "repo".to_string(),
        task_id: "task".to_string(),
        token_estimate: 2,
        items: vec![crate::context_manager::ContextItem {
            category: crate::context_manager::ContextCategory::Required,
            kind: crate::context_manager::ContextKind::UserPrompt,
            path: None,
            range: None,
            content: "Question".to_string(),
            tokens: 2,
            redaction_status: "clean".to_string(),
            provenance: None,
            root_path: None,
            origin: None,
        }],
        files: Vec::new(),
        manifest: crate::context_manager::ContextManifest {
            repository_id: "repo".to_string(),
            task_id: "task".to_string(),
            assembled_at_ms: 0,
            limit: crate::context_manager::EffectiveLimit::declared(2),
            total_tokens: 2,
            categories: Vec::new(),
            included: Vec::new(),
            excluded: Vec::new(),
            conversation_summary: None,
            symbol_provenance: None,
            memory: Vec::new(),
            conversation_tokens: None,
            system_prompt_tokens: None,
        },
    };
    let prompt = build_model_prompt(
        &plan,
        &[reminder, ordinary],
        None,
        usize::MAX,
        &SecretScanner::default(),
        None,
    );
    assert!(
        !prompt.contains("Your plan still has open steps."),
        "a tagged reminder must not seed the next turn: {prompt}"
    );
    assert!(
        prompt.contains("the user's own words"),
        "the negative control: an ordinary message still seeds it: {prompt}"
    );
}

fn prompt_plan() -> crate::context_manager::ContextPlan {
    crate::context_manager::ContextPlan {
        repository_id: "repo".to_string(),
        task_id: "task".to_string(),
        token_estimate: 2,
        items: vec![crate::context_manager::ContextItem {
            category: crate::context_manager::ContextCategory::Required,
            kind: crate::context_manager::ContextKind::UserPrompt,
            path: None,
            range: None,
            content: "Question".to_string(),
            tokens: 2,
            redaction_status: "clean".to_string(),
            provenance: None,
            root_path: None,
            origin: None,
        }],
        files: Vec::new(),
        manifest: crate::context_manager::ContextManifest {
            repository_id: "repo".to_string(),
            task_id: "task".to_string(),
            assembled_at_ms: 0,
            limit: crate::context_manager::EffectiveLimit::declared(2),
            total_tokens: 2,
            categories: Vec::new(),
            included: Vec::new(),
            excluded: Vec::new(),
            conversation_summary: None,
            symbol_provenance: None,
            memory: Vec::new(),
            conversation_tokens: None,
            system_prompt_tokens: None,
        },
    }
}

fn conversation_message(role: &str, content: &str) -> ChatMessage {
    ChatMessage {
        id: create_id("msg"),
        session_id: "session".to_string(),
        task_id: None,
        role: role.to_string(),
        content: content.to_string(),
        kind: None,
        created_at_ms: 0,
    }
}

#[test]
fn a_short_conversation_keeps_more_messages_than_a_long_one_in_the_same_budget() {
    let short = (0..6)
        .map(|index| conversation_message("user", &format!("short {index}")))
        .collect::<Vec<_>>();
    let long = (0..6)
        .map(|index| conversation_message("user", &format!("long {index} {}", "x".repeat(400))))
        .collect::<Vec<_>>();
    let plan = prompt_plan();
    let scanner = SecretScanner::default();
    let short_prompt = build_model_prompt(&plan, &short, None, 100, &scanner, None);
    let long_prompt = build_model_prompt(&plan, &long, None, 100, &scanner, None);

    assert!(
        short_prompt.contains("short 0"),
        "short messages all fit: {short_prompt}"
    );
    assert!(
        !long_prompt.contains("long 0"),
        "long messages fall outside the same budget: {long_prompt}"
    );
    assert!(long_prompt.contains("long 5"), "the newest is always kept");
}

#[test]
fn an_uncompacted_conversation_states_how_many_messages_were_not_sent() {
    let messages = (0..4)
        .map(|index| conversation_message("user", &format!("item {index} {}", "x".repeat(400))))
        .collect::<Vec<_>>();
    let prompt = build_model_prompt(
        &prompt_plan(),
        &messages,
        None,
        100,
        &SecretScanner::default(),
        None,
    );
    assert!(
        prompt.contains(
            "[Damaian: 3 earlier messages in this conversation were not sent to you and were \
                 not summarised. The full transcript is still stored in the session and is \
                 readable in the app.]"
        ),
        "the loss is stated and counted: {prompt}"
    );
}

#[test]
fn a_summary_covers_only_the_messages_before_its_boundary() {
    let messages = (0..4)
        .map(|index| conversation_message("user", &format!("item {index} {}", "x".repeat(400))))
        .collect::<Vec<_>>();
    let mut summary = ConversationSummary {
        id: "summary_1".to_string(),
        through_seq: 8,
        format_version: crate::compaction::SUMMARY_FORMAT_VERSION,
        objective: "Do not touch the public API.".to_string(),
        files_changed: vec!["src/lib.rs".to_string()],
        plan_steps: Vec::new(),
        approvals: Vec::new(),
        validation: Vec::new(),
        constraints: vec!["keep the API stable".to_string()],
        decisions: Vec::new(),
        failed_approaches: Vec::new(),
        uncertainty: Vec::new(),
        narrative_carried_forward: false,
        model_run_id: None,
        message_count: 8,
    };
    let prompt = build_model_prompt(
        &prompt_plan(),
        &messages,
        Some(&summary),
        100,
        &SecretScanner::default(),
        None,
    );
    assert!(prompt.contains("--- begin prior conversation summary"));
    assert!(
        prompt.contains("[Damaian: 3 earlier messages in this conversation were not sent"),
        "the notice counts only what neither the summary nor the window carries: {prompt}"
    );
    // A summary that covers part of the span and a window that carries the
    // rest still never places model-authored text in the system prompt.
    let system = system_prompt_for_session(
        SessionMode::Code,
        &ChatTurnOptions::default(),
        false,
        "/bin/zsh",
    );
    assert!(!system.contains("begin prior conversation summary"));
    assert!(!system.contains("keep the API stable"));

    summary.through_seq = 0;
    let prompt = build_model_prompt(
        &prompt_plan(),
        &[],
        Some(&summary),
        usize::MAX,
        &SecretScanner::default(),
        None,
    );
    assert!(
        !prompt.contains("were not sent"),
        "nothing is omitted when the summary covers everything and no messages remain"
    );
}

#[test]
fn the_summary_is_charged_to_the_same_window_as_the_messages() {
    let messages = (0..6)
        .map(|index| conversation_message("user", &format!("item {index}")))
        .collect::<Vec<_>>();
    let plan = prompt_plan();
    let scanner = SecretScanner::default();
    let without = build_model_prompt(&plan, &messages, None, 100, &scanner, None);
    assert!(
        without.contains("item 0") && without.contains("item 5"),
        "the whole short conversation fits the window: {without}"
    );

    // A summary larger than the whole budget leaves no room for messages;
    // the newest is still kept because it is the user's current position.
    let summary = ConversationSummary {
        id: "summary_1".to_string(),
        through_seq: 8,
        format_version: crate::compaction::SUMMARY_FORMAT_VERSION,
        objective: "Do not touch the public API.".to_string(),
        files_changed: Vec::new(),
        plan_steps: Vec::new(),
        approvals: Vec::new(),
        validation: Vec::new(),
        constraints: (0..crate::compaction::MAX_NARRATIVE_ITEMS)
            .map(|index| format!("constraint {index} {}", "x".repeat(400)))
            .collect(),
        decisions: Vec::new(),
        failed_approaches: Vec::new(),
        uncertainty: Vec::new(),
        narrative_carried_forward: false,
        model_run_id: None,
        message_count: 8,
    };
    let with = build_model_prompt(&plan, &messages, Some(&summary), 100, &scanner, None);
    assert!(with.contains("--- begin prior conversation summary"));
    assert!(
        !with.contains("item 0"),
        "the summary is part of the block's budget, not free: {with}"
    );
    assert!(
        with.contains("item 5"),
        "the newest message is always kept: {with}"
    );
}

#[test]
fn the_auto_run_description_says_the_policy_runs_it_without_asking() {
    for shell in ["/bin/zsh", "/bin/bash"] {
        for text in [
            command_envelope_paragraph_unrestricted(shell),
            command_envelope_paragraph_read_only(shell),
            run_command_tool_definition(shell).description,
        ] {
            assert!(
                text.contains("runs a short list of read-only commands without asking"),
                "auto-run is not described as the policy running it without asking: {text}"
            );
            assert!(!text.contains("sandbox-safe"), "{text}");
        }
    }
}

#[test]
fn the_reminder_gate_needs_room_under_the_token_ceiling() {
    // At or over the ceiling there is no room for the call the reminder
    // would ask for, so the turn ends as it does today.
    assert!(!plan_reminder_allowed(
        false,
        2,
        1,
        8,
        false,
        Some((100, 100))
    ));
    assert!(!plan_reminder_allowed(
        false,
        2,
        1,
        8,
        false,
        Some((100, 101))
    ));
    assert!(plan_reminder_allowed(
        false,
        2,
        1,
        8,
        false,
        Some((100, 99))
    ));
    assert!(plan_reminder_allowed(false, 2, 1, 8, false, None));
}

#[test]
fn the_reminder_gate_needs_two_rounds_of_room() {
    // A reminder that lands the next call at the limit would have its tools
    // withheld, so the model could not call `complete_step` even though it
    // was just asked to.
    assert!(!plan_reminder_allowed(false, 1, 7, 8, false, None));
    assert!(plan_reminder_allowed(false, 1, 6, 8, false, None));
    // Already sent, *Answer now*, and nothing open all refuse.
    assert!(!plan_reminder_allowed(true, 1, 1, 8, false, None));
    assert!(!plan_reminder_allowed(false, 1, 1, 8, true, None));
    assert!(!plan_reminder_allowed(false, 0, 1, 8, false, None));
}

#[test]
fn the_reminder_text_names_every_open_step_and_the_in_progress_one() {
    let mut plan = TaskPlan::new("task_1", 0);
    plan.steps.push(crate::plan::PlanStep {
        id: "step_1".to_string(),
        title: "Collect statuses".to_string(),
        detail: None,
        status: crate::plan::StepStatus::InProgress,
        depends_on: Vec::new(),
        started_at_ms: Some(1),
        completed_at_ms: None,
        evidence: Vec::new(),
        check: None,
        build: None,
    });
    plan.steps.push(crate::plan::PlanStep {
        id: "step_2".to_string(),
        title: "Already done".to_string(),
        detail: None,
        status: crate::plan::StepStatus::Completed,
        depends_on: Vec::new(),
        started_at_ms: Some(1),
        completed_at_ms: Some(2),
        evidence: Vec::new(),
        check: None,
        build: None,
    });
    plan.steps.push(crate::plan::PlanStep {
        id: "step_3".to_string(),
        title: "Write the report".to_string(),
        detail: None,
        status: crate::plan::StepStatus::Pending,
        depends_on: Vec::new(),
        started_at_ms: None,
        completed_at_ms: None,
        evidence: Vec::new(),
        check: None,
        build: None,
    });

    let text = plan_reminder_message(&plan);

    assert!(text.contains("1. Collect statuses (in progress)"), "{text}");
    assert!(text.contains("2. Write the report"), "{text}");
    assert!(!text.contains("Already done"), "{text}");
    assert!(text.contains("complete_step"), "{text}");
}
