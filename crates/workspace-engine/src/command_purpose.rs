//! What an agent command was for (spec 63 proposal §5.1).
//!
//! A purpose describes a command for the findings and plan evidence its run
//! produces. It is never a policy input: it is computed from a proposal only
//! after the command policy has classified it, and nothing in
//! `command_policy.rs` reads it (requirement 6). That is what lets the
//! exploration list be generous — a wrong answer here costs a finding, never
//! an approval.

use crate::command_policy::is_validation_command;
use crate::finding::default_parsers;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CommandPurpose {
    /// A validation: a detected project check, a command the policy knows as
    /// a validation, or one a finding parser can read. Its failure is a
    /// finding.
    Check,
    /// The agent looking around. Its failure says nothing about the user's
    /// code.
    Exploration,
    /// Anything else, including every command this module does not
    /// recognise. Treated as today, and the default for records written
    /// before purposes existed.
    #[default]
    Other,
}

impl CommandPurpose {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Check => "check",
            Self::Exploration => "exploration",
            Self::Other => "other",
        }
    }

    /// The inverse of [`Self::as_str`]. An unknown value reads as `Other`,
    /// which keeps today's treatment rather than failing a stored record.
    pub fn from_stored(value: &str) -> Self {
        match value {
            "check" => Self::Check,
            "exploration" => Self::Exploration,
            _ => Self::Other,
        }
    }
}

/// The purpose of `command`. `detected` is true for a proposal built by
/// `propose_detected_validations`, which is a check whatever its text.
pub fn command_purpose(command: &str, detected: bool) -> CommandPurpose {
    let command = command.trim();
    if detected
        || is_validation_command(command)
        || default_parsers()
            .iter()
            .any(|parser| parser.matches(command))
    {
        return CommandPurpose::Check;
    }
    if is_exploration(command) {
        CommandPurpose::Exploration
    } else {
        CommandPurpose::Other
    }
}

/// Programs that only look, matched on the first word of a simple command.
const EXPLORATION_PROGRAMS: &[&str] = &[
    "ls", "pwd", "cat", "head", "tail", "wc", "grep", "rg", "find", "awk", "sort", "uniq", "cut",
    "du", "stat", "file", "echo", "true",
];

/// `git` subcommands that only look. `branch` and `worktree` count only in
/// their listing forms.
const EXPLORATION_GIT_SUBCOMMANDS: &[&str] = &[
    "status",
    "diff",
    "log",
    "show",
    "rev-list",
    "rev-parse",
    "ls-files",
    "grep",
];

/// Every simple command is on the exploration list, ignoring a `cd <dir>`.
/// A command that is only `cd` is not exploration.
fn is_exploration(command: &str) -> bool {
    let mut looked = false;
    for segment in simple_commands(command) {
        let words: Vec<&str> = segment.split_whitespace().collect();
        if words.is_empty() {
            continue;
        }
        if words[0] == "cd" && words.len() <= 2 {
            continue;
        }
        if !is_exploration_command(&words) {
            return false;
        }
        looked = true;
    }
    looked
}

fn is_exploration_command(words: &[&str]) -> bool {
    match words[0] {
        "sed" => words.get(1) == Some(&"-n"),
        "git" => {
            // `git -C <dir> log` is how the agent reads another worktree.
            let rest = match words.get(1) {
                Some(&"-C") => words.get(3..).unwrap_or_default(),
                _ => &words[1..],
            };
            match rest {
                [subcommand, ..] if EXPLORATION_GIT_SUBCOMMANDS.contains(subcommand) => true,
                ["branch", "--list", ..] | ["worktree", "list", ..] => true,
                _ => false,
            }
        }
        program => EXPLORATION_PROGRAMS.contains(&program),
    }
}

/// Splits on `;`, `|`, `&` and newlines outside quotes, so `&&`, `||` and
/// pipes all separate. `2>&1` is a redirection, not a separator.
///
/// A lexical split for description only. It does not understand expansion
/// or subshells and must not be reused for a security decision; spec 64 has
/// its own tokenizer for that.
fn simple_commands(command: &str) -> Vec<String> {
    let mut segments = Vec::new();
    let mut current = String::new();
    let mut quote: Option<char> = None;
    let mut escaped = false;
    let mut previous = None;
    for character in command.chars() {
        if escaped {
            escaped = false;
            current.push(character);
        } else if character == '\\' && quote != Some('\'') {
            escaped = true;
            current.push(character);
        } else if let Some(open) = quote {
            if character == open {
                quote = None;
            }
            current.push(character);
        } else if character == '\'' || character == '"' {
            quote = Some(character);
            current.push(character);
        } else if matches!(character, ';' | '|' | '\n')
            || (character == '&' && !matches!(previous, Some('>' | '<')))
        {
            segments.push(std::mem::take(&mut current));
        } else {
            current.push(character);
        }
        previous = Some(character);
    }
    segments.push(current);
    segments
}

#[cfg(test)]
mod tests {
    use super::*;

    fn purpose(command: &str) -> CommandPurpose {
        command_purpose(command, false)
    }

    #[test]
    fn the_two_commands_from_the_motivating_session_are_exploration() {
        assert_eq!(
            purpose(
                "cd .claude/worktrees/spec25-task8 && grep -rn 'ContextManager::new' --include=*.rs ."
            ),
            CommandPurpose::Exploration
        );
        assert_eq!(
            purpose("ls -d target; du -sh target; ls -d ../../target && du -sh ../../target"),
            CommandPurpose::Exploration
        );
    }

    #[test]
    fn searches_listings_and_git_reads_are_exploration() {
        for command in [
            "grep -rn nothing-matches .",
            "rg -n 'a|b' src",
            "find . -name '*.rs' | head -20",
            "cat Cargo.toml | wc -l",
            "sed -n '1,40p' src/lib.rs",
            "ls -la 2>&1",
            "git status --short",
            "git log --oneline -5",
            "git -C .claude/worktrees/x diff --stat",
            "git branch --list",
            "git worktree list",
            "git ls-files | sort | uniq -c",
            "pwd",
            "echo done || true",
        ] {
            assert_eq!(purpose(command), CommandPurpose::Exploration, "{command}");
        }
    }

    #[test]
    fn checks_are_validations_and_parser_commands() {
        for command in [
            "cargo test",
            "cargo test -p workspace-engine",
            "cargo clippy --all-targets",
            "cargo build",
            "npm test",
            "npm run lint",
            "pytest -q",
            "npx biome check .",
        ] {
            assert_eq!(purpose(command), CommandPurpose::Check, "{command}");
        }
    }

    #[test]
    fn a_detected_validation_is_a_check_whatever_its_text() {
        assert_eq!(command_purpose("ls", true), CommandPurpose::Check);
        assert_eq!(
            command_purpose("python3 script.py", true),
            CommandPurpose::Check
        );
    }

    #[test]
    fn unknown_or_mixed_commands_are_other() {
        for command in [
            "python3 script.py",
            "",
            "cd src",
            "sed -i '' s/a/b/ file",
            "git branch -D old",
            "git worktree add ../x",
            "git commit -m wip",
            "ls && rm -rf build",
            "grep -rn x . | xargs rm",
            "FOO=1 grep x .",
            "./grep x",
        ] {
            assert_eq!(purpose(command), CommandPurpose::Other, "{command}");
        }
    }

    #[test]
    fn quoted_separators_do_not_split() {
        assert_eq!(
            simple_commands("grep -rn 'a|b; c && d' . | head"),
            vec!["grep -rn 'a|b; c && d' . ", " head"]
        );
        assert_eq!(
            simple_commands(r#"echo "x\"|y" ; pwd"#),
            vec![r#"echo "x\"|y" "#, " pwd"]
        );
    }

    #[test]
    fn stored_values_round_trip_and_unknown_reads_as_other() {
        for purpose in [
            CommandPurpose::Check,
            CommandPurpose::Exploration,
            CommandPurpose::Other,
        ] {
            assert_eq!(CommandPurpose::from_stored(purpose.as_str()), purpose);
        }
        assert_eq!(CommandPurpose::from_stored("later"), CommandPurpose::Other);
        assert_eq!(CommandPurpose::default(), CommandPurpose::Other);
    }
}
