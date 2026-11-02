//! The durable worker-task execution budget (spec 103 Task 1).
//!
//! A budget is a finite, user-approved allowance for one worker task: how many
//! action rounds, exploration rounds, input tokens and output tokens it may
//! spend before it stops. It is deliberately separate from the ordinary
//! per-segment limits (`agent_max_tool_rounds`, `agent_max_exploration_rounds`):
//! those remain small checkpoints inside this total, so later tasks can start a
//! new segment without asking the user again while the total lasts.
//!
//! Nothing here decides whether a segment may continue, performs a model call,
//! or compacts a request. Task 1 only makes the typed facts and the policy that
//! proposes and validates them. Later slices consume them.

use crate::error::{ClientError, Result};
use serde::{Deserialize, Serialize};

/// Absolute product ceiling for the total action-round allowance.
///
/// Configuration may choose any value up to this, and a repository or profile
/// may only lower the effective value. Raising this constant is a product
/// decision rather than a configuration one.
pub const ABSOLUTE_TASK_ACTION_ROUND_CEILING: u32 = 256;

/// Absolute product ceiling for the total exploration-round allowance.
pub const ABSOLUTE_TASK_EXPLORATION_ROUND_CEILING: u32 = 384;

/// Absolute product ceiling for input tokens charged to one task.
pub const ABSOLUTE_TASK_INPUT_TOKEN_CEILING: u64 = 64_000_000;

/// Absolute product ceiling for output tokens charged to one task.
pub const ABSOLUTE_TASK_OUTPUT_TOKEN_CEILING: u64 = 4_000_000;

/// One worker task's approved execution allowance.
///
/// Every value is non-zero: a zero would describe a task that cannot start,
/// which is indistinguishable from Damaian being broken. The four dimensions
/// are independent — exhausting any one of them ends the task — so no field is
/// implied by another.
///
/// The record is deliberately only numbers. It never carries a prompt, a tool
/// result, a path or any other content; those stay in the session and audit
/// stores.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskExecutionBudget {
    pub action_rounds: u32,
    pub exploration_rounds: u32,
    pub input_tokens: u64,
    pub output_tokens: u64,
}

impl TaskExecutionBudget {
    /// Builds a budget from untrusted numeric input, rejecting zero and any
    /// value that does not fit its stored type.
    ///
    /// The round arguments are `u64` so a value that overflows `u32` is an
    /// explicit error rather than a silent truncation. Values above the
    /// absolute product ceilings are also refused; those ceilings are not
    /// configuration.
    pub fn try_new(
        action_rounds: u64,
        exploration_rounds: u64,
        input_tokens: u64,
        output_tokens: u64,
    ) -> Result<Self> {
        let action_rounds = u32::try_from(action_rounds).map_err(|_| {
            ClientError::InvalidInput(format!(
                "task budget action rounds must be at most {}",
                u32::MAX
            ))
        })?;
        let exploration_rounds = u32::try_from(exploration_rounds).map_err(|_| {
            ClientError::InvalidInput(format!(
                "task budget exploration rounds must be at most {}",
                u32::MAX
            ))
        })?;
        let budget = Self {
            action_rounds,
            exploration_rounds,
            input_tokens,
            output_tokens,
        };
        budget.validate()?;
        Ok(budget)
    }

    /// Rejects a budget that cannot describe a runnable task, or one above an
    /// absolute product ceiling.
    pub fn validate(&self) -> Result<()> {
        if self.action_rounds == 0 {
            return Err(ClientError::InvalidInput(
                "task budget action rounds must be at least 1".to_string(),
            ));
        }
        if self.exploration_rounds == 0 {
            return Err(ClientError::InvalidInput(
                "task budget exploration rounds must be at least 1".to_string(),
            ));
        }
        if self.input_tokens == 0 {
            return Err(ClientError::InvalidInput(
                "task budget input tokens must be at least 1".to_string(),
            ));
        }
        if self.output_tokens == 0 {
            return Err(ClientError::InvalidInput(
                "task budget output tokens must be at least 1".to_string(),
            ));
        }
        if self.action_rounds > ABSOLUTE_TASK_ACTION_ROUND_CEILING {
            return Err(ClientError::InvalidInput(format!(
                "task budget action rounds must be at most {ABSOLUTE_TASK_ACTION_ROUND_CEILING}"
            )));
        }
        if self.exploration_rounds > ABSOLUTE_TASK_EXPLORATION_ROUND_CEILING {
            return Err(ClientError::InvalidInput(format!(
                "task budget exploration rounds must be at most \
                 {ABSOLUTE_TASK_EXPLORATION_ROUND_CEILING}"
            )));
        }
        if self.input_tokens > ABSOLUTE_TASK_INPUT_TOKEN_CEILING {
            return Err(ClientError::InvalidInput(format!(
                "task budget input tokens must be at most {ABSOLUTE_TASK_INPUT_TOKEN_CEILING}"
            )));
        }
        if self.output_tokens > ABSOLUTE_TASK_OUTPUT_TOKEN_CEILING {
            return Err(ClientError::InvalidInput(format!(
                "task budget output tokens must be at most {ABSOLUTE_TASK_OUTPUT_TOKEN_CEILING}"
            )));
        }
        Ok(())
    }

    /// Validates an explicitly proposed budget against the effective ceiling.
    ///
    /// A value equal to the ceiling is accepted: the ceiling is a limit, not a
    /// value the proposal must stay below.
    pub fn validate_against(&self, ceiling: &Self) -> Result<()> {
        self.validate()?;
        if self.action_rounds > ceiling.action_rounds {
            return Err(ClientError::InvalidInput(format!(
                "task budget action rounds ({}) exceed the effective ceiling ({})",
                self.action_rounds, ceiling.action_rounds
            )));
        }
        if self.exploration_rounds > ceiling.exploration_rounds {
            return Err(ClientError::InvalidInput(format!(
                "task budget exploration rounds ({}) exceed the effective ceiling ({})",
                self.exploration_rounds, ceiling.exploration_rounds
            )));
        }
        if self.input_tokens > ceiling.input_tokens {
            return Err(ClientError::InvalidInput(format!(
                "task budget input tokens ({}) exceed the effective ceiling ({})",
                self.input_tokens, ceiling.input_tokens
            )));
        }
        if self.output_tokens > ceiling.output_tokens {
            return Err(ClientError::InvalidInput(format!(
                "task budget output tokens ({}) exceed the effective ceiling ({})",
                self.output_tokens, ceiling.output_tokens
            )));
        }
        Ok(())
    }

    /// The engine-owned sentence a budgeted plan approval appends to its user
    /// note.
    ///
    /// The values are the exact typed budget the user approved — never the
    /// current policy proposal, and never a ceiling. A budgetless approval never
    /// calls this, so inline and historical notes keep their pre-budget bytes.
    pub fn approval_note(&self) -> String {
        format!(
            "The user approved this task execution budget, which remains in force for this task: \
             action rounds = {}; exploration rounds = {}; input tokens = {}; output tokens = {}.",
            self.action_rounds, self.exploration_rounds, self.input_tokens, self.output_tokens
        )
    }

    /// The per-dimension minimum of this budget and `ceiling`.
    ///
    /// Used to turn configured "propose this much" values into what a plan
    /// review can actually offer after a lower ceiling applied. It never raises
    /// the proposal.
    pub fn clamped_to(&self, ceiling: &Self) -> Self {
        Self {
            action_rounds: self.action_rounds.min(ceiling.action_rounds),
            exploration_rounds: self.exploration_rounds.min(ceiling.exploration_rounds),
            input_tokens: self.input_tokens.min(ceiling.input_tokens),
            output_tokens: self.output_tokens.min(ceiling.output_tokens),
        }
    }
}

/// What a worker plan review may propose, and what it can never exceed.
///
/// [`Self::default`] is the effective proposal: the configured proposal after
/// the effective ceiling has been applied. [`Self::ceiling`] is the bound an
/// explicit budget is validated against. Both come from the same overlay merge
/// that every other config key uses, so a repository or profile may lower either
/// one and may never raise the user's or administrator's value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskBudgetPolicy {
    pub default: TaskExecutionBudget,
    pub ceiling: TaskExecutionBudget,
}

/// The display identity of one budget dimension.
///
/// A typed identity rather than a client-side lookup: it lets a client key a
/// row without parsing its label, and lets the engine rename a dimension
/// without a client guessing which string means action rounds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TaskBudgetDimension {
    ActionRounds,
    ExplorationRounds,
    InputTokens,
    OutputTokens,
}

/// One row of a task-budget review, as every client renders it.
///
/// `label` and `unit` are engine-owned because "rounds" and "tokens" are not
/// interchangeable and a client that chose one would be inventing policy. The
/// numeric value is carried unchanged beside them so a client never formats,
/// scales or derives it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskBudgetReviewRow {
    pub dimension: TaskBudgetDimension,
    pub label: String,
    pub value: u64,
    pub unit: String,
}

/// The budget a worker plan review offers, with everything a client renders.
///
/// The engine builds this from [`TaskBudgetPolicy`], so changing a default,
/// ceiling or label never requires a desktop, CLI or worker change. The exact
/// [`Self::budget`] a user reviewed is what comes back on approval; display
/// rows are presentation and are never re-derived from the returned budget.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskBudgetReview {
    pub budget: TaskExecutionBudget,
    pub title: String,
    pub note: String,
    pub rows: Vec<TaskBudgetReviewRow>,
}

impl TaskBudgetReview {
    /// The proposal for a plan review: the effective policy's default, with one
    /// display row per dimension.
    pub fn from_policy(policy: &TaskBudgetPolicy) -> Self {
        let budget = policy.default;
        Self {
            budget,
            title: "Task budget".to_string(),
            note: "This worker may run up to these totals before it stops. Reaching one \
                   ends the task; every patch, command, check and question still needs its \
                   own decision."
                .to_string(),
            rows: vec![
                TaskBudgetReviewRow {
                    dimension: TaskBudgetDimension::ActionRounds,
                    label: "Action rounds".to_string(),
                    value: u64::from(budget.action_rounds),
                    unit: "rounds".to_string(),
                },
                TaskBudgetReviewRow {
                    dimension: TaskBudgetDimension::ExplorationRounds,
                    label: "Exploration rounds".to_string(),
                    value: u64::from(budget.exploration_rounds),
                    unit: "rounds".to_string(),
                },
                TaskBudgetReviewRow {
                    dimension: TaskBudgetDimension::InputTokens,
                    label: "Input tokens".to_string(),
                    value: budget.input_tokens,
                    unit: "tokens".to_string(),
                },
                TaskBudgetReviewRow {
                    dimension: TaskBudgetDimension::OutputTokens,
                    label: "Output tokens".to_string(),
                    value: budget.output_tokens,
                    unit: "tokens".to_string(),
                },
            ],
        }
    }

    /// A plain-text rendering for a non-card client such as the CLI.
    ///
    /// The rows are printed exactly as the engine wrote them; the CLI makes no
    /// choice about labels, units or which dimensions exist.
    pub fn render(&self) -> String {
        let mut text = format!("{}\n", self.title);
        for row in &self.rows {
            text.push_str(&format!("  {}: {} {}\n", row.label, row.value, row.unit));
        }
        text.push_str(&format!("{}\n", self.note));
        text
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn budget(action: u32, exploration: u32, input: u64, output: u64) -> TaskExecutionBudget {
        TaskExecutionBudget {
            action_rounds: action,
            exploration_rounds: exploration,
            input_tokens: input,
            output_tokens: output,
        }
    }

    #[test]
    fn zero_and_overflow_are_rejected() {
        assert!(TaskExecutionBudget::try_new(0, 1, 1, 1).is_err());
        assert!(TaskExecutionBudget::try_new(1, 0, 1, 1).is_err());
        assert!(TaskExecutionBudget::try_new(1, 1, 0, 1).is_err());
        assert!(TaskExecutionBudget::try_new(1, 1, 1, 0).is_err());
        assert!(TaskExecutionBudget::try_new(u64::from(u32::MAX) + 1, 1, 1, 1).is_err());
        assert!(TaskExecutionBudget::try_new(1, u64::from(u32::MAX) + 1, 1, 1).is_err());
    }

    #[test]
    fn the_absolute_ceiling_is_inclusive() {
        let at_ceiling = budget(
            ABSOLUTE_TASK_ACTION_ROUND_CEILING,
            ABSOLUTE_TASK_EXPLORATION_ROUND_CEILING,
            ABSOLUTE_TASK_INPUT_TOKEN_CEILING,
            ABSOLUTE_TASK_OUTPUT_TOKEN_CEILING,
        );
        assert!(at_ceiling.validate().is_ok());

        for above in [
            budget(
                ABSOLUTE_TASK_ACTION_ROUND_CEILING + 1,
                ABSOLUTE_TASK_EXPLORATION_ROUND_CEILING,
                ABSOLUTE_TASK_INPUT_TOKEN_CEILING,
                ABSOLUTE_TASK_OUTPUT_TOKEN_CEILING,
            ),
            budget(
                ABSOLUTE_TASK_ACTION_ROUND_CEILING,
                ABSOLUTE_TASK_EXPLORATION_ROUND_CEILING + 1,
                ABSOLUTE_TASK_INPUT_TOKEN_CEILING,
                ABSOLUTE_TASK_OUTPUT_TOKEN_CEILING,
            ),
            budget(
                ABSOLUTE_TASK_ACTION_ROUND_CEILING,
                ABSOLUTE_TASK_EXPLORATION_ROUND_CEILING,
                ABSOLUTE_TASK_INPUT_TOKEN_CEILING + 1,
                ABSOLUTE_TASK_OUTPUT_TOKEN_CEILING,
            ),
            budget(
                ABSOLUTE_TASK_ACTION_ROUND_CEILING,
                ABSOLUTE_TASK_EXPLORATION_ROUND_CEILING,
                ABSOLUTE_TASK_INPUT_TOKEN_CEILING,
                ABSOLUTE_TASK_OUTPUT_TOKEN_CEILING + 1,
            ),
        ] {
            assert!(above.validate().is_err());
        }
    }

    #[test]
    fn validation_is_inclusive_of_the_ceiling_and_rejects_a_raise() {
        let ceiling = budget(32, 48, 1_000_000, 100_000);
        assert!(ceiling.validate_against(&ceiling).is_ok());
        assert!(
            budget(33, 48, 1_000_000, 100_000)
                .validate_against(&ceiling)
                .is_err()
        );
        assert!(
            budget(32, 49, 1_000_000, 100_000)
                .validate_against(&ceiling)
                .is_err()
        );
        assert!(
            budget(32, 48, 1_000_001, 100_000)
                .validate_against(&ceiling)
                .is_err()
        );
        assert!(
            budget(32, 48, 1_000_000, 100_001)
                .validate_against(&ceiling)
                .is_err()
        );
    }

    #[test]
    fn a_review_renders_exactly_the_policy_proposal() {
        let policy = TaskBudgetPolicy {
            default: budget(96, 144, 8_000_000, 512_000),
            ceiling: budget(256, 384, 64_000_000, 4_000_000),
        };
        let review = TaskBudgetReview::from_policy(&policy);
        assert_eq!(review.budget, policy.default);
        assert_eq!(review.rows.len(), 4);
        assert!(review.rows.iter().any(|row| {
            row.dimension == TaskBudgetDimension::ActionRounds
                && row.value == 96
                && row.unit == "rounds"
        }));
        assert!(review.render().contains("Action rounds: 96 rounds"));
    }

    #[test]
    fn the_approval_note_names_all_four_approved_values() {
        let approved = budget(96, 144, 8_000_000, 512_000);
        let note = approved.approval_note();
        for value in ["96", "144", "8000000", "512000"] {
            assert!(note.contains(value), "{note}");
        }
        assert!(note.contains("action rounds = 96"));
        assert!(note.contains("exploration rounds = 144"));
        assert!(note.contains("input tokens = 8000000"));
        assert!(note.contains("output tokens = 512000"));
    }

    #[test]
    fn clamping_never_raises_a_proposal() {
        let proposal = budget(64, 96, 4_000_000, 250_000);
        let ceiling = budget(32, 128, 8_000_000, 100_000);
        assert_eq!(
            proposal.clamped_to(&ceiling),
            budget(32, 96, 4_000_000, 100_000)
        );
    }
}
