//! Safety-pinned request preparation (spec 103 Task 4).
//!
//! [`prepare_request`] sits between the turn's message selection and the model
//! adapter. It measures the complete provider request — model name, options,
//! native tool schemas and the message array — and, only when that complete
//! request would exceed the configured share of the resolved context window,
//! asks [`crate::within_turn_compaction`] to replace eligible completed tool
//! results with manifests.
//!
//! Two rules are load-bearing:
//!
//! * The caller supplies the messages it would actually send and every pin it
//!   derived from typed engine state. This module never infers a tool kind,
//!   identity, success or unresolved outcome from message text.
//! * A failure carries no request. `InvalidInput` and `CannotFit` must never
//!   lead to a partially reduced request being sent.
//!
//! The full-request accounting subtracts the exact unchanged request overhead
//! (everything outside the message array) before asking the reducer for a
//! message target, then re-measures the actual serialized request after
//! compaction. Integer rounding therefore cannot claim a request fits when the
//! complete body is still over the threshold.
//!
//! An `Unchanged` reduction is never sent: the caller only reaches this module
//! after measuring the complete request over the threshold, so an unchanged
//! message view means nothing was reduced. The deterministic minimum for a
//! request whose fixed overhead alone is too large is measured by serializing
//! the same request with an empty message array rather than inferred by
//! subtracting estimates.

use serde::{Deserialize, Serialize};

use crate::context_manager::TokenEstimate;
use crate::model::{ModelRequest, model_request_json, provider_messages_json};
use crate::secret_scanner::SecretScanner;
use crate::within_turn_compaction::{
    CompactedResultFact, CompletedToolRound, WithinTurnCompactionInvalidInput,
    WithinTurnCompactionOutcome, WithinTurnCompactionRequest, compact_within_turn,
};

/// The attempt-record shape this build writes.
pub const WITHIN_TURN_REQUEST_FORMAT_VERSION: u32 = 1;

/// Why a threshold-crossing preparation was attempted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WithinTurnCompactionAttemptOutcome {
    Compacted,
    CannotFit,
    InvalidInput,
}

/// One durable, content-safe record of a request that crossed the window share.
///
/// Every string is an identifier, a redacted identity, a hash or a format name.
/// No tool body is stored: the session transcript keeps the authoritative body,
/// and this record exists so a later reader can tell what the request path did.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WithinTurnCompactionAttempt {
    pub format_version: u32,
    pub session_id: String,
    pub task_id: String,
    pub outcome: WithinTurnCompactionAttemptOutcome,
    pub context_window_tokens: usize,
    pub window_share_bps: u16,
    pub threshold_tokens: usize,
    /// Complete-request estimates, including model/options/tool schemas.
    pub request_tokens_before: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_tokens_after: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_tokens_minimum: Option<usize>,
    /// The message-array view the reducer measured.
    pub message_tokens_before: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message_tokens_after: Option<usize>,
    pub message_target_tokens: usize,
    /// The unchanged part of `model_request_json`: everything that is not the
    /// message array.
    pub fixed_overhead_tokens: usize,
    pub completed_rounds: usize,
    pub pinned_results: usize,
    pub eligible_results: usize,
    pub compacted_results: usize,
    pub ineligible_results: usize,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub facts: Vec<CompactedResultFact>,
    /// The typed invalid-input reason, already redacted. `None` for the other
    /// outcomes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub invalid_input: Option<String>,
}

impl WithinTurnCompactionAttempt {
    /// The stable field set the audit log carries. No field contains a tool
    /// body; the audit log redacts every value again on write.
    pub fn audit_fields(&self) -> Vec<(&'static str, String)> {
        let mut fields = vec![
            ("actor", "system".to_string()),
            ("sessionId", self.session_id.clone()),
            ("taskId", self.task_id.clone()),
            ("formatVersion", self.format_version.to_string()),
            (
                "outcome",
                match self.outcome {
                    WithinTurnCompactionAttemptOutcome::Compacted => "compacted",
                    WithinTurnCompactionAttemptOutcome::CannotFit => "cannot_fit",
                    WithinTurnCompactionAttemptOutcome::InvalidInput => "invalid_input",
                }
                .to_string(),
            ),
            (
                "contextWindowTokens",
                self.context_window_tokens.to_string(),
            ),
            ("windowShareBps", self.window_share_bps.to_string()),
            ("thresholdTokens", self.threshold_tokens.to_string()),
            (
                "requestTokensBefore",
                self.request_tokens_before.to_string(),
            ),
            (
                "messageTokensBefore",
                self.message_tokens_before.to_string(),
            ),
            (
                "messageTargetTokens",
                self.message_target_tokens.to_string(),
            ),
            (
                "fixedOverheadTokens",
                self.fixed_overhead_tokens.to_string(),
            ),
            ("completedRounds", self.completed_rounds.to_string()),
            ("pinnedResults", self.pinned_results.to_string()),
            ("eligibleResults", self.eligible_results.to_string()),
            ("compactedResults", self.compacted_results.to_string()),
            ("ineligibleResults", self.ineligible_results.to_string()),
        ];
        if let Some(after) = self.request_tokens_after {
            fields.push(("requestTokensAfter", after.to_string()));
        }
        if let Some(minimum) = self.request_tokens_minimum {
            fields.push(("requestTokensMinimum", minimum.to_string()));
        }
        if !self.facts.is_empty() {
            // Facts are the reducer's already-redacted identities, hashes,
            // ranges, outcomes and counts. There is no body to leak.
            fields.push((
                "manifests",
                serde_json::to_string(&self.facts).unwrap_or_else(|_| "[]".to_string()),
            ));
        }
        if let Some(invalid_input) = &self.invalid_input {
            fields.push(("invalidInput", invalid_input.clone()));
        }
        fields
    }
}

/// What the request path may send.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RequestPreparationOutcome {
    /// No resolved window, or the complete request already fits. The caller
    /// sends exactly the request it built, and records nothing.
    Unchanged,
    /// The reducer reduced the request and the complete serialized body now
    /// fits the threshold. The returned request is what must be sent.
    Compacted {
        request: ModelRequest,
        attempt: WithinTurnCompactionAttempt,
    },
    /// The request must not be sent. Carries the durable attempt to record, but
    /// no request messages.
    FailedClosed {
        attempt: WithinTurnCompactionAttempt,
    },
}

/// Everything [`prepare_request`] needs. All values are explicit; there is no
/// ambient session or transcript to read.
#[derive(Debug)]
pub struct RequestPreparationInput<'a> {
    /// The complete request as the caller would send it today.
    pub request: &'a ModelRequest,
    pub completed_rounds: &'a [CompletedToolRound],
    /// Call ids that must remain byte-for-byte verbatim.
    pub pinned_call_ids: &'a std::collections::BTreeSet<String>,
    /// The resolved provider context window. `None` preserves existing request
    /// behaviour: no threshold and no attempt.
    pub context_window: Option<usize>,
    /// `Config::conversation_window_share`, in basis points.
    pub window_share_bps: u16,
    pub scanner: &'a SecretScanner,
    pub session_id: &'a str,
    pub task_id: &'a str,
}

/// The configured request-window share of a resolved context window.
pub fn request_threshold_tokens(context_window: usize, window_share_bps: u16) -> usize {
    context_window * usize::from(window_share_bps) / 10_000
}

/// Prepares the next model request.
///
/// The function only compacts when the complete serialized request exceeds the
/// configured threshold. It re-measures the complete body after reduction, so
/// a message-only fit cannot mask fixed overhead (model name, options or native
/// tool schemas) pushing the real request over the window share.
pub fn prepare_request(input: &RequestPreparationInput<'_>) -> RequestPreparationOutcome {
    let full_before = model_request_json(input.request);
    let request_tokens_before = TokenEstimate::Prose.tokens(&full_before);
    let Some(context_window) = input.context_window else {
        return RequestPreparationOutcome::Unchanged;
    };
    let threshold = request_threshold_tokens(context_window, input.window_share_bps);
    if request_tokens_before <= threshold {
        return RequestPreparationOutcome::Unchanged;
    }

    let messages_json = provider_messages_json(&input.request.messages);
    let message_tokens_before = TokenEstimate::Prose.tokens(&messages_json);
    // `model_request_json` always embeds `provider_messages_json` verbatim, so
    // this subtraction is the exact unchanged overhead. `saturating_sub`
    // cannot hide a negative value: a serializer change that stopped embedding
    // the array would leave overhead at zero and the target at the threshold.
    let overhead_bytes = full_before.len().saturating_sub(messages_json.len());
    let fixed_overhead_tokens = overhead_bytes.div_ceil(4);
    let target_bytes = threshold.saturating_mul(4);
    let message_target_tokens = target_bytes.saturating_sub(overhead_bytes) / 4;

    // The reducer classifies every completed call in the same precedence:
    // pinned first, then ineligible outcome, then eligible. Counting here keeps
    // the durable record complete for a successful reduction too, where the
    // reducer returns facts but no refusal counts.
    let mut pinned_results = 0usize;
    let mut eligible_results = 0usize;
    let mut ineligible_results = 0usize;
    for round in input.completed_rounds {
        for call in &round.calls {
            if input.pinned_call_ids.contains(&call.call_id) {
                pinned_results += 1;
            } else if call.outcome.is_eligible() {
                eligible_results += 1;
            } else {
                ineligible_results += 1;
            }
        }
    }

    let context = AttemptContext {
        session_id: input.session_id,
        task_id: input.task_id,
        scanner: input.scanner,
        context_window,
        window_share_bps: input.window_share_bps,
        threshold,
        request_tokens_before,
        message_tokens_before,
        message_target_tokens,
        fixed_overhead_tokens,
        completed_rounds: input.completed_rounds.len(),
        pinned_results,
        eligible_results,
        ineligible_results,
    };

    if target_bytes < overhead_bytes {
        // The fixed request alone is over the threshold. No reduction of the
        // message array can make it fit, so the deterministic minimum is the
        // same request with nothing left in the message array.
        let attempt = context.attempt(
            WithinTurnCompactionAttemptOutcome::CannotFit,
            Some(message_tokens_before),
            None,
            Some(minimum_full_request_tokens(input.request)),
            0,
            Vec::new(),
            None,
        );
        return RequestPreparationOutcome::FailedClosed { attempt };
    }

    let compaction = compact_within_turn(&WithinTurnCompactionRequest {
        messages: &input.request.messages,
        rounds: input.completed_rounds,
        pinned_call_ids: input.pinned_call_ids,
        token_target: message_target_tokens,
        scanner: input.scanner,
    });

    resolve_reduction(input.request, threshold, compaction, &context)
}

/// The fields every attempt record shares, independent of what the reducer
/// decided. Keeping them in one place lets the outcome resolution be tested
/// without rebuilding the whole preparation input.
struct AttemptContext<'a> {
    session_id: &'a str,
    task_id: &'a str,
    scanner: &'a SecretScanner,
    context_window: usize,
    window_share_bps: u16,
    threshold: usize,
    request_tokens_before: usize,
    message_tokens_before: usize,
    message_target_tokens: usize,
    fixed_overhead_tokens: usize,
    completed_rounds: usize,
    pinned_results: usize,
    eligible_results: usize,
    ineligible_results: usize,
}

impl AttemptContext<'_> {
    #[allow(clippy::too_many_arguments)]
    fn attempt(
        &self,
        outcome: WithinTurnCompactionAttemptOutcome,
        message_tokens_after: Option<usize>,
        request_tokens_after: Option<usize>,
        request_tokens_minimum: Option<usize>,
        compacted_results: usize,
        facts: Vec<CompactedResultFact>,
        invalid_input: Option<String>,
    ) -> WithinTurnCompactionAttempt {
        WithinTurnCompactionAttempt {
            format_version: WITHIN_TURN_REQUEST_FORMAT_VERSION,
            session_id: self.session_id.to_string(),
            task_id: self.task_id.to_string(),
            outcome,
            context_window_tokens: self.context_window,
            window_share_bps: self.window_share_bps,
            threshold_tokens: self.threshold,
            request_tokens_before: self.request_tokens_before,
            request_tokens_after,
            request_tokens_minimum,
            message_tokens_before: self.message_tokens_before,
            message_tokens_after,
            message_target_tokens: self.message_target_tokens,
            fixed_overhead_tokens: self.fixed_overhead_tokens,
            completed_rounds: self.completed_rounds,
            pinned_results: self.pinned_results,
            eligible_results: self.eligible_results,
            compacted_results,
            ineligible_results: self.ineligible_results,
            facts,
            invalid_input,
        }
    }
}

/// Turns a reducer outcome into what the caller may send.
///
/// `Unchanged` is always a refusal. The caller runs the reducer only after it
/// has measured the complete request over the threshold, so an unchanged
/// message view means the request was never reduced. Deriving a "fit" from the
/// message estimate alone would let integer arithmetic claim a body that was
/// never measured on the serializer the provider sees.
fn resolve_reduction(
    original: &ModelRequest,
    threshold: usize,
    compaction: WithinTurnCompactionOutcome,
    context: &AttemptContext<'_>,
) -> RequestPreparationOutcome {
    match compaction {
        WithinTurnCompactionOutcome::Compacted(view) => {
            let mut request = original.clone();
            request.messages = view.messages;
            let full_after = model_request_json(&request);
            let request_tokens_after = TokenEstimate::Prose.tokens(&full_after);
            if request_tokens_after > threshold {
                // The reducer met its message target but the complete body did
                // not fit. Rounding must never be allowed to claim otherwise.
                let attempt = context.attempt(
                    WithinTurnCompactionAttemptOutcome::CannotFit,
                    Some(view.after_tokens),
                    None,
                    Some(request_tokens_after),
                    view.compacted_results,
                    view.facts,
                    None,
                );
                return RequestPreparationOutcome::FailedClosed { attempt };
            }
            let attempt = context.attempt(
                WithinTurnCompactionAttemptOutcome::Compacted,
                Some(view.after_tokens),
                Some(request_tokens_after),
                None,
                view.compacted_results,
                view.facts,
                None,
            );
            RequestPreparationOutcome::Compacted { request, attempt }
        }
        WithinTurnCompactionOutcome::Unchanged(view) => {
            // No eligible result was replaced. The complete request measured
            // before reduction is the smallest body this preparation can send,
            // and it is over the threshold.
            let attempt = context.attempt(
                WithinTurnCompactionAttemptOutcome::CannotFit,
                Some(view.after_tokens),
                None,
                Some(context.request_tokens_before),
                0,
                Vec::new(),
                None,
            );
            RequestPreparationOutcome::FailedClosed { attempt }
        }
        WithinTurnCompactionOutcome::CannotFit(refusal) => {
            let request_tokens_minimum = request_tokens_from_message_tokens(
                refusal.minimum_tokens,
                context.fixed_overhead_tokens,
            );
            let attempt = context.attempt(
                WithinTurnCompactionAttemptOutcome::CannotFit,
                None,
                None,
                Some(request_tokens_minimum),
                refusal.compacted_results,
                refusal.facts,
                None,
            );
            RequestPreparationOutcome::FailedClosed { attempt }
        }
        WithinTurnCompactionOutcome::InvalidInput(error) => {
            let invalid_input = context.scanner.redact(&describe_invalid_input(&error)).text;
            let attempt = context.attempt(
                WithinTurnCompactionAttemptOutcome::InvalidInput,
                None,
                None,
                None,
                0,
                Vec::new(),
                Some(invalid_input),
            );
            RequestPreparationOutcome::FailedClosed { attempt }
        }
    }
}

/// The smallest complete request this model, option and tool-schema set can
/// produce: the same request with an empty message array.
///
/// This is measured through the provider serializer rather than inferred from
/// token estimates, so the reported minimum is an exact full-request figure.
fn minimum_full_request_tokens(request: &ModelRequest) -> usize {
    let mut empty = request.clone();
    empty.messages.clear();
    TokenEstimate::Prose.tokens(&model_request_json(&empty))
}

fn request_tokens_from_message_tokens(
    message_tokens: usize,
    fixed_overhead_tokens: usize,
) -> usize {
    // Each figure is already a ceiling. Keeping the addition conservative is
    // the safe direction for a recorded minimum: it can over-report the minimum
    // by at most one token but can never claim a smaller body than exists.
    message_tokens.saturating_add(fixed_overhead_tokens)
}

fn describe_invalid_input(error: &WithinTurnCompactionInvalidInput) -> String {
    match error {
        WithinTurnCompactionInvalidInput::DuplicateCompletedCallId { call_id } => {
            format!("duplicate completed call id: {call_id}")
        }
        WithinTurnCompactionInvalidInput::DuplicateToolCallDeclaration { call_id } => {
            format!("duplicate tool-call declaration: {call_id}")
        }
        WithinTurnCompactionInvalidInput::DuplicateToolResult { call_id } => {
            format!("duplicate tool result: {call_id}")
        }
        WithinTurnCompactionInvalidInput::MissingToolCallDeclaration { call_id } => {
            format!("missing tool-call declaration: {call_id}")
        }
        WithinTurnCompactionInvalidInput::MissingToolResult { call_id } => {
            format!("missing tool result: {call_id}")
        }
        WithinTurnCompactionInvalidInput::ResultBeforeDeclaration { call_id } => {
            format!("tool result before its declaration: {call_id}")
        }
        WithinTurnCompactionInvalidInput::SplitRound { round_index } => {
            format!("completed round {round_index} spans two assistant messages")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::ModelMessage;
    use crate::within_turn_compaction::WithinTurnCompaction;

    fn request(messages: Vec<ModelMessage>) -> ModelRequest {
        ModelRequest {
            provider: "openai".to_string(),
            model: "test-model".to_string(),
            messages,
            temperature: Some("0".to_string()),
            reasoning_level: Some("none".to_string()),
            stream: true,
            tools: None,
            max_tokens: Some(512),
            request_usage: true,
            emit_cache_breakpoints: false,
        }
    }

    #[test]
    fn an_unchanged_reduction_after_a_threshold_crossing_never_returns_a_request() {
        let scanner = SecretScanner::default();
        let original = request(vec![ModelMessage::system("SYSTEM")]);
        let context = AttemptContext {
            session_id: "session",
            task_id: "task",
            scanner: &scanner,
            context_window: 1_000,
            window_share_bps: 10_000,
            threshold: 1_000,
            // The complete request is over the threshold; the message estimate
            // below would pass the old "before + overhead <= threshold" check.
            request_tokens_before: 5_000,
            message_tokens_before: 4,
            message_target_tokens: 1,
            fixed_overhead_tokens: 10,
            completed_rounds: 1,
            pinned_results: 0,
            eligible_results: 1,
            ineligible_results: 0,
        };
        let view = WithinTurnCompaction {
            messages: original.messages.clone(),
            before_tokens: 4,
            after_tokens: 4,
            target_tokens: 1,
            compacted_results: 0,
            facts: Vec::new(),
        };

        let outcome = resolve_reduction(
            &original,
            1_000,
            WithinTurnCompactionOutcome::Unchanged(view),
            &context,
        );

        match outcome {
            RequestPreparationOutcome::FailedClosed { attempt } => {
                assert_eq!(
                    attempt.outcome,
                    WithinTurnCompactionAttemptOutcome::CannotFit
                );
                assert_eq!(attempt.compacted_results, 0);
                assert_eq!(attempt.request_tokens_after, None);
                assert_eq!(
                    attempt.request_tokens_minimum,
                    Some(context.request_tokens_before)
                );
            }
            RequestPreparationOutcome::Unchanged => {
                panic!("an unchanged reduction must fail closed, not send the request")
            }
            other => panic!("expected a cannot-fit refusal, got {other:?}"),
        }
    }
}
