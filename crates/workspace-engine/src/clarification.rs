//! Model-initiated clarification (spec 50): the one bounded question a model
//! may ask the user mid-turn, and the types that keep an answer from ever
//! standing in for an approval.
//!
//! The load-bearing rule is proposal §5.2. A waiting question is a
//! [`PendingQuestionRef`], a waiting approval is a [`PendingApprovalRef`], and
//! nothing converts one into the other. The `const` assertion at the bottom
//! of this file breaks the build if a conversion is ever added
//! (`context.md` §5).

use crate::hash::create_id;
use crate::session::PendingApprovalRef;
use serde::{Deserialize, Serialize};

/// Longest question, in characters. `context.md` §8.
pub const QUESTION_MAX_CHARS: usize = 300;
/// Fewest suggested answers. A question with no options is an essay prompt,
/// and the point of the tool is that answering takes one click (§5.1).
pub const OPTIONS_MIN: usize = 2;
/// Most suggested answers.
pub const OPTIONS_MAX: usize = 4;
/// Longest option label, in characters. Labels, not paragraphs.
pub const OPTION_MAX_CHARS: usize = 60;
/// Longest free-text answer, in characters.
pub const ANSWER_MAX_CHARS: usize = 2_000;

/// What the model supplies to `ask_user`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AskUserInput {
    /// One question. Rendered as untrusted text everywhere.
    pub question: String,
    /// 2–4 suggested answers.
    pub options: Vec<String>,
    /// Whether a free-text answer is accepted alongside the options.
    #[serde(default)]
    pub allow_free_text: bool,
}

impl AskUserInput {
    /// Checks every bound. A violation is an error naming the bound, never a
    /// truncation, so a model cannot get a wall of text into the dialog by
    /// exceeding a limit (§5.1).
    pub fn validate(&self) -> Result<(), String> {
        if self.question.trim().is_empty() {
            return Err("The question is empty.".to_string());
        }
        let length = self.question.chars().count();
        if length > QUESTION_MAX_CHARS {
            return Err(format!(
                "The question is {length} characters; the limit is {QUESTION_MAX_CHARS}."
            ));
        }
        let count = self.options.len();
        if !(OPTIONS_MIN..=OPTIONS_MAX).contains(&count) {
            return Err(format!(
                "The question has {count} options; it needs {OPTIONS_MIN} to {OPTIONS_MAX}."
            ));
        }
        for (index, option) in self.options.iter().enumerate() {
            if option.trim().is_empty() {
                return Err(format!("Option {index} is empty."));
            }
            let length = option.chars().count();
            if length > OPTION_MAX_CHARS {
                return Err(format!(
                    "Option {index} is {length} characters; the limit is {OPTION_MAX_CHARS}."
                ));
            }
            // A repeated label would make "which option was chosen" ambiguous.
            if self.options[..index].contains(option) {
                return Err(format!("Option {index} repeats an earlier option."));
            }
        }
        Ok(())
    }
}

/// Checks a user's answer against the question it answers. A chosen option
/// must name its own label. Free text needs `allow_free_text` and is bounded.
pub fn validate_answer(
    input: &AskUserInput,
    answer: &str,
    selected_option: Option<usize>,
) -> Result<(), String> {
    match selected_option {
        Some(index) => match input.options.get(index) {
            Some(label) if label == answer => Ok(()),
            Some(_) => Err(format!("The answer does not match option {index}.")),
            None => Err(format!("There is no option {index}.")),
        },
        None => {
            if !input.allow_free_text {
                return Err("This question accepts only one of its options.".to_string());
            }
            if answer.trim().is_empty() {
                return Err("The answer is empty.".to_string());
            }
            let length = answer.chars().count();
            if length > ANSWER_MAX_CHARS {
                return Err(format!(
                    "The answer is {length} characters; the limit is {ANSWER_MAX_CHARS}."
                ));
            }
            Ok(())
        }
    }
}

/// The link a task waiting on a question leaves on its status event.
///
/// It carries no proposal id, because there is no proposal, and there is no
/// conversion to or from [`PendingApprovalRef`]. The id is private, so the
/// only ways to get one are [`Self::new`] and replaying the session log.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingQuestionRef {
    question_id: String,
}

impl PendingQuestionRef {
    /// A new question id, `question_…`, which never has a proposal id's shape.
    #[allow(clippy::new_without_default)] // Minting an id is not a default value.
    pub fn new() -> Self {
        Self {
            question_id: create_id("question"),
        }
    }

    /// Rebuilds a link read back from the session log.
    pub(crate) fn recorded(question_id: String) -> Self {
        Self { question_id }
    }

    pub fn question_id(&self) -> &str {
        &self.question_id
    }
}

/// What a task in `WaitingForApproval` is waiting for. The state is shared
/// because both are "stopped, waiting for a human, nothing in flight"
/// (§5.2). The variant says which human decision it is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PendingDecision {
    Approval(PendingApprovalRef),
    Question(PendingQuestionRef),
}

/// A question as recorded in the session log, with its answer once there is
/// one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuestionState {
    pub question_id: String,
    pub task_id: String,
    pub question: String,
    pub options: Vec<String>,
    pub allow_free_text: bool,
    /// The answer and, when it was one of the options, which one.
    pub answer: Option<(String, Option<usize>)>,
}

impl QuestionState {
    /// The question as it was asked, for checking an answer against.
    pub fn input(&self) -> AskUserInput {
        AskUserInput {
            question: self.question.clone(),
            options: self.options.clone(),
            allow_free_text: self.allow_free_text,
        }
    }
}

// Proposal §5.2's compile-fail test. Nothing in this repository runs doctests
// (`context.md` §5), so this is a `const` assertion instead: the inherent
// `CONVERTS` shadows the trait's only when `T: Into<PendingApprovalRef>`
// holds, so adding `From<PendingQuestionRef> for PendingApprovalRef` (or any
// route to `Into`) fails the build here.
struct ConversionProbe<T>(core::marker::PhantomData<T>);
trait NoConversion {
    const CONVERTS: bool = false;
}
impl<T> NoConversion for ConversionProbe<T> {}
// Never used while the rule holds: the assertions resolve to the trait's
// `CONVERTS`, and this one exists only to take over if a conversion appears.
#[allow(dead_code)]
impl<T: Into<PendingApprovalRef>> ConversionProbe<T> {
    const CONVERTS: bool = true;
}
const _: () = assert!(
    !ConversionProbe::<PendingQuestionRef>::CONVERTS,
    "a question must never convert into an approval (spec 50 §5.2)"
);
const _: () = assert!(
    !ConversionProbe::<PendingDecision>::CONVERTS,
    "a pending decision must be matched, never converted into an approval (spec 50 §5.2)"
);
