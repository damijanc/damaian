//! Automatic bounded worker segments (spec 103 Task 5).
//!
//! A worker whose plan carries an approved [`TaskExecutionBudget`] may cross an
//! ordinary per-segment checkpoint without asking for a generic *Continue*.
//! This module owns the typed facts and the deterministic decisions:
//!
//! * [`TaskSegmentStarted`] and [`TaskSegmentFinished`] are the durable,
//!   content-safe records of one segment boundary. They hold only counts, token
//!   totals, a token source and a progress digest — never a prompt, plan prose,
//!   tool body, command text, path or raw finding.
//! * [`decide_segment_continuation`] is the pure predicate. It refuses another
//!   segment when any approved total would be exhausted, when durable usage is
//!   missing, or when the progress digest did not advance.
//! * [`progress_digest`] hashes authoritative engine state only. It excludes
//!   timestamps, token spend, transient call ids and model prose.
//!
//! The chat orchestrator supplies the state and performs the request
//! preparation; nothing here reads a session store, calls a model or decides an
//! approval. Repository configuration cannot reach this module's policy.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use crate::hash::sha256_hex;
use crate::model::{ModelRequest, UsageSource, model_request_json};
use crate::plan::{Evidence, StepStatus, TaskPlan};
use crate::secret_scanner::SecretScanner;
use crate::session::TaskUsage;
use crate::task_budget::{
    ABSOLUTE_TASK_ACTION_ROUND_CEILING, ABSOLUTE_TASK_EXPLORATION_ROUND_CEILING,
    TaskExecutionBudget,
};
use crate::within_turn_compaction::CompletedToolRound;

/// The segment record shape this build writes.
///
/// A later reader accepts the versions it knows and skips a newer one rather
/// than failing the append-only log.
pub const TASK_SEGMENT_FORMAT_VERSION: u32 = 1;

/// The task-wide token totals and their provenance at a segment boundary.
///
/// `source` is `None` only when no durable `task_usage_recorded` event exists
/// for the task. That is deliberately distinct from zero: the continuation
/// predicate treats missing usage as unprovable and declines, because assuming
/// a task had spent nothing would widen the authority the user approved.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SegmentTokenTotals {
    pub input_tokens: u64,
    pub output_tokens: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<UsageSource>,
}

impl SegmentTokenTotals {
    /// Totals from the durable task usage, or an explicit "unavailable" record.
    ///
    /// Cached input tokens are deliberately not carried: they are a subset of
    /// `input_tokens`, never extra allowance (spec 103 Task 5).
    pub fn from_usage(usage: Option<&TaskUsage>) -> Self {
        match usage {
            Some(usage) => Self {
                input_tokens: usage.input_tokens,
                output_tokens: usage.output_tokens,
                source: Some(usage.source),
            },
            None => Self {
                input_tokens: 0,
                output_tokens: 0,
                source: None,
            },
        }
    }

    pub fn is_available(&self) -> bool {
        self.source.is_some()
    }
}

/// One `segment_started` payload.
///
/// `total_action_rounds` and `total_exploration_rounds` are the task-wide
/// counts *before* this segment, so `total + segment` is the count the segment
/// reaches. `segment_action_rounds` is zero here by construction; the finished
/// record carries the segment's own counts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskSegmentStarted {
    pub format_version: u32,
    pub session_id: String,
    pub task_id: String,
    pub segment: u32,
    pub segment_action_rounds: u32,
    pub segment_exploration_rounds: u32,
    pub total_action_rounds: u32,
    pub total_exploration_rounds: u32,
    pub tokens: SegmentTokenTotals,
    pub start_digest: String,
}

impl TaskSegmentStarted {
    /// The stable, content-safe audit field set.
    pub fn audit_fields(&self) -> Vec<(&'static str, String)> {
        vec![
            ("actor", "system".to_string()),
            ("sessionId", self.session_id.clone()),
            ("taskId", self.task_id.clone()),
            ("formatVersion", self.format_version.to_string()),
            ("segment", self.segment.to_string()),
            (
                "segmentActionRounds",
                self.segment_action_rounds.to_string(),
            ),
            (
                "segmentExplorationRounds",
                self.segment_exploration_rounds.to_string(),
            ),
            ("totalActionRounds", self.total_action_rounds.to_string()),
            (
                "totalExplorationRounds",
                self.total_exploration_rounds.to_string(),
            ),
            ("inputTokens", self.tokens.input_tokens.to_string()),
            ("outputTokens", self.tokens.output_tokens.to_string()),
            ("tokenSource", token_source_label(&self.tokens).to_string()),
            ("startDigest", self.start_digest.clone()),
        ]
    }
}

/// One `segment_finished` payload.
///
/// Written only when the boundary actually commits an automatic continuation.
/// A declined continuation leaves the segment open, so a later pause and resume
/// return to the same segment and totals rather than minting a second one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskSegmentFinished {
    pub format_version: u32,
    pub session_id: String,
    pub task_id: String,
    pub segment: u32,
    pub segment_action_rounds: u32,
    pub segment_exploration_rounds: u32,
    pub total_action_rounds: u32,
    pub total_exploration_rounds: u32,
    pub tokens: SegmentTokenTotals,
    pub start_digest: String,
    pub progress_digest: String,
}

impl TaskSegmentFinished {
    pub fn audit_fields(&self) -> Vec<(&'static str, String)> {
        vec![
            ("actor", "system".to_string()),
            ("sessionId", self.session_id.clone()),
            ("taskId", self.task_id.clone()),
            ("formatVersion", self.format_version.to_string()),
            ("segment", self.segment.to_string()),
            (
                "segmentActionRounds",
                self.segment_action_rounds.to_string(),
            ),
            (
                "segmentExplorationRounds",
                self.segment_exploration_rounds.to_string(),
            ),
            ("totalActionRounds", self.total_action_rounds.to_string()),
            (
                "totalExplorationRounds",
                self.total_exploration_rounds.to_string(),
            ),
            ("inputTokens", self.tokens.input_tokens.to_string()),
            ("outputTokens", self.tokens.output_tokens.to_string()),
            ("tokenSource", token_source_label(&self.tokens).to_string()),
            ("startDigest", self.start_digest.clone()),
            ("progressDigest", self.progress_digest.clone()),
        ]
    }
}

fn token_source_label(tokens: &SegmentTokenTotals) -> &'static str {
    match tokens.source {
        Some(UsageSource::Measured) => "measured",
        Some(UsageSource::Estimated) => "estimated",
        None => "unavailable",
    }
}

/// One typed segment event as a fresh store read returns it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TaskSegmentEvent {
    Started(TaskSegmentStarted),
    Finished(TaskSegmentFinished),
}

impl TaskSegmentEvent {
    pub fn task_id(&self) -> &str {
        match self {
            Self::Started(event) => &event.task_id,
            Self::Finished(event) => &event.task_id,
        }
    }

    pub fn format_version(&self) -> u32 {
        match self {
            Self::Started(event) => event.format_version,
            Self::Finished(event) => event.format_version,
        }
    }

    pub fn segment(&self) -> u32 {
        match self {
            Self::Started(event) => event.segment,
            Self::Finished(event) => event.segment,
        }
    }
}

/// A segment event with the sequence number its session-log line carried.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredTaskSegment {
    pub seq: u64,
    pub event: TaskSegmentEvent,
}

impl StoredTaskSegment {
    pub fn is_started(&self) -> bool {
        matches!(self.event, TaskSegmentEvent::Started(_))
    }

    pub fn is_finished(&self) -> bool {
        matches!(self.event, TaskSegmentEvent::Finished(_))
    }

    /// The task-wide totals this record leaves behind, whichever kind it is.
    pub fn totals(&self) -> (u32, u32) {
        match &self.event {
            TaskSegmentEvent::Started(event) => {
                (event.total_action_rounds, event.total_exploration_rounds)
            }
            TaskSegmentEvent::Finished(event) => {
                (event.total_action_rounds, event.total_exploration_rounds)
            }
        }
    }

    pub fn segment(&self) -> u32 {
        self.event.segment()
    }
}

/// The task-total counts a boundary decision sees.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SegmentTotals {
    pub action_rounds: u32,
    pub exploration_rounds: u32,
}

/// Why an automatic continuation was declined.
///
/// Task 5 uses this only to keep the existing pause rather than invent a
/// success. Task 6 turns these reasons into engine-owned terminal outcomes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SegmentDeclineReason {
    ActionRoundsExhausted,
    ExplorationRoundsExhausted,
    InputTokensExhausted,
    OutputTokensExhausted,
    UsageUnavailable,
    NoProgress,
}

impl SegmentDeclineReason {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ActionRoundsExhausted => "action_rounds_exhausted",
            Self::ExplorationRoundsExhausted => "exploration_rounds_exhausted",
            Self::InputTokensExhausted => "input_tokens_exhausted",
            Self::OutputTokensExhausted => "output_tokens_exhausted",
            Self::UsageUnavailable => "usage_unavailable",
            Self::NoProgress => "no_progress",
        }
    }
}

/// What one boundary decision allows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SegmentContinuationDecision {
    Continue {
        /// The next segment's action-round allowance, already capped by the
        /// remaining approved total and the ordinary segment limit.
        action_rounds: u32,
        exploration_rounds: u32,
    },
    Declined(SegmentDeclineReason),
}

/// Everything the pure continuation predicate needs.
///
/// Every value is supplied by the engine; nothing is inferred from prose or a
/// UI label. Task totals are bounded by the approved budget and by Task 1's
/// absolute *task* ceilings, never by the legacy one-turn/extension caps: those
/// bound one ordinary turn and must not decide how a multi-segment task ends.
#[derive(Debug, Clone, Copy)]
pub struct SegmentContinuationInput<'a> {
    pub budget: &'a TaskExecutionBudget,
    pub totals: SegmentTotals,
    pub usage: Option<&'a TaskUsage>,
    pub normal_action_rounds: u32,
    pub normal_exploration_rounds: u32,
    pub progress_advanced: bool,
}

/// The engine-owned continuation predicate.
///
/// The four approved dimensions are independent: exhausting any one, or being
/// unable to prove token usage, declines. The next segment's allowance is the
/// remaining total capped by one ordinary segment, so an approved total smaller
/// than a normal segment is respected exactly. Cached input tokens are not an
/// allowance and are not consulted.
pub fn decide_segment_continuation(
    input: &SegmentContinuationInput<'_>,
) -> SegmentContinuationDecision {
    let action_cap = input
        .budget
        .action_rounds
        .min(ABSOLUTE_TASK_ACTION_ROUND_CEILING);
    let exploration_cap = input
        .budget
        .exploration_rounds
        .min(ABSOLUTE_TASK_EXPLORATION_ROUND_CEILING);

    if input.totals.action_rounds >= action_cap {
        return SegmentContinuationDecision::Declined(SegmentDeclineReason::ActionRoundsExhausted);
    }
    if input.totals.exploration_rounds >= exploration_cap {
        return SegmentContinuationDecision::Declined(
            SegmentDeclineReason::ExplorationRoundsExhausted,
        );
    }
    let Some(usage) = input.usage else {
        return SegmentContinuationDecision::Declined(SegmentDeclineReason::UsageUnavailable);
    };
    if usage.input_tokens >= input.budget.input_tokens {
        return SegmentContinuationDecision::Declined(SegmentDeclineReason::InputTokensExhausted);
    }
    if usage.output_tokens >= input.budget.output_tokens {
        return SegmentContinuationDecision::Declined(SegmentDeclineReason::OutputTokensExhausted);
    }
    if !input.progress_advanced {
        return SegmentContinuationDecision::Declined(SegmentDeclineReason::NoProgress);
    }

    SegmentContinuationDecision::Continue {
        action_rounds: input
            .normal_action_rounds
            .min(action_cap - input.totals.action_rounds),
        exploration_rounds: input
            .normal_exploration_rounds
            .min(exploration_cap - input.totals.exploration_rounds),
    }
}

/// Why the exact next request may not be sent under the approved task budget.
///
/// This is deliberately separate from [`decide_segment_continuation`]: the
/// predicate proves rounds remain, while this proves the *exact prepared
/// request* fits what remains. Both are required before a boundary commits,
/// because a request can overspend an approved total even when the round
/// counters still have room.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TaskRequestTokenAuthority {
    /// No durable usage record exists, so the remaining allowance is unknown.
    UsageUnavailable,
    /// The prepared request's estimated input charge exceeds what remains.
    InputExceeded {
        required_input_tokens: u64,
        remaining_input_tokens: u64,
    },
    /// The approved output total is spent, or the configured output allowance
    /// is zero.
    OutputExhausted { remaining_output_tokens: u64 },
}

impl TaskRequestTokenAuthority {
    /// The typed limiting dimension, for the durable audit fact.
    pub fn dimension(&self) -> &'static str {
        match self {
            Self::UsageUnavailable => "usage",
            Self::InputExceeded { .. } => "input_tokens",
            Self::OutputExhausted { .. } => "output_tokens",
        }
    }
}

/// Computes the approved output cap for the next request *before* it is built.
///
/// Task 4 measures the complete request and decides whether it fits. Applying
/// `max_tokens` afterwards would make that decision describe a different shape
/// from the one the adapter sends: the field itself is request bytes. The
/// caller therefore puts this value into `ModelRequest::max_tokens` first, and
/// only then invokes request preparation.
///
/// `configured` is [`crate::Config::max_output_tokens`]. The result is the
/// smaller of that configured ceiling (when one exists) and the remaining
/// approved output total. A missing usage record or an exhausted output total
/// fails closed rather than guessing an allowance.
pub fn task_request_output_cap(
    budget: &TaskExecutionBudget,
    usage: Option<&TaskUsage>,
    configured: Option<u32>,
) -> Result<u32, TaskRequestTokenAuthority> {
    let Some(usage) = usage else {
        return Err(TaskRequestTokenAuthority::UsageUnavailable);
    };

    let remaining_output = budget.output_tokens.saturating_sub(usage.output_tokens);
    if remaining_output == 0 {
        return Err(TaskRequestTokenAuthority::OutputExhausted {
            remaining_output_tokens: 0,
        });
    }
    let configured_output = configured.map(u64::from).unwrap_or(remaining_output);
    // The product ceilings keep every budget value in `u32`, so this conversion
    // cannot truncate in practice; falling back to `u32::MAX` is the safe
    // direction if a future ceiling grows.
    let capped_output = u32::try_from(configured_output.min(remaining_output)).unwrap_or(u32::MAX);
    if capped_output == 0 {
        return Err(TaskRequestTokenAuthority::OutputExhausted {
            remaining_output_tokens: remaining_output,
        });
    }
    Ok(capped_output)
}

/// Proves that the already-prepared exact request fits the remaining approved
/// input total.
///
/// This is a measurement, not a mutation: `max_tokens` was applied before
/// request preparation, so the request measured here is byte-for-byte the one
/// Task 4 prepared and the adapter will send. A missing usage record still
/// fails closed, and an exhausted output total is refused even though the cap
/// was computed earlier.
pub fn authorize_task_request(
    request: &ModelRequest,
    budget: &TaskExecutionBudget,
    usage: Option<&TaskUsage>,
) -> Result<(), TaskRequestTokenAuthority> {
    let Some(usage) = usage else {
        return Err(TaskRequestTokenAuthority::UsageUnavailable);
    };
    if usage.output_tokens >= budget.output_tokens {
        return Err(TaskRequestTokenAuthority::OutputExhausted {
            remaining_output_tokens: 0,
        });
    }

    let required_input_tokens = model_request_json(request).len().div_ceil(4) as u64;
    let remaining_input_tokens = budget.input_tokens.saturating_sub(usage.input_tokens);
    if required_input_tokens > remaining_input_tokens {
        return Err(TaskRequestTokenAuthority::InputExceeded {
            required_input_tokens,
            remaining_input_tokens,
        });
    }

    Ok(())
}

/// The persisted half of a segment, carried in a paused turn.
///
/// `total_*` counts include the segment's own counts, so a resume can restore
/// the current segment without re-deriving it from a partial log. The record is
/// content-safe: numbers, a segment number and a digest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskSegmentPauseState {
    pub segment: u32,
    pub segment_action_rounds: u32,
    pub segment_exploration_rounds: u32,
    pub total_action_rounds: u32,
    pub total_exploration_rounds: u32,
    pub start_digest: String,
}

/// Authoritative engine state the progress digest may read.
///
/// `step_evidence` is the evidence accrued for the currently open step but not
/// yet attached, because attaching is the true turn end's job. It is
/// authoritative engine state: a check that ran is advancement even before the
/// plan record is updated.
pub struct SegmentProgressInput<'a> {
    pub plan: Option<&'a TaskPlan>,
    pub step_evidence: &'a [Evidence],
    pub completed_rounds: &'a [CompletedToolRound],
    pub notes_text: &'a str,
    pub notes_carried: bool,
    pub scanner: &'a SecretScanner,
}

/// The deterministic, content-safe progress digest for one segment boundary.
///
/// The predicate counts only real advancement:
///
/// * plan step identity, status and evidence (timestamps and titles excluded);
/// * the set of distinct typed tool identities, outcomes and ranges, with the
///   identity redacted before hashing — repeating the same typed call/outcome
///   under a fresh call id does not add an entry;
/// * evidence accrued for the open step;
/// * the bounded turn-note state (text hash, length and carried flag).
///
/// It deliberately excludes token spend, timestamps, message text, transient
/// generated ids and tool result bodies. The returned value is a hash, so no
/// path, command, note or finding is stored or exposed.
pub fn progress_digest(input: &SegmentProgressInput<'_>) -> String {
    let mut canonical = String::from("task-segment-progress/v1\n");

    match input.plan {
        Some(plan) => {
            canonical.push_str("plan\n");
            let mut steps: Vec<_> = plan.steps.iter().collect();
            steps.sort_by(|left, right| left.id.cmp(&right.id));
            for step in steps {
                canonical.push_str("step\t");
                canonical.push_str(&step.id);
                canonical.push('\t');
                canonical.push_str(step_status_label(step.status));
                canonical.push('\t');
                let mut facts = BTreeSet::new();
                for evidence in &step.evidence {
                    facts.insert(evidence_fact(input.scanner, evidence));
                }
                for fact in facts {
                    canonical.push_str(&fact);
                    canonical.push('\t');
                }
                canonical.push('\n');
            }
        }
        None => canonical.push_str("plan:none\n"),
    }

    canonical.push_str("open-evidence\n");
    // Deduplicated: repeating the same typed observation is not advancement.
    let mut open_facts = BTreeSet::new();
    for evidence in input.step_evidence {
        open_facts.insert(evidence_fact(input.scanner, evidence));
    }
    for fact in open_facts {
        canonical.push_str(&fact);
        canonical.push('\n');
    }

    // Deduplicated and sorted: two calls with the same typed identity, range and
    // outcome are the same advancement, regardless of their generated call ids.
    let mut tool_facts = BTreeSet::new();
    for round in input.completed_rounds {
        for call in &round.calls {
            let identity_hash =
                sha256_hex(input.scanner.redact(&call.identity.value).text.as_bytes());
            let range = call
                .range
                .map(|range| format!("{}:{}", range.start, range.end))
                .unwrap_or_else(|| "-".to_string());
            tool_facts.insert(format!(
                "{}\t{}\t{}\t{}\t{}",
                stable_json(&call.tool_kind),
                stable_json(&call.identity.kind),
                identity_hash,
                range,
                stable_json(&call.outcome),
            ));
        }
    }
    canonical.push_str("tools\n");
    for fact in tool_facts {
        canonical.push_str(&fact);
        canonical.push('\n');
    }

    let notes_hash = sha256_hex(input.scanner.redact(input.notes_text).text.as_bytes());
    canonical.push_str(&format!(
        "notes\t{}\t{}\t{}\n",
        notes_hash,
        input.notes_text.chars().count(),
        input.notes_carried
    ));

    sha256_hex(canonical.as_bytes())
}

fn step_status_label(status: StepStatus) -> &'static str {
    match status {
        StepStatus::Pending => "pending",
        StepStatus::InProgress => "in_progress",
        StepStatus::Completed => "completed",
        StepStatus::Blocked => "blocked",
        StepStatus::Skipped => "skipped",
    }
}

fn evidence_fact(scanner: &SecretScanner, evidence: &Evidence) -> String {
    match evidence {
        Evidence::CommandExit {
            exit_code,
            purpose,
            command_hash,
            ..
        } => format!(
            "command-exit\t{:?}\t{}\t{}",
            exit_code,
            purpose.as_str(),
            command_hash.as_deref().unwrap_or("-")
        ),
        Evidence::CommandDeclined { command_hash, .. } => format!(
            "command-declined\t{}",
            command_hash.as_deref().unwrap_or("-")
        ),
        Evidence::PatchApplied { files, .. } => {
            let mut fact = String::from("patch-applied");
            let mut files: Vec<_> = files.iter().collect();
            files.sort_by(|left, right| left.path.cmp(&right.path));
            for file in files {
                fact.push('\t');
                fact.push_str(&sha256_hex(scanner.redact(&file.path).text.as_bytes()));
                fact.push('\t');
                fact.push_str(&file.applied_hash);
            }
            fact
        }
        Evidence::FileRead { path, hash } => format!(
            "file-read\t{}\t{}",
            sha256_hex(scanner.redact(path).text.as_bytes()),
            hash
        ),
        Evidence::Findings { refs, failing } => {
            format!("findings\t{}\t{}", refs.len(), failing)
        }
    }
}

fn stable_json<T: Serialize>(value: &T) -> String {
    serde_json::to_string(value).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::file_access::LineRange;
    use crate::plan::{PatchedFile, PlanStep};
    use crate::within_turn_compaction::{
        CompactedToolKind, CompletedToolCall, ToolIdentity, ToolResultOutcome,
    };

    fn budget(action: u32, exploration: u32, input: u64, output: u64) -> TaskExecutionBudget {
        TaskExecutionBudget {
            action_rounds: action,
            exploration_rounds: exploration,
            input_tokens: input,
            output_tokens: output,
        }
    }

    fn usage(input: u64, output: u64) -> TaskUsage {
        TaskUsage {
            input_tokens: input,
            output_tokens: output,
            source: UsageSource::Measured,
            reported_cost: None,
            run_count: 1,
            cached_input_tokens: None,
            cache_reported_input_tokens: 0,
            runs_without_cache_report: 0,
            models: Vec::new(),
        }
    }

    fn decide(
        budget: &TaskExecutionBudget,
        action: u32,
        exploration: u32,
        usage: Option<&TaskUsage>,
        progress: bool,
    ) -> SegmentContinuationDecision {
        decide_segment_continuation(&SegmentContinuationInput {
            budget,
            totals: SegmentTotals {
                action_rounds: action,
                exploration_rounds: exploration,
            },
            usage,
            normal_action_rounds: 8,
            normal_exploration_rounds: 24,
            progress_advanced: progress,
        })
    }

    #[test]
    fn a_smaller_total_caps_both_allowances() {
        let budget = budget(3, 2, 1_000, 100);
        let usage = usage(10, 10);
        assert_eq!(
            decide(&budget, 0, 0, Some(&usage), true),
            SegmentContinuationDecision::Continue {
                action_rounds: 3,
                exploration_rounds: 2,
            }
        );
    }

    #[test]
    fn each_approved_total_independently_declines() {
        let budget = budget(4, 6, 1_000, 100);
        let cheap = usage(0, 0);
        assert_eq!(
            decide(&budget, 4, 0, Some(&cheap), true),
            SegmentContinuationDecision::Declined(SegmentDeclineReason::ActionRoundsExhausted)
        );
        assert_eq!(
            decide(&budget, 0, 6, Some(&cheap), true),
            SegmentContinuationDecision::Declined(SegmentDeclineReason::ExplorationRoundsExhausted)
        );
        let input_spent = usage(1_000, 0);
        assert_eq!(
            decide(&budget, 0, 0, Some(&input_spent), true),
            SegmentContinuationDecision::Declined(SegmentDeclineReason::InputTokensExhausted)
        );
        let output_spent = usage(1, 100);
        assert_eq!(
            decide(&budget, 0, 0, Some(&output_spent), true),
            SegmentContinuationDecision::Declined(SegmentDeclineReason::OutputTokensExhausted)
        );
    }

    #[test]
    fn missing_usage_is_not_free_and_unchanged_progress_declines() {
        let budget = budget(4, 6, 1_000, 100);
        assert_eq!(
            decide(&budget, 0, 0, None, true),
            SegmentContinuationDecision::Declined(SegmentDeclineReason::UsageUnavailable)
        );
        let usage = usage(1, 1);
        assert_eq!(
            decide(&budget, 0, 0, Some(&usage), false),
            SegmentContinuationDecision::Declined(SegmentDeclineReason::NoProgress)
        );
    }

    #[test]
    fn task_totals_continue_past_the_legacy_turn_caps_to_the_approved_total() {
        // 64 and 32 are the legacy one-turn/extension caps. They must not end a
        // task whose approved totals are larger.
        let budget = budget(100, 90, 1_000, 100);
        let usage = usage(10, 10);

        assert_eq!(
            decide(&budget, 64, 0, Some(&usage), true),
            SegmentContinuationDecision::Continue {
                action_rounds: 8,
                exploration_rounds: 24,
            }
        );
        assert_eq!(
            decide(&budget, 0, 32, Some(&usage), true),
            SegmentContinuationDecision::Continue {
                action_rounds: 8,
                exploration_rounds: 24,
            }
        );
        assert_eq!(
            decide(&budget, 99, 89, Some(&usage), true),
            SegmentContinuationDecision::Continue {
                action_rounds: 1,
                exploration_rounds: 1,
            }
        );
        assert_eq!(
            decide(&budget, 100, 0, Some(&usage), true),
            SegmentContinuationDecision::Declined(SegmentDeclineReason::ActionRoundsExhausted)
        );
        assert_eq!(
            decide(&budget, 0, 90, Some(&usage), true),
            SegmentContinuationDecision::Declined(SegmentDeclineReason::ExplorationRoundsExhausted)
        );
    }

    #[test]
    fn the_product_task_ceilings_remain_the_hard_bound() {
        let budget = budget(
            ABSOLUTE_TASK_ACTION_ROUND_CEILING,
            ABSOLUTE_TASK_EXPLORATION_ROUND_CEILING,
            1_000,
            100,
        );
        let usage = usage(10, 10);
        assert_eq!(
            decide(
                &budget,
                ABSOLUTE_TASK_ACTION_ROUND_CEILING - 1,
                ABSOLUTE_TASK_EXPLORATION_ROUND_CEILING - 1,
                Some(&usage),
                true,
            ),
            SegmentContinuationDecision::Continue {
                action_rounds: 1,
                exploration_rounds: 1,
            }
        );
        assert_eq!(
            decide(
                &budget,
                ABSOLUTE_TASK_ACTION_ROUND_CEILING,
                0,
                Some(&usage),
                true,
            ),
            SegmentContinuationDecision::Declined(SegmentDeclineReason::ActionRoundsExhausted)
        );
        assert_eq!(
            decide(
                &budget,
                0,
                ABSOLUTE_TASK_EXPLORATION_ROUND_CEILING,
                Some(&usage),
                true,
            ),
            SegmentContinuationDecision::Declined(SegmentDeclineReason::ExplorationRoundsExhausted)
        );
    }

    fn request_with_messages(text: &str, max_tokens: Option<u32>) -> ModelRequest {
        ModelRequest {
            provider: "openai".to_string(),
            model: "test-model".to_string(),
            messages: vec![crate::model::ModelMessage::user(text)],
            temperature: None,
            reasoning_level: None,
            stream: true,
            tools: None,
            max_tokens,
            request_usage: true,
            emit_cache_breakpoints: false,
        }
    }

    #[test]
    fn a_request_may_not_exceed_the_remaining_input_total() {
        let request = request_with_messages(&"x".repeat(4_000), Some(512));
        let required = model_request_json(&request).len().div_ceil(4) as u64;
        let budget = budget(10, 10, required + 100, 100);
        let used = usage(1, 0);

        assert_eq!(
            task_request_output_cap(&budget, Some(&used), Some(512)),
            Ok(100)
        );
        assert_eq!(
            authorize_task_request(&request, &budget, Some(&used)),
            Ok(())
        );

        let nearly_spent = usage(required + 100, 0);
        assert_eq!(
            authorize_task_request(&request, &budget, Some(&nearly_spent)),
            Err(TaskRequestTokenAuthority::InputExceeded {
                required_input_tokens: required,
                remaining_input_tokens: 0,
            })
        );
    }

    #[test]
    fn the_outgoing_request_is_capped_to_the_remaining_output_total() {
        let request = request_with_messages("hello", Some(8_192));
        let budget = budget(10, 10, 10_000, 100);

        assert_eq!(
            task_request_output_cap(&budget, Some(&usage(10, 40)), Some(8_192)),
            Ok(60)
        );
        // No configured provider ceiling means the approved remainder is the
        // cap that must be applied before Task 4 measures the request.
        assert_eq!(
            task_request_output_cap(&budget, Some(&usage(10, 40)), None),
            Ok(60)
        );
        assert_eq!(
            authorize_task_request(&request, &budget, Some(&usage(10, 100))),
            Err(TaskRequestTokenAuthority::OutputExhausted {
                remaining_output_tokens: 0,
            })
        );
        assert_eq!(
            task_request_output_cap(&budget, Some(&usage(10, 100)), Some(8_192)),
            Err(TaskRequestTokenAuthority::OutputExhausted {
                remaining_output_tokens: 0,
            })
        );
        assert_eq!(
            authorize_task_request(&request, &budget, None),
            Err(TaskRequestTokenAuthority::UsageUnavailable)
        );
        assert_eq!(
            task_request_output_cap(&budget, None, Some(8_192)),
            Err(TaskRequestTokenAuthority::UsageUnavailable)
        );
    }

    #[test]
    fn a_max_tokens_field_applied_after_measurement_cannot_hide_a_threshold_crossing() {
        use crate::request_preparation::{
            RequestPreparationInput, RequestPreparationOutcome, prepare_request,
        };

        let scanner = SecretScanner::default();
        let budget = budget(10, 10, 10_000, 100);
        let used = usage(0, 0);
        // Build the request the way Task 4 used to see it: no output cap yet.
        let uncapped = request_with_messages(&"x".repeat(4_000), None);
        let uncapped_tokens = model_request_json(&uncapped).len().div_ceil(4);
        // The threshold is exactly the uncapped size, so adding the
        // `max_tokens` field is the only thing that crosses it.
        let context_window = uncapped_tokens;
        let pinned = BTreeSet::new();
        fn input<'a>(
            request: &'a ModelRequest,
            pinned: &'a BTreeSet<String>,
            scanner: &'a SecretScanner,
            context_window: usize,
        ) -> RequestPreparationInput<'a> {
            RequestPreparationInput {
                request,
                completed_rounds: &[],
                pinned_call_ids: pinned,
                context_window: Some(context_window),
                window_share_bps: 10_000,
                scanner,
                session_id: "session",
                task_id: "task",
            }
        }

        assert_eq!(
            prepare_request(&input(&uncapped, &pinned, &scanner, context_window)),
            RequestPreparationOutcome::Unchanged,
            "the uncapped request is exactly at the threshold"
        );

        let cap = task_request_output_cap(&budget, Some(&used), None)
            .expect("a positive remaining output total");
        let mut capped = uncapped.clone();
        capped.max_tokens = Some(cap);
        assert!(
            model_request_json(&capped).len().div_ceil(4) > context_window,
            "the cap field itself must cross the threshold"
        );
        assert!(
            matches!(
                prepare_request(&input(&capped, &pinned, &scanner, context_window)),
                RequestPreparationOutcome::FailedClosed { .. }
            ),
            "the capped request must be measured, not the uncapped shape"
        );
    }

    fn snapshot<'a>(
        plan: Option<&'a TaskPlan>,
        evidence: &'a [Evidence],
        rounds: &'a [CompletedToolRound],
        notes: &'a str,
        scanner: &'a SecretScanner,
    ) -> SegmentProgressInput<'a> {
        SegmentProgressInput {
            plan,
            step_evidence: evidence,
            completed_rounds: rounds,
            notes_text: notes,
            notes_carried: false,
            scanner,
        }
    }

    fn call(id: &str, path: &str) -> CompletedToolRound {
        CompletedToolRound {
            calls: vec![CompletedToolCall {
                call_id: id.to_string(),
                tool_kind: CompactedToolKind::ReadFile,
                identity: ToolIdentity::path(path),
                range: Some(LineRange { start: 1, end: 10 }),
                outcome: ToolResultOutcome::Succeeded,
                include_excerpt: false,
            }],
        }
    }

    fn plan(status: StepStatus) -> TaskPlan {
        let mut plan = TaskPlan::new("task_1", 1);
        plan.steps.push(PlanStep {
            id: "step_1".to_string(),
            title: "Read the retry helper".to_string(),
            detail: None,
            status,
            depends_on: Vec::new(),
            started_at_ms: None,
            completed_at_ms: None,
            evidence: Vec::new(),
            check: None,
        });
        plan
    }

    #[test]
    fn repeating_the_same_call_under_a_new_id_is_not_progress() {
        let scanner = SecretScanner::default();
        let plan = plan(StepStatus::InProgress);
        let first = vec![call("call_1", "src/a.rs")];
        let repeated = vec![call("call_2", "src/a.rs")];
        let left = progress_digest(&snapshot(Some(&plan), &[], &first, "", &scanner));
        let right = progress_digest(&snapshot(Some(&plan), &[], &repeated, "", &scanner));
        assert_eq!(left, right);

        let distinct = vec![call("call_3", "src/b.rs")];
        let third = progress_digest(&snapshot(Some(&plan), &[], &distinct, "", &scanner));
        assert_ne!(left, third);
    }

    #[test]
    fn plan_evidence_and_note_changes_are_progress() {
        let scanner = SecretScanner::default();
        let base = plan(StepStatus::Pending);
        let started = plan(StepStatus::InProgress);
        let none = progress_digest(&snapshot(Some(&base), &[], &[], "", &scanner));
        let advanced = progress_digest(&snapshot(Some(&started), &[], &[], "", &scanner));
        assert_ne!(none, advanced);

        let note = progress_digest(&snapshot(Some(&started), &[], &[], "found it", &scanner));
        assert_ne!(advanced, note);

        let evidence = [Evidence::FileRead {
            path: "src/a.rs".to_string(),
            hash: "abc".to_string(),
        }];
        let ran = progress_digest(&snapshot(Some(&started), &evidence, &[], "", &scanner));
        assert_ne!(advanced, ran);
    }

    #[test]
    fn a_digest_is_stable_and_content_safe() {
        let scanner = SecretScanner::default();
        let plan = plan(StepStatus::Completed);
        let rounds = vec![call("call_1", "src/secret.rs")];
        let left = progress_digest(&snapshot(Some(&plan), &[], &rounds, "note", &scanner));
        let right = progress_digest(&snapshot(Some(&plan), &[], &rounds, "note", &scanner));
        assert_eq!(left, right);
        assert_eq!(left.len(), 64);
        assert!(left.chars().all(|character| character.is_ascii_hexdigit()));
        assert!(!left.contains("src/secret.rs"));
    }

    #[test]
    fn an_identity_is_redacted_before_hashing() {
        let scanner = SecretScanner::default();
        let secret = "ghp_abcdefghijklmnopqrstuvwxyz0123456789";
        let rounds = vec![call("call_1", &format!("src/{secret}.rs"))];
        let digest = progress_digest(&snapshot(None, &[], &rounds, "", &scanner));
        assert!(!digest.contains(secret));

        let redacted = calls_redacted_digest(&scanner, secret);
        assert_eq!(digest, redacted, "raw and pre-redacted identities agree");
    }

    /// The digest hashes the *redacted* identity, so an already-redacted value
    /// produces the same result rather than a second, different fact.
    fn calls_redacted_digest(scanner: &SecretScanner, secret: &str) -> String {
        let redacted = scanner.redact(&format!("src/{secret}.rs")).text;
        let rounds = vec![call("call_1", &redacted)];
        progress_digest(&snapshot(None, &[], &rounds, "", scanner))
    }

    #[test]
    fn patch_and_command_evidence_are_typed_not_textual() {
        let command = Evidence::CommandExit {
            marker_id: "act_1".to_string(),
            exit_code: Some(0),
            purpose: crate::command_purpose::CommandPurpose::Check,
            command_hash: Some("hash".to_string()),
            working_directory: String::new(),
        };
        let patched = Evidence::PatchApplied {
            marker_id: "act_2".to_string(),
            files: vec![PatchedFile {
                path: "src/a.rs".to_string(),
                applied_hash: "applied".to_string(),
            }],
        };
        let scanner = SecretScanner::default();
        let left = evidence_fact(&scanner, &command);
        let right = evidence_fact(&scanner, &patched);
        assert!(left.contains("check"));
        assert!(left.contains("hash"));
        assert!(!left.contains("act_1"));
        assert!(right.contains("applied"));
        assert!(!right.contains("act_2"));
        assert!(!right.contains("src/a.rs"));
    }

    #[test]
    fn the_event_payloads_carry_no_content_fields() {
        let started = TaskSegmentStarted {
            format_version: TASK_SEGMENT_FORMAT_VERSION,
            session_id: "ses_1".to_string(),
            task_id: "task_1".to_string(),
            segment: 2,
            segment_action_rounds: 0,
            segment_exploration_rounds: 0,
            total_action_rounds: 8,
            total_exploration_rounds: 12,
            tokens: SegmentTokenTotals {
                input_tokens: 100,
                output_tokens: 20,
                source: Some(UsageSource::Estimated),
            },
            start_digest: "a".repeat(64),
        };
        let json = serde_json::to_string(&started).unwrap();
        for forbidden in ["prompt", "content", "command", "path", "finding"] {
            assert!(!json.contains(forbidden), "{json}");
        }
        assert!(json.contains("\"tokens\""));
        assert!(json.contains("\"source\":\"estimated\""));
        assert!(
            started
                .audit_fields()
                .iter()
                .any(|(key, _)| *key == "startDigest")
        );
    }
}
