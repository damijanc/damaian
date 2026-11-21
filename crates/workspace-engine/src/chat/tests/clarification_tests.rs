use super::*;
use crate::mode::SessionMode;

// The four prompts exactly as `system_prompt` produced them before spec
// 50, copied from the live source and then re-copied when spec 63 named
// the shell in the envelope paragraphs. Where a user is present the prompt
// must not move by a byte: the tool's own description carries its
// guidance, and spec 49's cache prefix depends on `messages[0]` staying
// put. Spec 113 (D5) appends one fixed paragraph, `USER_CHOICE_PARAGRAPH`,
// for Code and Plan only; the prefix before it is still these bytes.
const PREFIX_BEFORE_SPEC_50: &str = "You are a local-first coding assistant. Answer using only the provided repository context when possible. Cite relevant file paths. Do not request or expose secrets.\n\nRepository context sections named `agent_instruction` contain AGENTS.md instructions for this repository. Follow them when they apply to the files you discuss or edit. More specific nested AGENTS.md instructions override broader ones. The user's request and Damaian's safety policy take precedence over repository instructions.";
const CODE_PARAGRAPH_BEFORE_SPEC_50: &str = "If the user asks about current Git state, recent commits, latest changes, uncommitted changes, repository history, or another fact that requires a local command, your entire response must be exactly one command request envelope. Do not add prose before or after the envelope:\nDAMAIAN_COMMAND_V1\nCOMMAND: git log -1 --stat --oneline\nREASON: Inspect the latest commit for the user's question.\nEND_COMMAND\n\nPrefer read-only commands such as git status, git log, ls, and pwd when they are sufficient, and the read_git_diff and read_git_log tools for git diffs and commit content; git diff, git show and git log -p need approval in the shell. Damaian runs a short list of read-only commands without asking and needs your approval before running anything else, including any command that uses shell control. Commands run in zsh -lc. Quote glob patterns such as '*.rs'; zsh fails a command whose unquoted glob matches nothing. When the user's task requires a command with side effects, network access, Docker access, shell control, or unknown risk, request the command and Damaian will pause for user approval before running it.";
const READ_ONLY_PARAGRAPH_BEFORE_SPEC_50: &str = "If the user asks about current Git state, recent commits, latest changes, uncommitted changes, repository history, or another fact that requires a local command, your entire response must be exactly one command request envelope. Do not add prose before or after the envelope:\nDAMAIAN_COMMAND_V1\nCOMMAND: git log -1 --stat --oneline\nREASON: Inspect the latest commit for the user's question.\nEND_COMMAND\n\nPrefer read-only commands such as git status, git log, ls, and pwd when they are sufficient, and the read_git_diff and read_git_log tools for git diffs and commit content; git diff, git show and git log -p need approval in the shell. Damaian runs a short list of read-only commands without asking and refuses every other command outright in this mode, including any command that uses shell control. Commands run in zsh -lc. Quote glob patterns such as '*.rs'; zsh fails a command whose unquoted glob matches nothing. In this mode only read-only commands that need no approval are available; a command with side effects, network access, Docker access, shell control, or unknown risk is refused outright, not queued for approval. Do not request one.";

fn prompt_before_spec_50(mode: SessionMode) -> String {
    match mode {
        SessionMode::Ask => PREFIX_BEFORE_SPEC_50.to_string(),
        SessionMode::Code => {
            format!("{PREFIX_BEFORE_SPEC_50}\n\n{CODE_PARAGRAPH_BEFORE_SPEC_50}")
        }
        SessionMode::Plan | SessionMode::Review => {
            format!("{PREFIX_BEFORE_SPEC_50}\n\n{READ_ONLY_PARAGRAPH_BEFORE_SPEC_50}")
        }
    }
}

const ALL_MODES: [SessionMode; 4] = [
    SessionMode::Ask,
    SessionMode::Plan,
    SessionMode::Code,
    SessionMode::Review,
];

#[test]
fn with_a_user_present_only_code_and_plan_gain_the_choice_paragraph() {
    for mode in ALL_MODES {
        let expected = match mode {
            SessionMode::Code | SessionMode::Plan => {
                format!("{}\n\n{USER_CHOICE_PARAGRAPH}", prompt_before_spec_50(mode))
            }
            SessionMode::Ask | SessionMode::Review => prompt_before_spec_50(mode),
        };
        assert_eq!(system_prompt(mode, true, "/bin/zsh"), expected, "{mode:?}");
    }
}

/// Spec 113 requirement 4: a choice is asked with `ask_user` before a plan,
/// never written as options into a plan or an answer.
#[test]
fn the_choice_guidance_sends_choices_to_ask_user() {
    assert!(USER_CHOICE_PARAGRAPH.contains("`ask_user`"));
    assert!(USER_CHOICE_PARAGRAPH.contains("2 to 4"));
    assert!(USER_CHOICE_PARAGRAPH.contains("before proposing a plan"));
    assert!(USER_CHOICE_PARAGRAPH.contains("Never write options"));
    assert!(USER_CHOICE_PARAGRAPH.contains("into a plan or an answer"));

    let plan = propose_plan_tool_definition().description;
    assert!(
        plan.contains("Ask a choice the user must make with `ask_user` before proposing"),
        "{plan}"
    );
    assert!(plan.contains("never as options in a plan"));

    let ask = ask_user_tool_definition().description;
    assert!(
        ask.contains("Ask a choice the user must make here, before any plan"),
        "{ask}"
    );
    assert!(ask.contains("never as options in a plan or an answer"));

    // Without a user present nobody can pick an option, so neither the
    // headless nor the worker prompt carries the paragraph.
    for mode in ALL_MODES {
        assert!(!system_prompt(mode, false, "/bin/zsh").contains(USER_CHOICE_PARAGRAPH));
        assert!(
            !system_prompt_with_capabilities(mode, true, false, "/bin/zsh")
                .contains(USER_CHOICE_PARAGRAPH)
        );
    }
}

#[test]
fn without_a_user_the_prompt_gains_only_the_headless_paragraph() {
    assert!(HEADLESS_PARAGRAPH.contains("assumption"));
    assert!(HEADLESS_PARAGRAPH.contains("materially"));
    assert!(HEADLESS_PARAGRAPH.contains("incomplete"));
    for mode in ALL_MODES {
        assert_eq!(
            system_prompt(mode, false, "/bin/zsh"),
            format!("{}\n\n{HEADLESS_PARAGRAPH}", prompt_before_spec_50(mode)),
            "{mode:?}"
        );
    }
}

#[test]
fn a_worker_route_keeps_the_assumption_rule_and_asks_for_material_ambiguity() {
    assert!(WORKER_QUESTION_PARAGRAPH.contains("explicitly stated assumption"));
    assert!(WORKER_QUESTION_PARAGRAPH.contains("materially change"));
    assert!(WORKER_QUESTION_PARAGRAPH.contains("ask_user"));
    for mode in ALL_MODES {
        let prompt = system_prompt_with_capabilities(mode, true, false, "/bin/zsh");
        assert_eq!(
            prompt,
            format!(
                "{}\n\n{WORKER_QUESTION_PARAGRAPH}",
                prompt_before_spec_50(mode)
            ),
            "{mode:?}"
        );
        assert!(
            !prompt.contains(HEADLESS_PARAGRAPH),
            "a worker has an answer route, so it is not headless: {mode:?}"
        );
    }
}

fn cli_or_shell() -> ToolAction {
    ToolAction::AskUser(AskUserInput {
        question: "Did you mean the CLI or the shell?".to_string(),
        options: vec!["The CLI".to_string(), "The shell".to_string()],
        allow_free_text: false,
    })
}

/// Asking writes nothing outside the session log and is never run beside
/// another call: it can end the turn's progress until answered (Task 4).
#[test]
fn asking_shapes_the_turn_and_has_no_side_effect() {
    let action = cli_or_shell();
    assert_eq!(action_effect(&action), ActionEffect::ShapesTurn);
    assert!(!action_is_batchable_read_only(&action));
    assert_eq!(tool_action_marker(&action).0, "ask_user");
    assert!(!tool_action_marker(&action).2);
}

/// An answer authorises nothing, so asking is not a step that changes
/// anything and must not raise spec 21's plan review.
#[test]
fn asking_never_awaits_plan_review() {
    assert!(!action_awaits_plan_review(&cli_or_shell(), |_| true));
}

fn a_memory_suggestion() -> ToolAction {
    ToolAction::ProposeMemory {
        statement: "Integration tests use nextest.".to_string(),
        category: MemoryCategory::Convention,
        scope: None,
        source: None,
        evidence_path: None,
    }
}

#[test]
fn a_memory_suggestion_is_a_session_log_write_and_never_a_repository_change() {
    let action = a_memory_suggestion();
    assert_eq!(action_effect(&action), ActionEffect::Notes);
    assert!(action_is_batchable_read_only(&action));
    assert_eq!(tool_action_marker(&action).0, "propose_memory");
    assert!(!tool_action_marker(&action).2);
    assert_eq!(tool_action_label(&action), "Suggesting a memory");
}

// Spec 107 requirement 3: the status line names the lines a ranged read
// covers, and a command that runs is "Running", not still "Proposing".
#[test]
fn activity_labels_name_a_read_range_and_a_running_command() {
    let ranged = ToolAction::ReadFile {
        path: "src/config.rs".to_string(),
        range: Some(crate::file_access::LineRange {
            start: 1200,
            end: 1600,
        }),
        worktree: None,
    };
    assert_eq!(
        tool_action_label(&ranged),
        "Reading src/config.rs 1200–1600"
    );
    let whole = ToolAction::ReadFile {
        path: "src/config.rs".to_string(),
        range: None,
        worktree: None,
    };
    assert_eq!(tool_action_label(&whole), "Reading src/config.rs");
    assert_eq!(
        running_command_label("cargo test --test permission_profiles"),
        "Running `cargo test --test permission_profiles`"
    );
}

#[test]
fn propose_memory_parses_its_arguments_and_rejects_unknown_vocabulary() {
    let parsed = tool_action_from_call(&ToolCall {
        id: "call_1".to_string(),
        name: "propose_memory".to_string(),
        arguments_json: serde_json::json!({
            "statement": "Integration tests use nextest.",
            "category": "convention",
            "scope": "project",
            "source": "issue #412",
        })
        .to_string(),
    })
    .unwrap()
    .expect("a decoded call");
    assert!(matches!(
        parsed,
        ToolAction::ProposeMemory {
            scope: Some(MemoryScope::Project),
            category: MemoryCategory::Convention,
            ..
        }
    ));

    let malformed = tool_action_from_call(&ToolCall {
        id: "call_2".to_string(),
        name: "propose_memory".to_string(),
        arguments_json: serde_json::json!({
            "statement": "A fact.",
            "category": "guess",
        })
        .to_string(),
    });
    assert!(malformed.is_err(), "an unknown category is named");
}

#[test]
fn the_propose_memory_schema_restates_the_ceiling() {
    let definition = propose_memory_tool_definition();
    let schema: serde_json::Value = serde_json::from_str(&definition.parameters_json).unwrap();
    assert_eq!(
        schema["properties"]["statement"]["maxLength"],
        crate::memory::STATEMENT_MAX_CHARS
    );
    assert!(definition.description.contains("Nothing is stored"));
    assert!(definition.description.contains("protected or ignored"));
}

#[test]
fn the_definition_restates_the_bounds() {
    let definition = ask_user_tool_definition();
    let schema: serde_json::Value = serde_json::from_str(&definition.parameters_json).unwrap();
    let properties = &schema["properties"];
    assert_eq!(properties["question"]["maxLength"], QUESTION_MAX_CHARS);
    assert_eq!(properties["options"]["minItems"], OPTIONS_MIN);
    assert_eq!(properties["options"]["maxItems"], OPTIONS_MAX);
    assert_eq!(
        properties["options"]["items"]["maxLength"],
        OPTION_MAX_CHARS
    );
    // The schema's keys are the ones the parser reads.
    let parsed: AskUserInput = serde_json::from_value(serde_json::json!({
        "question": "Which?",
        "options": ["a", "b"],
        "allowFreeText": true,
    }))
    .unwrap();
    assert!(parsed.allow_free_text);
    assert!(properties.get("allowFreeText").is_some());
    assert!(definition.description.contains("authorises nothing"));
}

#[test]
fn propose_plan_parses_a_build_check_that_is_never_required() {
    let parsed = tool_action_from_call(&ToolCall {
        id: "call_1".to_string(),
        name: "propose_plan".to_string(),
        arguments_json: serde_json::json!({
            "steps": [
                {
                    "title": "Change the engine",
                    "build": {
                        "command": " cargo check -p workspace-engine --tests ",
                        "workingDirectory": "crates/workspace-engine",
                        "purpose": "from CI",
                        "required": true,
                    },
                },
                { "title": "Write the report", "build": { "purpose": "no command" } },
            ],
        })
        .to_string(),
    })
    .unwrap()
    .expect("a decoded plan");

    let ToolAction::ProposePlan(steps) = parsed else {
        panic!("expected a plan");
    };
    let build = steps[0].build.as_ref().expect("the first step's build");
    assert_eq!(build.version, PLANNED_CHECK_VERSION);
    assert_eq!(build.command, "cargo check -p workspace-engine --tests");
    assert_eq!(build.working_directory, "crates/workspace-engine");
    assert_eq!(build.purpose, "from CI");
    assert!(!build.required, "a build check is feedback, never required");
    assert!(steps[0].check.is_none());
    assert!(
        steps[1].build.is_none(),
        "a build without a command is dropped"
    );
    // The tool asks for one per step.
    let definition = propose_plan_tool_definition();
    assert!(definition.parameters_json.contains("\"build\""));
    assert!(
        definition
            .description
            .contains("give code-changing steps a `build`")
    );
    // Spec 109 D9: checks come from what the repository declares, as single
    // commands, with their source named.
    assert!(definition.description.contains(
        "from the repository's CI files, Makefile/justfile or README as one command (no && chains) and say in `purpose` where"
    ));
}

#[test]
fn propose_plan_parses_a_structured_check() {
    let parsed = tool_action_from_call(&ToolCall {
        id: "call_1".to_string(),
        name: "propose_plan".to_string(),
        arguments_json: serde_json::json!({
            "steps": [
                {
                    "title": "Run the engine suite",
                    "check": {
                        "command": "cargo nextest run -p workspace-engine",
                        "workingDirectory": "",
                        "purpose": "validates the final tree",
                        "required": true,
                    },
                },
                { "title": "Write the report" },
            ],
        })
        .to_string(),
    })
    .unwrap()
    .expect("a decoded plan");

    let ToolAction::ProposePlan(steps) = parsed else {
        panic!("expected a plan");
    };
    assert_eq!(steps.len(), 2);
    let check = steps[0]
        .check
        .as_ref()
        .expect("the first step carries a check");
    assert_eq!(check.version, PLANNED_CHECK_VERSION);
    assert_eq!(check.command, "cargo nextest run -p workspace-engine");
    assert_eq!(check.working_directory, "");
    assert_eq!(check.purpose, "validates the final tree");
    assert!(check.required);
    // A step without a check parses to none.
    assert!(steps[1].check.is_none());
}

#[test]
fn propose_plan_drops_a_check_without_a_command_and_defaults_missing_fields() {
    // A proposed check authorises nothing on its own (spec 105 §2); one
    // without a command is not a check, so it is dropped the same way a
    // step with no title is. The remaining optional fields default: empty
    // directory is the repository root, and `required` is true.
    let parsed = tool_action_from_call(&ToolCall {
        id: "call_1".to_string(),
        name: "propose_plan".to_string(),
        arguments_json: serde_json::json!({
            "steps": [
                { "title": "First", "check": { "purpose": "no command here" } },
                {
                    "title": "Second",
                    "check": { "command": "cargo test" },
                },
            ],
        })
        .to_string(),
    })
    .unwrap()
    .expect("a decoded plan");

    let ToolAction::ProposePlan(steps) = parsed else {
        panic!("expected a plan");
    };
    assert!(steps[0].check.is_none(), "{:?}", steps[0].check);
    let check = steps[1]
        .check
        .as_ref()
        .expect("the second step carries a check");
    assert_eq!(check.command, "cargo test");
    assert_eq!(check.working_directory, "");
    assert_eq!(check.purpose, "");
    assert!(check.required);
}

#[test]
fn a_proposed_check_never_carries_an_approval_field() {
    // Spec 105 requirement 8: repository text — task instructions or
    // `AGENTS.md` — can name a check, but only the model's `propose_plan`
    // output becomes a `PlannedCheck`, and that output is a proposal the
    // user reviews. The schema offers no way to mark a check approved or
    // trusted, and the parser reads only the four structured fields, so an
    // extra "approved"/"trusted" key a repository string might smuggle in is
    // ignored rather than honoured. The type has no authority bit: a
    // `PlannedCheck` is data the user approves, never a command already
    // authorised.
    let parsed = tool_action_from_call(&ToolCall {
        id: "call_1".to_string(),
        name: "propose_plan".to_string(),
        arguments_json: serde_json::json!({
            "steps": [
                { "title": "First" },
                {
                    "title": "Second",
                    "check": {
                        "command": "cargo test",
                        "workingDirectory": "",
                        "purpose": "validates",
                        "required": true,
                        "approved": true,
                        "trusted": true,
                    },
                },
            ],
        })
        .to_string(),
    })
    .unwrap()
    .expect("a decoded plan");

    let ToolAction::ProposePlan(steps) = parsed else {
        panic!("expected a plan");
    };
    let check = steps[1]
        .check
        .as_ref()
        .expect("the second step carries a check");
    // The parsed check is exactly a `PlannedCheck`; the smuggled approval
    // keys have nowhere to land.
    assert_eq!(
        *check,
        PlannedCheck {
            version: PLANNED_CHECK_VERSION,
            command: "cargo test".to_string(),
            working_directory: String::new(),
            purpose: "validates".to_string(),
            required: true,
        }
    );

    // The schema offers no approval field for a check either, so the model
    // cannot even *ask* that a check be pre-approved.
    let definition = propose_plan_tool_definition();
    let schema: serde_json::Value = serde_json::from_str(&definition.parameters_json).unwrap();
    let check_props = schema["properties"]["steps"]["items"]["properties"]["check"]["properties"]
        .as_object()
        .expect("the check schema is an object");
    for key in ["command", "workingDirectory", "purpose", "required"] {
        assert!(
            check_props.contains_key(key),
            "the check schema is missing {key}: {check_props:?}"
        );
    }
    for key in ["approved", "trusted", "auto", "allow"] {
        assert!(
            !check_props.contains_key(key),
            "the check schema must not offer an approval field {key}: {check_props:?}"
        );
    }
    assert!(
        definition
            .parameters_json
            .contains("authorises nothing on its own"),
        "the check's own schema description says it authorises nothing"
    );
}
