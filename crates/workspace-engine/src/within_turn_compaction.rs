//! Within-turn tool-result compaction (spec 103 Task 3).
//!
//! This module is a pure request reducer. A caller that is about to send a
//! model request supplies the messages it would send, the completed tool rounds
//! it knows about, an explicit set of pinned call ids, and an explicit token
//! target. The reducer walks completed rounds oldest-first and replaces eligible
//! tool result bodies with a deterministic manifest until the target fits.
//!
//! Everything the manifest says about a result comes from the caller's typed
//! metadata: tool kind, identity, range and outcome. Nothing here reads prose
//! back to guess what a tool did, whether it succeeded, or whether an external
//! side effect is known. An eligible result is therefore a decision the engine
//! already made, not an interpretation of model-visible text.
//!
//! Two invariants are load-bearing:
//!
//! * Only `tool`-role message bodies change. Assistant tool-call messages,
//!   message order and every `tool_call_id` pairing stay exactly as supplied, so
//!   a batched assistant turn and its results remain provider-valid. A request
//!   whose completed metadata and messages cannot be proven to pair uniquely is
//!   rejected with [`WithinTurnCompactionInvalidInput`] instead of silently
//!   choosing or skipping an entry.
//! * The manifest is serialized JSON inside the tool result. Tool output never
//!   becomes system or user text, and a result whose body contains an
//!   instruction stays a quoted JSON string.
//!
//! Token estimates count the complete provider-visible message, not only its
//! `content`. Role, tool-call ids/names/arguments, `tool_call_id`,
//! `reasoning_content` and JSON framing are constant across compaction but still
//! consume the target budget, so the reducer measures each message through the
//! same serializer the request itself uses.
//!
//! The caller's messages are borrowed and never mutated. This module also does
//! not write a session or audit event; Task 4 owns integration and durable
//! recording.

use std::collections::{BTreeSet, HashMap, HashSet};

use serde::{Deserialize, Serialize};

use crate::context_manager::TokenEstimate;
use crate::file_access::LineRange;
use crate::hash::sha256_hex;
use crate::model::{ModelMessage, provider_message_json, provider_messages_json};
use crate::secret_scanner::SecretScanner;

/// The manifest format this build writes.
///
/// A later reader accepts the versions it knows rather than assuming the
/// current shape; the stored transcript is append-only and a manifest is only
/// ever part of a request.
pub const WITHIN_TURN_MANIFEST_VERSION: u32 = 1;

/// The most characters a manifest excerpt may contain, including its
/// truncation marker.
///
/// A manifest must stay small enough that replacing a result is a reduction
/// rather than a second copy of it. The bound is in characters, not bytes, so
/// truncation cannot split a UTF-8 sequence.
pub const MAX_MANIFEST_EXCERPT_CHARS: usize = 400;

/// The kind of tool that produced a compacted result.
///
/// The caller selects the variant from the action it dispatched. The reducer
/// never derives it from the result text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CompactedToolKind {
    ReadFile,
    ListDirectory,
    SearchContent,
    SearchCodebase,
    ReadGitStatus,
    ReadGitDiff,
    ListWorktrees,
    ReadGitLog,
    Command,
    WebDiagnostic,
    McpCall,
    FetchUrl,
}

/// What a redacted identity names.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolIdentityKind {
    Path,
    Command,
    Query,
    Url,
    Endpoint,
}

/// The stable identity a caller supplies for a tool call.
///
/// `value` is the thing a later reread or retry needs: a repository-relative
/// path, a command line, a search query, a URL, or a server/tool endpoint. It is
/// redacted before it enters a manifest. The caller must leave it empty only
/// when the action genuinely has no identity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolIdentity {
    pub kind: ToolIdentityKind,
    pub value: String,
}

impl ToolIdentity {
    pub fn path(value: impl Into<String>) -> Self {
        Self {
            kind: ToolIdentityKind::Path,
            value: value.into(),
        }
    }

    pub fn command(value: impl Into<String>) -> Self {
        Self {
            kind: ToolIdentityKind::Command,
            value: value.into(),
        }
    }

    pub fn query(value: impl Into<String>) -> Self {
        Self {
            kind: ToolIdentityKind::Query,
            value: value.into(),
        }
    }

    pub fn url(value: impl Into<String>) -> Self {
        Self {
            kind: ToolIdentityKind::Url,
            value: value.into(),
        }
    }

    pub fn endpoint(value: impl Into<String>) -> Self {
        Self {
            kind: ToolIdentityKind::Endpoint,
            value: value.into(),
        }
    }
}

/// The typed outcome the caller observed for one tool result.
///
/// Only `Succeeded` and `FailedResolved` are eligible. An unresolved failure and
/// an external action whose outcome is unknown stay verbatim even when the
/// caller forgot to pin them: dropping either would lose the one fact a later
/// repair or recovery needs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolResultOutcome {
    Succeeded,
    /// The tool failed, and the engine has since resolved the failure.
    FailedResolved,
    /// The tool failed and nothing has resolved it.
    FailedUnresolved,
    /// A side-effecting action ran but its external result is not known.
    UnknownExternalOutcome,
}

impl ToolResultOutcome {
    pub fn is_eligible(self) -> bool {
        matches!(self, Self::Succeeded | Self::FailedResolved)
    }
}

/// One completed tool call and the typed metadata the engine already holds.
///
/// The result body is the matching `tool` message in
/// [`WithinTurnCompactionRequest::messages`], located by `call_id`; this struct
/// deliberately does not carry a second unredacted copy of it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CompletedToolCall {
    /// The stable call/action identifier shared by the assistant tool call and
    /// its `tool` result. This is the pairing key and the pin key.
    pub call_id: String,
    pub tool_kind: CompactedToolKind,
    /// Raw identity as the engine observed it. The reducer redacts it.
    pub identity: ToolIdentity,
    /// The typed line range a reread should use, when one was requested.
    pub range: Option<LineRange>,
    pub outcome: ToolResultOutcome,
    /// Whether this result needs a bounded redacted excerpt in its manifest.
    ///
    /// This is an explicit caller decision, not a size heuristic. `false`
    /// leaves the manifest with only structured facts and the result hash.
    pub include_excerpt: bool,
}

/// The calls that completed in one assistant round, oldest round first.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CompletedToolRound {
    pub calls: Vec<CompletedToolCall>,
}

/// Everything the reducer needs. All fields are explicit; there is no ambient
/// transcript, session or audit store to read.
#[derive(Debug)]
pub struct WithinTurnCompactionRequest<'a> {
    /// The request as it would be sent, including assistant tool-call messages
    /// and their `tool` results.
    pub messages: &'a [ModelMessage],
    /// Completed rounds in chronological order. The slice order is the
    /// oldest-first order; nothing is re-sorted by id or content.
    pub rounds: &'a [CompletedToolRound],
    /// Call ids that must remain byte-for-byte verbatim.
    pub pinned_call_ids: &'a BTreeSet<String>,
    /// The token ceiling the reduced request must reach.
    pub token_target: usize,
    /// The scanner used to redact identities, excerpts and the hashed body.
    pub scanner: &'a SecretScanner,
}

/// One durable, content-safe fact about a result the reducer replaced.
///
/// Every string is already redacted. A later audit can compare call ids,
/// hashes, counts and estimates without storing or re-reading tool output.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CompactedResultFact {
    pub round_index: usize,
    pub call_id: String,
    pub tool_kind: CompactedToolKind,
    pub identity: ToolIdentity,
    pub range: Option<LineRange>,
    pub outcome: ToolResultOutcome,
    pub result_hash: String,
    pub excerpt_included: bool,
    pub before_tokens: usize,
    pub after_tokens: usize,
    pub manifest_version: u32,
}

/// A reduced request view plus the facts Task 4 will audit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WithinTurnCompaction {
    /// The request to send. Message order and every tool-call pairing match the
    /// input; only eligible `tool` bodies differ.
    pub messages: Vec<ModelMessage>,
    pub before_tokens: usize,
    pub after_tokens: usize,
    pub target_tokens: usize,
    pub compacted_results: usize,
    pub facts: Vec<CompactedResultFact>,
}

/// The typed refusal returned when even the fully compacted request is too
/// large.
///
/// This carries no messages: a caller cannot accidentally send a partially
/// reduced request because the target was not met.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WithinTurnCompactionRefusal {
    pub target_tokens: usize,
    pub before_tokens: usize,
    /// The token estimate after every eligible result was replaced. This is
    /// the deterministic minimum for this request and metadata.
    pub minimum_tokens: usize,
    pub eligible_results: usize,
    pub compacted_results: usize,
    pub pinned_results: usize,
    pub ineligible_results: usize,
    pub facts: Vec<CompactedResultFact>,
}

/// The typed invalid-input outcome for a request whose completed metadata and
/// provider message view do not describe the same calls.
///
/// Provider-valid pairing is an invariant, not something the reducer may repair.
/// It never picks one of two results, skips a declaration it cannot find, or
/// reorders messages to make a malformed transcript fit; it returns this error
/// instead, and the outcome carries no messages. A caller that receives it must
/// treat the request as unusable rather than send a partial reduction.
///
/// Only ids named by [`WithinTurnCompactionRequest::rounds`] are required to be
/// paired. Unrelated history calls may remain unrepresented in the metadata,
/// and a pinned id that names no completed round is simply unused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WithinTurnCompactionInvalidInput {
    /// The same completed call id appears more than once across the rounds.
    DuplicateCompletedCallId { call_id: String },
    /// More than one assistant message declares the completed call id.
    DuplicateToolCallDeclaration { call_id: String },
    /// More than one `tool` result carries the completed call id.
    DuplicateToolResult { call_id: String },
    /// No `assistant` message declares the completed call id. `tool_calls` on
    /// any other role do not count as a declaration.
    MissingToolCallDeclaration { call_id: String },
    /// No `tool` message carries the completed call id's result.
    MissingToolResult { call_id: String },
    /// The result message precedes the assistant declaration it belongs to.
    ResultBeforeDeclaration { call_id: String },
    /// One round names calls declared by different assistant messages.
    SplitRound { round_index: usize },
}

/// What the reducer decided.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WithinTurnCompactionOutcome {
    /// The request already fits; no message changed.
    Unchanged(WithinTurnCompaction),
    /// Eligible results were replaced and the target now fits.
    Compacted(WithinTurnCompaction),
    /// The completed metadata and the messages do not form a provably valid
    /// request. This is not a size refusal and carries no messages.
    InvalidInput(WithinTurnCompactionInvalidInput),
    /// Every eligible result was replaced and the target still does not fit.
    CannotFit(WithinTurnCompactionRefusal),
}

impl WithinTurnCompactionOutcome {
    pub fn messages(&self) -> Option<&[ModelMessage]> {
        match self {
            Self::Unchanged(view) | Self::Compacted(view) => Some(&view.messages),
            Self::InvalidInput(_) | Self::CannotFit(_) => None,
        }
    }
}

/// Reduces a request view until it reaches `token_target`.
///
/// Pairing is validated before any estimate is compared with the target, so a
/// malformed request fails closed even when its message view happens to fit.
/// Rounds are then visited in slice order and calls in round order. A result is
/// eligible only when it is not pinned and its typed outcome is eligible. An
/// eligible replacement that would not reduce the estimate is skipped, so the
/// reducer never grows a request. Once the target is reached, later results stay
/// verbatim.
///
/// Every token figure counts the complete provider-visible message view: role,
/// content, tool-call ids/names/arguments, `tool_call_id`, `reasoning_content`
/// and the JSON framing, including the array around the messages. Those
/// constants survive compaction but still occupy the provider's context window,
/// so ignoring them would report a request as fitting when the real one does
/// not.
pub fn compact_within_turn(
    request: &WithinTurnCompactionRequest<'_>,
) -> WithinTurnCompactionOutcome {
    let results = match validate_pairing(request) {
        Ok(results) => results,
        Err(error) => return WithinTurnCompactionOutcome::InvalidInput(error),
    };

    let mut messages = request.messages.to_vec();
    let before_tokens = messages_tokens(&messages);
    let target = request.token_target;
    if before_tokens <= target {
        return WithinTurnCompactionOutcome::Unchanged(WithinTurnCompaction {
            messages,
            before_tokens,
            after_tokens: before_tokens,
            target_tokens: target,
            compacted_results: 0,
            facts: Vec::new(),
        });
    }

    let mut after_tokens = before_tokens;
    let mut facts = Vec::new();
    let mut eligible_results = 0usize;
    let mut pinned_results = 0usize;
    let mut ineligible_results = 0usize;
    let mut reached_target = false;

    'rounds: for (round_index, round) in request.rounds.iter().enumerate() {
        for call in &round.calls {
            // Validation guarantees exactly one result for every completed id.
            let message_index = results[call.call_id.as_str()];
            if request.pinned_call_ids.contains(&call.call_id) {
                pinned_results += 1;
                continue;
            }
            if !call.outcome.is_eligible() {
                ineligible_results += 1;
                continue;
            }

            eligible_results += 1;
            let mut replaced = messages[message_index].clone();
            let result_before = message_tokens(&replaced);
            let manifest = call.render_manifest(&replaced.content, request.scanner);
            let original_content = std::mem::replace(&mut replaced.content, manifest.text.clone());
            let result_after = message_tokens(&replaced);
            if result_after >= result_before {
                continue;
            }

            messages[message_index] = replaced;
            let next_tokens = messages_tokens(&messages);
            if next_tokens >= after_tokens {
                // Rounding at the array boundary must never grow the request.
                messages[message_index].content = original_content;
                continue;
            }

            let fact = call.fact(round_index, result_before, result_after, &manifest);
            after_tokens = next_tokens;
            facts.push(fact);
            if after_tokens <= target {
                reached_target = true;
                break 'rounds;
            }
        }
    }

    if reached_target {
        return WithinTurnCompactionOutcome::Compacted(WithinTurnCompaction {
            messages,
            before_tokens,
            after_tokens,
            target_tokens: target,
            compacted_results: facts.len(),
            facts,
        });
    }

    WithinTurnCompactionOutcome::CannotFit(WithinTurnCompactionRefusal {
        target_tokens: target,
        before_tokens,
        minimum_tokens: after_tokens,
        eligible_results,
        compacted_results: facts.len(),
        pinned_results,
        ineligible_results,
        facts,
    })
}

/// Checks the pairing invariants the provider requires and returns the index of
/// each call id's unique `tool` result.
///
/// The checks, in deterministic walk order:
///
/// * each completed call id appears at most once across all rounds;
/// * no call id is declared by more than one `assistant` tool-call message,
///   anywhere in the message view;
/// * no call id has more than one `tool` result, anywhere in the message view;
/// * every completed id has exactly one `assistant` declaration and one result;
/// * that result follows its declaration;
/// * every call in one [`CompletedToolRound`] belongs to the same assistant
///   tool-call message.
///
/// A declaration only counts on a message whose role is exactly `assistant`.
/// [`ModelMessage`] fields are public, so a malformed transcript can carry
/// `tool_calls` on any role; a provider would not accept such a message as the
/// declaration for a following `tool` result, so it is not one here either.
///
/// Unrelated history calls need no metadata. A pinned id that names no completed
/// call is not validated, because it cannot have been compacted either.
fn validate_pairing<'a>(
    request: &'a WithinTurnCompactionRequest<'a>,
) -> Result<HashMap<&'a str, usize>, WithinTurnCompactionInvalidInput> {
    let mut completed_ids = HashSet::new();
    for round in request.rounds {
        for call in &round.calls {
            if !completed_ids.insert(call.call_id.as_str()) {
                return Err(WithinTurnCompactionInvalidInput::DuplicateCompletedCallId {
                    call_id: call.call_id.clone(),
                });
            }
        }
    }

    let mut declarations = HashMap::new();
    let mut results = HashMap::new();
    for (index, message) in request.messages.iter().enumerate() {
        if message.role == "tool"
            && let Some(call_id) = &message.tool_call_id
            && results.insert(call_id.as_str(), index).is_some()
        {
            return Err(WithinTurnCompactionInvalidInput::DuplicateToolResult {
                call_id: call_id.clone(),
            });
        }
        if message.role == "assistant" {
            for call in &message.tool_calls {
                if declarations.insert(call.id.as_str(), index).is_some() {
                    return Err(
                        WithinTurnCompactionInvalidInput::DuplicateToolCallDeclaration {
                            call_id: call.id.clone(),
                        },
                    );
                }
            }
        }
    }

    for (round_index, round) in request.rounds.iter().enumerate() {
        let mut round_declaration = None;
        for call in &round.calls {
            let Some(&declaration_index) = declarations.get(call.call_id.as_str()) else {
                return Err(
                    WithinTurnCompactionInvalidInput::MissingToolCallDeclaration {
                        call_id: call.call_id.clone(),
                    },
                );
            };
            let Some(&result_index) = results.get(call.call_id.as_str()) else {
                return Err(WithinTurnCompactionInvalidInput::MissingToolResult {
                    call_id: call.call_id.clone(),
                });
            };
            if result_index <= declaration_index {
                return Err(WithinTurnCompactionInvalidInput::ResultBeforeDeclaration {
                    call_id: call.call_id.clone(),
                });
            }
            match round_declaration {
                Some(index) if index != declaration_index => {
                    return Err(WithinTurnCompactionInvalidInput::SplitRound { round_index });
                }
                Some(_) => {}
                None => round_declaration = Some(declaration_index),
            }
        }
    }

    Ok(results)
}

impl CompletedToolCall {
    fn render_manifest(&self, body: &str, scanner: &SecretScanner) -> RenderedManifest {
        let redacted_body = scanner.redact(body).text;
        let result_hash = format!("sha256:{}", sha256_hex(redacted_body.as_bytes()));
        let excerpt = self
            .include_excerpt
            .then(|| bounded_excerpt(&redacted_body));
        let redacted_identity = ToolIdentity {
            kind: self.identity.kind,
            value: scanner.redact(&self.identity.value).text,
        };
        let manifest = ToolResultManifest {
            manifest_version: WITHIN_TURN_MANIFEST_VERSION,
            tool_kind: self.tool_kind,
            identity: ManifestIdentity {
                kind: redacted_identity.kind,
                value: redacted_identity.value.clone(),
            },
            range: self.range,
            outcome: self.outcome,
            result_hash: result_hash.clone(),
            excerpt,
        };
        RenderedManifest {
            // A struct of plain strings and numbers cannot fail to serialize.
            text: serde_json::to_string(&manifest)
                .expect("a tool result manifest is always serializable"),
            result_hash,
            excerpt_included: manifest.excerpt.is_some(),
            redacted_identity,
        }
    }

    fn fact(
        &self,
        round_index: usize,
        before_tokens: usize,
        after_tokens: usize,
        manifest: &RenderedManifest,
    ) -> CompactedResultFact {
        CompactedResultFact {
            round_index,
            call_id: self.call_id.clone(),
            tool_kind: self.tool_kind,
            identity: manifest.redacted_identity.clone(),
            range: self.range,
            outcome: self.outcome,
            result_hash: manifest.result_hash.clone(),
            excerpt_included: manifest.excerpt_included,
            before_tokens,
            after_tokens,
            manifest_version: WITHIN_TURN_MANIFEST_VERSION,
        }
    }
}

/// The serialized shape of a compacted result. Only redacted structured facts,
/// a hash and an optional bounded excerpt.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ToolResultManifest {
    manifest_version: u32,
    tool_kind: CompactedToolKind,
    identity: ManifestIdentity,
    #[serde(skip_serializing_if = "Option::is_none")]
    range: Option<LineRange>,
    outcome: ToolResultOutcome,
    result_hash: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    excerpt: Option<String>,
}

#[derive(Debug, Serialize)]
struct ManifestIdentity {
    kind: ToolIdentityKind,
    value: String,
}

struct RenderedManifest {
    text: String,
    result_hash: String,
    excerpt_included: bool,
    redacted_identity: ToolIdentity,
}

/// A deterministic estimate of the request view's provider-visible messages,
/// using the same prose ratio as the conversation window's accounting.
///
/// The messages are measured through [`provider_messages_json`], which is the
/// array [`crate::model::model_request_json`] embeds. The result body is the only
/// part this reducer changes, but role, ids, arguments, reasoning, per-message
/// framing and array separators still count against the target.
fn messages_tokens(messages: &[ModelMessage]) -> usize {
    TokenEstimate::Prose.tokens(&provider_messages_json(messages))
}

fn message_tokens(message: &ModelMessage) -> usize {
    TokenEstimate::Prose.tokens(&provider_message_json(message))
}

fn bounded_excerpt(text: &str) -> String {
    if text.chars().count() <= MAX_MANIFEST_EXCERPT_CHARS {
        return text.to_string();
    }
    let mut excerpt: String = text.chars().take(MAX_MANIFEST_EXCERPT_CHARS - 1).collect();
    excerpt.push('…');
    excerpt
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bounded_excerpt_never_exceeds_the_fixed_bound() {
        let text = "é".repeat(MAX_MANIFEST_EXCERPT_CHARS + 10);
        let excerpt = bounded_excerpt(&text);
        assert_eq!(excerpt.chars().count(), MAX_MANIFEST_EXCERPT_CHARS);
        assert!(excerpt.ends_with('…'));
    }

    #[test]
    fn an_excerpt_at_the_bound_is_not_marked() {
        let text = "a".repeat(MAX_MANIFEST_EXCERPT_CHARS);
        assert_eq!(bounded_excerpt(&text), text);
    }
}
