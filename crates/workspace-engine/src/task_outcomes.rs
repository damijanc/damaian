//! Durable, content-safe terminal outcomes for a bounded worker task (spec 103
//! Task 6).
//!
//! Task 5 can decline an automatic segment for three reasons that are *not* a
//! generic round pause: the approved task budget is exhausted, the next request
//! cannot fit the configured window share, or the segment repeated without
//! progress. This module owns those typed facts, their engine-owned labels and
//! explanations, and the task status each one settles to.
//!
//! Every record is content-safe. It carries identifiers, numbers, typed
//! dimensions, token sources and hashes only — never a prompt, plan prose, tool
//! body, command text, path or finding. The durable session log remains the
//! authoritative transcript; this record says only why the task stopped and
//! what the engine knew when it did.
//!
//! Nothing here decides a continuation, calls a model, or writes a store. The
//! orchestrator builds the outcome and [`crate::SessionStore`] persists the
//! status and the outcome in one event.

use serde::{Deserialize, Serialize};

use crate::request_preparation::WithinTurnCompactionAttempt;
use crate::session::TaskStatus;
use crate::task_budget::{
    ABSOLUTE_TASK_ACTION_ROUND_CEILING, ABSOLUTE_TASK_EXPLORATION_ROUND_CEILING,
    TaskBudgetDimension, TaskExecutionBudget,
};
use crate::task_segments::{SegmentTokenTotals, StoredTaskSegment, TaskSegmentEvent};

/// The terminal-outcome record shape this build writes.
///
/// A later reader accepts the versions it knows and skips a newer one rather
/// than failing the append-only log.
pub const TASK_TERMINAL_OUTCOME_FORMAT_VERSION: u32 = 1;

/// Why a bounded task stopped.
///
/// The first three are the user-facing terminal outcomes requirement 10 names.
/// [`Self::UsageUnavailable`] is the abnormal failure path: durable usage could
/// not be proven, so the engine stopped instead of guessing that a measured
/// total had been spent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskTerminalOutcomeKind {
    /// The approved task execution budget is exhausted on one dimension.
    ApprovedBudgetExhausted,
    /// The pinned/fixed minimum cannot fit the configured request-window share.
    CompactionCannotFit,
    /// The engine-owned segment progress state repeated.
    NoProgress,
    /// A worker spent its pre-plan exploration budget and did not propose a
    /// plan when asked to (spec 103 R17).
    PrePlanExplorationExhausted,
    /// Durable usage is missing or unreadable, so no budget decision is safe.
    UsageUnavailable,
}

impl TaskTerminalOutcomeKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ApprovedBudgetExhausted => "approved_budget_exhausted",
            Self::CompactionCannotFit => "compaction_cannot_fit",
            Self::NoProgress => "no_progress",
            Self::PrePlanExplorationExhausted => "preplan_exploration_exhausted",
            Self::UsageUnavailable => "usage_unavailable",
        }
    }

    /// The terminal task status this kind settles to.
    ///
    /// Never [`TaskStatus::Complete`]. The legacy unbudgeted
    /// `ToolBudgetExhausted` and `TokenBudgetExhausted` statuses are
    /// deliberately not reused: these outcomes name an approved task bound, not
    /// an ordinary round or token ceiling.
    pub fn task_status(self) -> TaskStatus {
        match self {
            Self::ApprovedBudgetExhausted => TaskStatus::TaskBudgetExhausted,
            Self::CompactionCannotFit => TaskStatus::TaskCompactionCannotFit,
            Self::NoProgress => TaskStatus::TaskNoProgress,
            Self::PrePlanExplorationExhausted => TaskStatus::TaskPrePlanExplorationExhausted,
            // The abnormal path is an ordinary named failure, but it still
            // carries this typed outcome so a surface can say *why* it failed.
            Self::UsageUnavailable => TaskStatus::Failed,
        }
    }
}

/// The exact figures behind one approved-budget outcome.
///
/// `approved` and `used` are the bound and the durable count the engine
/// compared. `required` and `remaining` are present only when the stop is a
/// request-input refusal, where "the next request needed more than remained" is
/// the truthful statement rather than "the total was spent".
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskOutcomeFigures {
    pub approved: u64,
    pub used: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub required: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remaining: Option<u64>,
}

/// The content-safe facts of a request that could not fit the window share.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskCompactionFacts {
    pub context_window_tokens: u64,
    pub window_share_bps: u16,
    pub threshold_tokens: u64,
    pub request_tokens_before: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_tokens_after: Option<u64>,
    /// The deterministic minimum measured by the request path, when the
    /// refusal had one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_tokens_minimum: Option<u64>,
    pub fixed_overhead_tokens: u64,
    pub completed_rounds: u64,
    pub pinned_results: u64,
    pub eligible_results: u64,
    pub ineligible_results: u64,
    pub compacted_results: u64,
}

impl TaskCompactionFacts {
    pub fn from_attempt(attempt: &WithinTurnCompactionAttempt) -> Self {
        Self {
            context_window_tokens: attempt.context_window_tokens as u64,
            window_share_bps: attempt.window_share_bps,
            threshold_tokens: attempt.threshold_tokens as u64,
            request_tokens_before: attempt.request_tokens_before as u64,
            request_tokens_after: attempt.request_tokens_after.map(|value| value as u64),
            request_tokens_minimum: attempt.request_tokens_minimum.map(|value| value as u64),
            fixed_overhead_tokens: attempt.fixed_overhead_tokens as u64,
            completed_rounds: attempt.completed_rounds as u64,
            pinned_results: attempt.pinned_results as u64,
            eligible_results: attempt.eligible_results as u64,
            ineligible_results: attempt.ineligible_results as u64,
            compacted_results: attempt.compacted_results as u64,
        }
    }
}

/// The live state of the segment open when the task stopped.
///
/// A declined boundary leaves its segment open, but its durable
/// `segment_started` record carries zeros and no progress digest by
/// construction. The orchestrator therefore snapshots the engine's live
/// counters, task totals, durable token totals and current progress digest, so
/// this describes the segment the turn actually ran. All values are counts,
/// token totals and hashes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskSegmentSnapshot {
    pub segment: u32,
    pub segment_action_rounds: u32,
    pub segment_exploration_rounds: u32,
    pub total_action_rounds: u32,
    pub total_exploration_rounds: u32,
    pub tokens: SegmentTokenTotals,
    pub start_digest: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub progress_digest: Option<String>,
}

impl TaskSegmentSnapshot {
    /// The record as the durable log stores it.
    ///
    /// A `Started` payload is the boundary, not what the segment later did: it
    /// keeps its zero counters and no progress digest. Use this for reading
    /// historical records, never to describe a live terminal stop.
    pub fn from_stored(stored: &StoredTaskSegment) -> Self {
        match &stored.event {
            TaskSegmentEvent::Started(event) => Self {
                segment: event.segment,
                segment_action_rounds: event.segment_action_rounds,
                segment_exploration_rounds: event.segment_exploration_rounds,
                total_action_rounds: event.total_action_rounds,
                total_exploration_rounds: event.total_exploration_rounds,
                tokens: event.tokens,
                start_digest: event.start_digest.clone(),
                progress_digest: None,
            },
            TaskSegmentEvent::Finished(event) => Self {
                segment: event.segment,
                segment_action_rounds: event.segment_action_rounds,
                segment_exploration_rounds: event.segment_exploration_rounds,
                total_action_rounds: event.total_action_rounds,
                total_exploration_rounds: event.total_exploration_rounds,
                tokens: event.tokens,
                start_digest: event.start_digest.clone(),
                progress_digest: Some(event.progress_digest.clone()),
            },
        }
    }
}

/// One durable terminal outcome, stored beside the task status.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskTerminalOutcome {
    pub format_version: u32,
    pub session_id: String,
    pub task_id: String,
    pub kind: TaskTerminalOutcomeKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dimension: Option<TaskBudgetDimension>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub figures: Option<TaskOutcomeFigures>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_segment: Option<TaskSegmentSnapshot>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<SegmentTokenTotals>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compaction: Option<TaskCompactionFacts>,
}

impl TaskTerminalOutcome {
    pub fn new(
        session_id: impl Into<String>,
        task_id: impl Into<String>,
        kind: TaskTerminalOutcomeKind,
    ) -> Self {
        Self {
            format_version: TASK_TERMINAL_OUTCOME_FORMAT_VERSION,
            session_id: session_id.into(),
            task_id: task_id.into(),
            kind,
            dimension: None,
            figures: None,
            last_segment: None,
            usage: None,
            compaction: None,
        }
    }

    pub fn with_dimension(mut self, dimension: TaskBudgetDimension) -> Self {
        self.dimension = Some(dimension);
        self
    }

    pub fn with_dimension_opt(mut self, dimension: Option<TaskBudgetDimension>) -> Self {
        self.dimension = dimension;
        self
    }

    pub fn with_figures(mut self, figures: TaskOutcomeFigures) -> Self {
        self.figures = Some(figures);
        self
    }

    pub fn with_figures_opt(mut self, figures: Option<TaskOutcomeFigures>) -> Self {
        self.figures = figures;
        self
    }

    pub fn with_last_segment(mut self, last_segment: Option<TaskSegmentSnapshot>) -> Self {
        self.last_segment = last_segment;
        self
    }

    pub fn with_usage(mut self, usage: Option<SegmentTokenTotals>) -> Self {
        self.usage = usage;
        self
    }

    pub fn with_compaction(mut self, compaction: TaskCompactionFacts) -> Self {
        self.compaction = Some(compaction);
        self
    }

    pub fn task_status(&self) -> TaskStatus {
        self.kind.task_status()
    }

    /// The engine-owned headline a surface prints. Never a browser lookup.
    pub fn label(&self) -> String {
        match self.kind {
            TaskTerminalOutcomeKind::ApprovedBudgetExhausted => match self.dimension {
                Some(dimension) => format!(
                    "Approved task budget exhausted: {}",
                    dimension_label(dimension).to_lowercase()
                ),
                None => "Approved task budget exhausted".to_string(),
            },
            TaskTerminalOutcomeKind::CompactionCannotFit => "Request cannot fit".to_string(),
            TaskTerminalOutcomeKind::NoProgress => "Task made no progress".to_string(),
            TaskTerminalOutcomeKind::PrePlanExplorationExhausted => {
                "Pre-plan exploration exhausted".to_string()
            }
            TaskTerminalOutcomeKind::UsageUnavailable => "Task usage unavailable".to_string(),
        }
    }

    /// The engine-owned explanation a surface prints.
    ///
    /// It names the exact limiting dimension and figures when they exist, and
    /// never claims a measured total was exhausted for the abnormal
    /// usage-unavailable path.
    pub fn explanation(&self) -> String {
        match self.kind {
            TaskTerminalOutcomeKind::ApprovedBudgetExhausted => {
                let Some(dimension) = self.dimension else {
                    return "The task reached its approved execution budget.".to_string();
                };
                let Some(figures) = self.figures else {
                    return format!(
                        "The task reached its approved {} budget.",
                        dimension_label(dimension).to_lowercase()
                    );
                };
                match dimension {
                    TaskBudgetDimension::ActionRounds => format!(
                        "The task reached its approved action-round budget: {} of {} rounds used.",
                        figures.used, figures.approved
                    ),
                    TaskBudgetDimension::ExplorationRounds => format!(
                        "The task reached its approved exploration-round budget: {} of {} rounds used.",
                        figures.used, figures.approved
                    ),
                    TaskBudgetDimension::InputTokens => match (figures.required, figures.remaining) {
                        (Some(required), Some(remaining)) => format!(
                            "The next request could not fit the approved input-token budget: it needed {} tokens and {} of {} approved tokens remained.",
                            required, remaining, figures.approved
                        ),
                        _ => format!(
                            "The task reached its approved input-token budget: {} of {} tokens used.",
                            figures.used, figures.approved
                        ),
                    },
                    TaskBudgetDimension::OutputTokens => format!(
                        "The task reached its approved output-token budget: {} of {} tokens used.",
                        figures.used, figures.approved
                    ),
                }
            }
            TaskTerminalOutcomeKind::CompactionCannotFit => {
                let Some(facts) = self.compaction else {
                    return "The next request could not fit the configured request-window share."
                        .to_string();
                };
                let minimum = facts
                    .request_tokens_minimum
                    .unwrap_or(facts.request_tokens_before);
                format!(
                    "The next request could not fit the configured request-window share: the threshold is {} tokens and the smallest request the engine could prove was {} tokens. {} completed rounds were considered, with {} pinned results.",
                    facts.threshold_tokens,
                    minimum,
                    facts.completed_rounds,
                    facts.pinned_results
                )
            }
            TaskTerminalOutcomeKind::NoProgress => "The task stopped because its latest segment made no measurable progress: the same plan state and typed tool results were observed again.".to_string(),
            TaskTerminalOutcomeKind::PrePlanExplorationExhausted => "The worker spent its pre-plan exploration budget and did not propose a plan when asked to. It stopped instead of offering a generic Continue; restart it with a narrower task, or give it a plan-oriented prompt.".to_string(),
            TaskTerminalOutcomeKind::UsageUnavailable => "The task stopped because its durable token usage could not be proven, so the engine could not safely spend more of the approved budget.".to_string(),
        }
    }

    /// The engine-owned actions a surface may offer.
    ///
    /// These bounded stops deliberately offer none against the same approved
    /// bound: the budget is spent, the required state does not fit, or repeating
    /// the segment would make no progress. A worker surface may still offer its
    /// ordinary Open lifecycle action from [`crate::TaskControl`].
    pub fn available_actions(&self) -> Vec<String> {
        Vec::new()
    }

    /// A plain-text rendering for the CLI. The CLI prints this and maps nothing.
    pub fn render(&self) -> String {
        let mut text = format!("{}\n{}\n", self.label(), self.explanation());
        let actions = self.available_actions();
        if !actions.is_empty() {
            text.push_str("Available actions:\n");
            for action in actions {
                text.push_str(&format!("  {action}\n"));
            }
        }
        text
    }

    /// The stable, content-safe audit field set.
    pub fn audit_fields(&self) -> Vec<(&'static str, String)> {
        let mut fields = vec![
            ("actor", "system".to_string()),
            ("sessionId", self.session_id.clone()),
            ("taskId", self.task_id.clone()),
            ("formatVersion", self.format_version.to_string()),
            ("kind", self.kind.as_str().to_string()),
            (
                "dimension",
                self.dimension
                    .map(|dimension| dimension.as_str().to_string())
                    .unwrap_or_else(|| "none".to_string()),
            ),
        ];
        if let Some(figures) = self.figures {
            fields.push(("approved", figures.approved.to_string()));
            fields.push(("used", figures.used.to_string()));
            if let Some(required) = figures.required {
                fields.push(("required", required.to_string()));
            }
            if let Some(remaining) = figures.remaining {
                fields.push(("remaining", remaining.to_string()));
            }
        }
        if let Some(segment) = &self.last_segment {
            fields.push(("segment", segment.segment.to_string()));
            fields.push(("totalActionRounds", segment.total_action_rounds.to_string()));
            fields.push((
                "totalExplorationRounds",
                segment.total_exploration_rounds.to_string(),
            ));
        }
        if let Some(usage) = &self.usage {
            fields.push(("inputTokens", usage.input_tokens.to_string()));
            fields.push(("outputTokens", usage.output_tokens.to_string()));
            fields.push((
                "tokenSource",
                usage
                    .source
                    .map(token_source_label)
                    .unwrap_or("unavailable")
                    .to_string(),
            ));
        }
        if let Some(compaction) = &self.compaction {
            fields.push(("thresholdTokens", compaction.threshold_tokens.to_string()));
            fields.push((
                "requestTokensBefore",
                compaction.request_tokens_before.to_string(),
            ));
            if let Some(minimum) = compaction.request_tokens_minimum {
                fields.push(("requestTokensMinimum", minimum.to_string()));
            }
        }
        fields
    }
}

impl TaskBudgetDimension {
    /// The engine-owned display name for a budget dimension.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ActionRounds => "action_rounds",
            Self::ExplorationRounds => "exploration_rounds",
            Self::InputTokens => "input_tokens",
            Self::OutputTokens => "output_tokens",
        }
    }
}

/// The engine-owned display name for a budget dimension.
pub fn dimension_label(dimension: TaskBudgetDimension) -> &'static str {
    match dimension {
        TaskBudgetDimension::ActionRounds => "Action rounds",
        TaskBudgetDimension::ExplorationRounds => "Exploration rounds",
        TaskBudgetDimension::InputTokens => "Input tokens",
        TaskBudgetDimension::OutputTokens => "Output tokens",
    }
}

/// The exact inclusive cap the segment predicate uses for a round dimension.
pub fn approved_action_round_cap(budget: &TaskExecutionBudget) -> u64 {
    u64::from(budget.action_rounds.min(ABSOLUTE_TASK_ACTION_ROUND_CEILING))
}

/// The exact inclusive cap the segment predicate uses for exploration rounds.
pub fn approved_exploration_round_cap(budget: &TaskExecutionBudget) -> u64 {
    u64::from(
        budget
            .exploration_rounds
            .min(ABSOLUTE_TASK_EXPLORATION_ROUND_CEILING),
    )
}

fn token_source_label(source: crate::model::UsageSource) -> &'static str {
    match source {
        crate::model::UsageSource::Measured => "measured",
        crate::model::UsageSource::Estimated => "estimated",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn outcome(kind: TaskTerminalOutcomeKind) -> TaskTerminalOutcome {
        TaskTerminalOutcome::new("ses_1", "task_1", kind)
    }

    #[test]
    fn every_kind_has_a_distinct_non_success_status() {
        let kinds = [
            TaskTerminalOutcomeKind::ApprovedBudgetExhausted,
            TaskTerminalOutcomeKind::CompactionCannotFit,
            TaskTerminalOutcomeKind::NoProgress,
            TaskTerminalOutcomeKind::PrePlanExplorationExhausted,
            TaskTerminalOutcomeKind::UsageUnavailable,
        ];
        let mut statuses = Vec::new();
        for kind in kinds {
            let status = kind.task_status();
            assert!(status.is_terminal(), "{kind:?} must be terminal");
            assert!(!status.is_successful(), "{kind:?} must not be success");
            assert!(!status.is_active(), "{kind:?} has no turn running");
            assert!(
                !status.may_have_side_effect_in_flight(),
                "{kind:?} has nothing in flight"
            );
            statuses.push(status);
        }
        statuses.sort_by(|left, right| left.as_str().cmp(right.as_str()));
        statuses.dedup();
        assert_eq!(statuses.len(), 5);
    }

    #[test]
    fn an_approved_budget_outcome_names_its_dimension_and_figures() {
        let outcome = outcome(TaskTerminalOutcomeKind::ApprovedBudgetExhausted)
            .with_dimension(TaskBudgetDimension::ActionRounds)
            .with_figures(TaskOutcomeFigures {
                approved: 8,
                used: 8,
                required: None,
                remaining: Some(0),
            });
        assert!(outcome.label().contains("action rounds"));
        assert!(outcome.explanation().contains("8 of 8 rounds"));
    }

    #[test]
    fn usage_unavailable_never_claims_a_total_was_spent() {
        let text = outcome(TaskTerminalOutcomeKind::UsageUnavailable).explanation();
        assert!(text.contains("could not be proven"), "{text}");
        assert!(!text.contains("spent"), "{text}");
        assert!(!text.contains("reached its approved"), "{text}");
    }

    #[test]
    fn the_rendering_is_engine_owned_and_offers_no_continuation() {
        let text = outcome(TaskTerminalOutcomeKind::NoProgress).render();
        assert!(text.contains("Task made no progress"), "{text}");
        assert!(!text.to_lowercase().contains("continue"), "{text}");
        assert!(
            outcome(TaskTerminalOutcomeKind::NoProgress)
                .available_actions()
                .is_empty()
        );
    }
}
