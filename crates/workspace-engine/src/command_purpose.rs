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
/// `read` is here for `… | while read f; do …; done`: it only fills a shell
/// variable.
/// `which`, `type` and `whence` only say where a program is; finding none is
/// an answer, as for `ls`.
const EXPLORATION_PROGRAMS: &[&str] = &[
    "ls", "pwd", "cat", "head", "tail", "wc", "grep", "rg", "find", "awk", "sort", "uniq", "cut",
    "du", "stat", "file", "echo", "true", "read", "which", "type", "whence",
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

/// Every simple command is on the exploration list, ignoring a `cd <dir>`,
/// a `set` of shell options, and loop syntax. A loop is judged by the
/// commands it runs: `for`'s word list runs nothing, `while` and `until`
/// run their condition, and `do` and `done` are keywords. A command that
/// runs nothing on the list (only `cd`, only `set`) is not exploration.
///
/// Command substitution runs a command the split cannot see, so any `$(…)`
/// or backtick outside single quotes makes the whole command `Other`.
fn is_exploration(command: &str) -> bool {
    if has_command_substitution(command) {
        return false;
    }
    let mut looked = false;
    for segment in simple_commands(command) {
        let mut words: Vec<&str> = segment.split_whitespace().collect();
        match words.first() {
            None | Some(&"done") => continue,
            Some(&"for") => continue,
            Some(&"do" | &"while" | &"until") => {
                words.remove(0);
                if words.is_empty() {
                    continue;
                }
            }
            _ => {}
        }
        if words[0] == "cd" && words.len() <= 2 {
            continue;
        }
        if words[0] == "set" && is_option_set(&words[1..]) {
            continue;
        }
        if !is_exploration_command(&words) {
            return false;
        }
        looked = true;
    }
    looked
}

/// `set -e`, `set +e`, `set -euo pipefail`: options only, no positional
/// parameters. `--` starts positional parameters, so it does not count.
fn is_option_set(arguments: &[&str]) -> bool {
    let mut expects_name = false;
    for argument in arguments {
        if expects_name {
            expects_name = false;
            continue;
        }
        let is_flag = (argument.starts_with('-') || argument.starts_with('+'))
            && argument.len() > 1
            && *argument != "--";
        if !is_flag {
            return false;
        }
        expects_name = argument.ends_with('o');
    }
    !arguments.is_empty() && !expects_name
}

/// `$(` or a backtick outside single quotes, where the shell would run it.
fn has_command_substitution(command: &str) -> bool {
    let mut in_single = false;
    let mut escaped = false;
    let mut previous = None;
    for character in command.chars() {
        if escaped {
            escaped = false;
        } else if in_single {
            in_single = character != '\'';
        } else if character == '\\' {
            escaped = true;
        } else if character == '\'' {
            in_single = true;
        } else if character == '`' || (character == '(' && previous == Some('$')) {
            return true;
        }
        previous = Some(character);
    }
    false
}

fn is_exploration_command(words: &[&str]) -> bool {
    match words[0] {
        // Any form that prints rather than editing in place. A script word is
        // quoted or starts with something other than `-`, so only option words
        // are checked for `i`.
        "sed" => !words[1..].iter().any(|word| {
            *word == "--in-place"
                || word.starts_with("--in-place=")
                || (word.starts_with('-') && !word.starts_with("--") && word.contains('i'))
        }),
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
        // Only the lookup forms: plain `command npm install` runs `npm`.
        "command" => matches!(words.get(1), Some(&"-v" | &"-V")),
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
    fn looking_up_where_a_program_is_is_exploration() {
        // From a session on 2026-10-29: `npm` was not on the shell's PATH, and
        // the probe that looked for it exited 1 and blocked the step.
        for command in [
            "command -v node; command -v npm; ls /opt/homebrew/bin/node 2>/dev/null",
            "command -V npm",
            "which node",
            "type -a npm",
            "whence -p node",
        ] {
            assert_eq!(purpose(command), CommandPurpose::Exploration, "{command}");
        }
        // `command` without a lookup flag runs its argument.
        for command in ["command npm install", "command -p rm -rf build", "command"] {
            assert_eq!(purpose(command), CommandPurpose::Other, "{command}");
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
    fn the_loops_from_the_findings_panel_are_exploration() {
        assert_eq!(
            purpose(
                r#"for w in spec25-task9 spec50-task4; do echo "=== $w"; sed -n '1,6p' ".claude/worktrees/$w/docs/specs/$w"*/proposal.md 2>/dev/null; done"#
            ),
            CommandPurpose::Exploration
        );
        assert_eq!(
            purpose(
                "set +e; for f in docs/specs/25_x/proposal.md docs/specs/32_hooks/proposal.md; do echo \"== $f\"; sed -n '3,8p' \"$f\"; done"
            ),
            CommandPurpose::Exploration
        );
    }

    #[test]
    fn a_loop_is_judged_by_the_commands_it_runs() {
        for command in [
            "for f in a b; do head -5 \"$f\"; done",
            "for f in a b\ndo\n  wc -l \"$f\"\ndone | sort",
            "while true; do ls; done",
            "until git diff --quiet; do git status; done",
            "find . -name '*.md' | while read f; do head -1 \"$f\"; done",
            "set -e; ls",
            "set -euo pipefail; git log --oneline -3",
            "set +e\ngrep -rn x .",
        ] {
            assert_eq!(purpose(command), CommandPurpose::Exploration, "{command}");
        }
        for command in [
            "for f in *.rs; do rustfmt \"$f\"; done",
            "for f in a b; do rm \"$f\"; done",
            "while read l; do rm \"$l\"; done",
            "for f in a; do head \"$f\"; done; touch x",
            "set -e; rm -rf build",
            "set +e echo '== 59'; grep -rn x .",
            "set -- a b; ls",
            "for f in a; do done",
            "set -e",
        ] {
            assert_eq!(purpose(command), CommandPurpose::Other, "{command}");
        }
    }

    #[test]
    fn a_loop_that_runs_a_check_is_a_check() {
        assert_eq!(
            purpose("for t in a b; do cargo test \"$t\"; done"),
            CommandPurpose::Check
        );
    }

    #[test]
    fn command_substitution_is_other_wherever_it_appears() {
        for command in [
            "for f in $(git ls-files); do head -1 \"$f\"; done",
            "for f in `ls`; do head -1 \"$f\"; done",
            "echo $(rm x)",
            "echo \"$(touch y)\"",
        ] {
            assert_eq!(purpose(command), CommandPurpose::Other, "{command}");
        }
        // Single quotes make it literal text, so a search for it is still a search.
        assert_eq!(purpose("grep -rn '$(foo)' ."), CommandPurpose::Exploration);
    }

    #[test]
    fn sed_is_exploration_unless_it_edits_in_place() {
        for command in [
            "sed -e 's/a/b/' src/lib.rs | head",
            "sed -E -n '/x/p' f",
            "sed 's/a/b/' f",
        ] {
            assert_eq!(purpose(command), CommandPurpose::Exploration, "{command}");
        }
        for command in [
            "sed -i 's/a/b/' f",
            "sed -e 's/a/b/' -i f",
            "sed -ni '/x/p' f",
            "sed --in-place 's/a/b/' f",
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
