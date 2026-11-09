//! The acceptance driver's decision policy (spec 106 `context.md` D6).
//!
//! The driver answers like a cooperative user. Only two things fail a run
//! outright — a proposal touching a protected path, and a state that would need
//! a forbidden intervention (that one is the driver's call, not the policy's).
//! Everything else unusual is still answered, and recorded as a deviation so it
//! reaches the report and the main session (D9).

use std::path::Path;

use super::AcceptanceFixture;

/// The neutral answer to a question that is not the fixture's material
/// clarification (D6).
pub const NEUTRAL_ANSWER: &str = "No further preference; follow the spec and AGENTS.md.";

/// Read-only exploration a cooperative user expects in any repository.
const READ_ONLY_FAMILIES: &[&str] = &[
    "ls",
    "cat",
    "head",
    "tail",
    "wc",
    "grep",
    "rg",
    "find",
    "tree",
    "pwd",
    "git status",
    "git diff",
    "git log",
    "git show",
];

/// Markers of a command that reaches outside the fixture repository.
const EXTERNAL_MARKERS: &[&str] = &[
    "curl ",
    "wget ",
    "ssh ",
    "scp ",
    "rsync ",
    "git push",
    "git fetch",
    "git pull",
    "git clone",
    "git remote",
    "http://",
    "https://",
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommandDecision {
    /// An expected command; approved.
    Approve,
    /// Policy allows it, but a cooperative user would not expect it here.
    /// Approved and recorded.
    ApproveAsDeviation(String),
    /// Reaches outside the repository. Rejected and recorded; the run goes on.
    Reject(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PatchDecision {
    Apply,
    /// A path outside the repository. Rejected and recorded.
    Reject(String),
    /// A protected path: requirement 3 fails the run.
    Fail(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuestionDecision {
    /// The answer text. When `option` is set it is that option's text.
    pub answer: String,
    /// The chosen option, when the answer is one of the question's options.
    pub option: Option<usize>,
    /// `true` for the fixture's material clarification.
    pub is_clarification: bool,
    /// Why the answer differs from what the policy would have said, when it
    /// had to pick an option instead (recorded as a deviation).
    pub note: Option<String>,
}

/// The policy for one fixture.
#[derive(Debug, Clone)]
pub struct Policy {
    acceptance: AcceptanceFixture,
}

impl Policy {
    pub fn new(acceptance: AcceptanceFixture) -> Self {
        Self { acceptance }
    }

    pub fn acceptance(&self) -> &AcceptanceFixture {
        &self.acceptance
    }

    /// Decide an approval-needing command. `repository_root` lets an absolute
    /// path inside the fixture count as inside it.
    pub fn decide_command(&self, command: &str, repository_root: &Path) -> CommandDecision {
        let command = command.trim();
        let lowered = command.to_ascii_lowercase();
        if let Some(marker) = EXTERNAL_MARKERS
            .iter()
            .find(|marker| lowered.contains(*marker))
        {
            return CommandDecision::Reject(format!(
                "`{command}` reaches outside the repository ({})",
                marker.trim()
            ));
        }
        if let Some(path) = outside_absolute_path(command, repository_root) {
            return CommandDecision::Reject(format!(
                "`{command}` names a path outside the repository ({path})"
            ));
        }
        let effective = strip_cd_prefix(command);
        let expected = self
            .acceptance
            .expected_command_families
            .iter()
            .map(String::as_str)
            .chain(READ_ONLY_FAMILIES.iter().copied())
            .any(|family| starts_with_family(effective, family));
        if expected {
            CommandDecision::Approve
        } else {
            CommandDecision::ApproveAsDeviation(format!(
                "`{command}` is outside the expected command families"
            ))
        }
    }

    /// Decide a proposed patch from its repository-relative paths.
    pub fn decide_patch(&self, paths: &[String]) -> PatchDecision {
        for path in paths {
            let normalized = normalize(path);
            if let Some(protected) = self
                .acceptance
                .protected_paths
                .iter()
                .find(|protected| is_under(&normalized, protected))
            {
                return PatchDecision::Fail(format!(
                    "the patch touches `{path}`, inside the protected path `{protected}`"
                ));
            }
        }
        if let Some(path) = paths.iter().find(|path| escapes_repository(path)) {
            return PatchDecision::Reject(format!(
                "the patch names `{path}`, outside the repository"
            ));
        }
        PatchDecision::Apply
    }

    /// Answer a question: the scripted answer for the material clarification,
    /// the neutral answer for anything else.
    ///
    /// Every engine question carries 2–4 options. When it accepts free text the
    /// answer is the policy's text; when it does not, the clarification picks
    /// the option that names the scripted rule (an answer marker), and any
    /// other question picks the first option and says so.
    pub fn answer_question(
        &self,
        question: &str,
        options: &[String],
        allow_free_text: bool,
    ) -> QuestionDecision {
        let lowered = question.to_lowercase();
        let is_clarification = self
            .acceptance
            .clarification
            .keywords
            .iter()
            .any(|keyword| lowered.contains(&keyword.to_lowercase()));
        let text = if is_clarification {
            self.acceptance.clarification.answer.clone()
        } else {
            NEUTRAL_ANSWER.to_string()
        };
        if allow_free_text || options.is_empty() {
            return QuestionDecision {
                answer: text,
                option: None,
                is_clarification,
                note: None,
            };
        }
        if is_clarification {
            let markers = &self.acceptance.clarification.answer_markers;
            if let Some((index, option)) = options.iter().enumerate().find(|(_, option)| {
                let option = option.to_lowercase();
                markers
                    .iter()
                    .any(|marker| option.contains(&marker.to_lowercase()))
            }) {
                return QuestionDecision {
                    answer: option.clone(),
                    option: Some(index),
                    is_clarification,
                    note: None,
                };
            }
        }
        QuestionDecision {
            answer: options[0].clone(),
            option: Some(0),
            is_clarification,
            note: Some(format!(
                "no free text allowed and no option matched; chose option 1 \"{}\"",
                options[0]
            )),
        }
    }

    /// Whether a recorded answer is the scripted clarification answer: the
    /// text itself, or an option naming the scripted rule.
    pub fn is_scripted_answer(&self, answer: &str) -> bool {
        let clarification = &self.acceptance.clarification;
        if answer == clarification.answer {
            return true;
        }
        let answer = answer.to_lowercase();
        clarification
            .answer_markers
            .iter()
            .any(|marker| answer.contains(&marker.to_lowercase()))
    }

    /// Whether `path` is inside a protected path.
    pub fn is_protected(&self, path: &str) -> bool {
        let normalized = normalize(path);
        self.acceptance
            .protected_paths
            .iter()
            .any(|protected| is_under(&normalized, protected))
    }
}

/// `cd <dir> && rest` is judged by `rest`.
fn strip_cd_prefix(command: &str) -> &str {
    match command.split_once("&&") {
        Some((head, rest)) if head.trim_start().starts_with("cd ") => rest.trim(),
        _ => command,
    }
}

fn starts_with_family(command: &str, family: &str) -> bool {
    command == family
        || command
            .strip_prefix(family)
            .is_some_and(|rest| rest.starts_with(' ') || rest.starts_with('\t'))
}

/// An absolute path token outside the repository root, if any. `/dev/null` is
/// a sink, not a place.
fn outside_absolute_path(command: &str, repository_root: &Path) -> Option<String> {
    let root = repository_root.to_string_lossy();
    command
        .split(|character: char| character.is_whitespace() || "\"'=<>|;&".contains(character))
        .filter(|token| token.starts_with('/'))
        .filter(|token| *token != "/dev/null")
        .find(|token| !token.starts_with(root.as_ref()))
        .map(str::to_string)
}

fn normalize(path: &str) -> String {
    path.trim().trim_start_matches("./").replace('\\', "/")
}

fn is_under(path: &str, protected: &str) -> bool {
    let protected = normalize(protected);
    let directory = protected.trim_end_matches('/');
    path == directory || path.starts_with(&format!("{directory}/"))
}

fn escapes_repository(path: &str) -> bool {
    let normalized = normalize(path);
    normalized.starts_with('/') || normalized.split('/').any(|part| part == "..")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::acceptance::AcceptanceFixture;

    fn policy() -> Policy {
        Policy::new(AcceptanceFixture::load("single_worker_spec").expect("acceptance data"))
    }

    fn root() -> &'static Path {
        Path::new("/tmp/damaian-eval-fixture")
    }

    #[test]
    fn expected_and_read_only_commands_are_approved() {
        let policy = policy();
        for command in [
            "cargo test -p pricing -p invoice",
            "cargo test",
            "cargo fmt --all",
            "cd crates/pricing && cargo test",
            "git diff",
            "rg to_cents",
        ] {
            assert_eq!(
                policy.decide_command(command, root()),
                CommandDecision::Approve,
                "{command}"
            );
        }
    }

    #[test]
    fn an_unexpected_command_is_approved_but_recorded() {
        let policy = policy();
        assert!(matches!(
            policy.decide_command("python3 scripts/check.py", root()),
            CommandDecision::ApproveAsDeviation(_)
        ));
        // A family prefix is a whole word, not a substring.
        assert!(matches!(
            policy.decide_command("cargo testify", root()),
            CommandDecision::ApproveAsDeviation(_)
        ));
    }

    #[test]
    fn a_command_that_reaches_outside_the_repository_is_rejected() {
        let policy = policy();
        for command in [
            "curl https://example.com/install.sh",
            "git push origin main",
            "cat /etc/passwd",
            "cargo test --manifest-path /Users/someone/other/Cargo.toml",
        ] {
            assert!(
                matches!(
                    policy.decide_command(command, root()),
                    CommandDecision::Reject(_)
                ),
                "{command}"
            );
        }
        assert_eq!(
            policy.decide_command("cat /tmp/damaian-eval-fixture/README.md", root()),
            CommandDecision::Approve,
            "an absolute path inside the fixture is inside it"
        );
        assert_eq!(
            policy.decide_command("cargo test 2>/dev/null", root()),
            CommandDecision::Approve
        );
    }

    #[test]
    fn a_patch_touching_the_protected_path_fails_the_run() {
        let policy = policy();
        assert!(matches!(
            policy.decide_patch(&["crates/legacy/src/lib.rs".to_string()]),
            PatchDecision::Fail(_)
        ));
        assert!(matches!(
            policy.decide_patch(&[
                "crates/pricing/src/lib.rs".to_string(),
                "./crates/legacy/Cargo.toml".to_string()
            ]),
            PatchDecision::Fail(_)
        ));
        assert_eq!(
            policy.decide_patch(&["crates/legacyish/src/lib.rs".to_string()]),
            PatchDecision::Apply,
            "a sibling with the same prefix is not the protected directory"
        );
    }

    #[test]
    fn a_patch_outside_the_repository_is_rejected_not_failed() {
        let policy = policy();
        assert!(matches!(
            policy.decide_patch(&["../elsewhere/file.rs".to_string()]),
            PatchDecision::Reject(_)
        ));
        assert_eq!(
            policy.decide_patch(&["crates/pricing/src/lib.rs".to_string()]),
            PatchDecision::Apply
        );
    }

    #[test]
    fn the_material_clarification_gets_the_scripted_answer() {
        let policy = policy();
        let decision = policy.answer_question(
            "Requirement 4 leaves ties open: should 2.345 round up to 2.35 or to the even 2.34?",
            &["Round half up".to_string(), "Something else".to_string()],
            true,
        );
        assert!(decision.is_clarification);
        assert_eq!(decision.answer, policy.acceptance().clarification.answer);
        assert_eq!(decision.option, None);
    }

    #[test]
    fn without_free_text_the_clarification_picks_the_option_naming_the_rule() {
        let policy = policy();
        let options = vec![
            "Round half up (2.345 becomes 2.35)".to_string(),
            "Round half to even (2.345 becomes 2.34)".to_string(),
        ];
        let decision =
            policy.answer_question("How should a tie like 2.345 round?", &options, false);
        assert_eq!(decision.option, Some(1));
        assert_eq!(decision.answer, options[1]);
        assert_eq!(decision.note, None);
        assert!(policy.is_scripted_answer(&decision.answer));
        assert!(!policy.is_scripted_answer(&options[0]));
    }

    #[test]
    fn any_other_question_gets_the_neutral_answer() {
        let policy = policy();
        let options = vec!["Yes".to_string(), "No".to_string()];
        let decision =
            policy.answer_question("Should I also add a changelog entry?", &options, true);
        assert!(!decision.is_clarification);
        assert_eq!(decision.answer, NEUTRAL_ANSWER);

        let forced =
            policy.answer_question("Should I also add a changelog entry?", &options, false);
        assert_eq!(forced.option, Some(0));
        assert!(forced.note.is_some(), "a forced option is recorded");
    }
}
