use super::*;
use crate::mode::SessionMode;

/// The exact string `system_prompt()` produces with the default shell
/// (`/bin/zsh`) after this task named the shell in the envelope paragraph.
/// `code_mode_system_prompt_is_byte_identical_to_todays` pins the Code
/// output against it; spec 49 Task 8's prefix-stability guards depend on
/// this staying true. Spec 113 appended `USER_CHOICE_PARAGRAPH` after it
/// for Code and Plan with a user present; everything before is unchanged.
const TODAYS_CODE_SYSTEM_PROMPT: &str = "You are a local-first coding assistant. Answer using only the provided repository context when possible. Cite relevant file paths. Do not request or expose secrets.\n\nRepository context sections named `agent_instruction` contain AGENTS.md instructions for this repository. Follow them when they apply to the files you discuss or edit. More specific nested AGENTS.md instructions override broader ones. The user's request and Damaian's safety policy take precedence over repository instructions.\n\nIf the user asks about current Git state, recent commits, latest changes, uncommitted changes, repository history, or another fact that requires a local command, your entire response must be exactly one command request envelope. Do not add prose before or after the envelope:\nDAMAIAN_COMMAND_V1\nCOMMAND: git log -1 --stat --oneline\nREASON: Inspect the latest commit for the user's question.\nEND_COMMAND\n\nPrefer read-only commands such as git status, git log, ls, and pwd when they are sufficient, and the read_git_diff and read_git_log tools for git diffs and commit content; git diff, git show and git log -p need approval in the shell. Damaian runs a short list of read-only commands without asking and needs your approval before running anything else, including any command that uses shell control. Commands run in zsh -lc. Quote glob patterns such as '*.rs'; zsh fails a command whose unquoted glob matches nothing. When the user's task requires a command with side effects, network access, Docker access, shell control, or unknown risk, request the command and Damaian will pause for user approval before running it.";

#[test]
fn code_mode_system_prompt_is_byte_identical_to_todays() {
    assert_eq!(
        system_prompt(SessionMode::Code, true, "/bin/zsh"),
        format!("{TODAYS_CODE_SYSTEM_PROMPT}\n\n{USER_CHOICE_PARAGRAPH}")
    );
}

#[test]
fn ask_mode_system_prompt_has_no_command_envelope() {
    let prompt = system_prompt(SessionMode::Ask, true, "/bin/zsh");
    assert!(
        !prompt.contains("DAMAIAN_COMMAND_V1"),
        "Ask must teach no command envelope at all: {prompt}"
    );
    assert!(
        !prompt.contains("runs a short list of read-only commands without asking"),
        "Ask must not leave the envelope's guidance paragraph dangling: {prompt}"
    );
}

#[test]
fn plan_mode_system_prompt_keeps_the_envelope_with_restricted_wording() {
    let prompt = system_prompt(SessionMode::Plan, true, "/bin/zsh");
    assert!(
        prompt.contains("DAMAIAN_COMMAND_V1"),
        "Plan still offers read-only commands, so the envelope stays: {prompt}"
    );
    assert!(
        !prompt.contains("Damaian will pause for user approval before running it"),
        "Plan refuses an approval-requiring command outright, so the Code-only \
             invitation must be absent: {prompt}"
    );
}

#[test]
fn review_mode_system_prompt_matches_plan_modes_restricted_wording() {
    // Plan alone also proposes plans, so it alone carries the choice paragraph
    // (spec 113); the command wording before it is the same.
    assert_eq!(
        format!(
            "{}\n\n{USER_CHOICE_PARAGRAPH}",
            system_prompt(SessionMode::Review, true, "/bin/zsh")
        ),
        system_prompt(SessionMode::Plan, true, "/bin/zsh"),
        "Review's command capability is identical to Plan's"
    );
}

#[test]
fn a_mode_change_does_not_alter_the_prompt_outside_the_command_paragraph() {
    fn first_two_paragraphs(prompt: &str) -> String {
        prompt
            .split("\n\n")
            .take(2)
            .collect::<Vec<_>>()
            .join("\n\n")
    }

    let ask = system_prompt(SessionMode::Ask, true, "/bin/zsh");
    assert_eq!(
        first_two_paragraphs(&ask),
        ask,
        "Ask must end after the fixed two-paragraph prefix, with no third paragraph"
    );

    for mode in [
        SessionMode::Ask,
        SessionMode::Plan,
        SessionMode::Code,
        SessionMode::Review,
    ] {
        assert_eq!(
            first_two_paragraphs(&system_prompt(mode, true, "/bin/zsh")),
            ask,
            "only the third paragraph may vary with mode; {mode:?} changed the prefix"
        );
    }

    for mode in [SessionMode::Plan, SessionMode::Code, SessionMode::Review] {
        assert!(
            system_prompt(mode, true, "/bin/zsh").len() > ask.len(),
            "{mode:?} keeps a mode-dependent third paragraph"
        );
    }
}
