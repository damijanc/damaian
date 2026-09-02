# Feature Spec: Token and Cost Accounting

Status: Not started
Order: 19 of 19
Roadmap: `docs/ROADMAP/01_phase_1_trust_and_recovery.md`, Phase 1, Work
Package 6 (Must). That directory is local-only and not committed, so the
reference is a name rather than a link; this spec is self-contained.
Related spec sections: `ai_coding_assistant_specification.md` section 7.5 (model
adapter), section 12.1 (performance). Related implementation specs:
[`08_stop_and_progress.md`](08_stop_and_progress.md) (the turn lifecycle these
figures attach to),
[`17_durable_task_state_and_crash_recovery.md`](17_durable_task_state_and_crash_recovery.md)
(the session-log append rules and the lost-call case), and
[`18_local_evaluation_harness.md`](18_local_evaluation_harness.md) (consumes
these fields).

## 1. Motivation

Damaian spends the user's money and does not tell them how much.

A chat turn can make many model calls — the agent loop runs up to
`agent_max_tool_rounds` rounds, each with the assembled context resent — and the
only bound on any of it is a round count. A user who runs a long task on a
frontier model has no way to know afterwards whether it cost two cents or four
dollars, and no way to compare a cheap approach against an expensive one, because
nothing is recorded.

The gap is total rather than partial. `ModelRun`
(`crates/workspace-engine/src/model.rs:158-177`) has no usage fields at all, the
streaming parser extracts message content and discards everything else, and
`Task` (`crates/workspace-engine/src/session.rs:46-56`) has nowhere to put a
number. The only token figure in the codebase is a `payload.len() / 4` estimate
(`model.rs:209`) used for context budgeting, not reporting.

This work package only measures. The enforced per-task ceiling lands in Phase 2
alongside the plan machinery, because stopping a task cleanly needs it — but a
ceiling cannot be built on numbers that do not exist, and
[spec 18](18_local_evaluation_harness.md)'s metric set has four rows that stay
empty until this ships.

## 2. Current State

- **`ModelRun` carries no usage data**: `run_id`, provider, model, timestamps,
  content, `incomplete`, `retry_count`, `tool_calls`, `truncated`,
  `reasoning_content` (`model.rs:158-177`). No token counts, no cost.
- **The SSE parser discards everything but content.** `extract_model_tokens`
  (`model.rs:823`) walks `data:` lines, skips `[DONE]`, and passes each payload
  to `extract_content_values`. A provider's `usage` object in the final chunk is
  parsed for nothing.
- **The request body does not ask for usage.** `model_request_json`
  (`model.rs:756`) emits `model`, `messages`, `stream`, and optionally
  `temperature`, `max_tokens`, `reasoning_effort`, and `tools`. For
  OpenAI-compatible streaming APIs, usage is omitted from the stream unless
  `stream_options: {"include_usage": true}` is sent.
- **An input-side estimate already exists.** `ContextBundle.token_estimate`
  (`crates/workspace-engine/src/context_manager.rs:34`) accumulates the same
  `len / 4` approximation, and `chat.rs:775` and `edit.rs:258` already audit it
  as `tokenEstimate`.
- **Retries are counted but not costed.** `ModelRun.retry_count`
  (`model.rs:166`, set at `model.rs:675`) records attempts beyond the first. Each
  retry is a billed call.
- **`Task` has no usage fields** and is replayed from `task_created` and
  `task_status_updated` events rather than stored as a record
  (`session.rs:237-260`).
- **Work is bounded by rounds, not tokens**: `agent_max_tool_rounds`,
  `agent_tool_retry_limit`.
- **No pricing information exists anywhere** in config or code.

## 3. Requirements

1. Input tokens, output tokens, and provider-reported cost are recorded per task
   and stored alongside the task in `SessionStore`.
2. The stored totals are exposed through task state and the completion report.
3. The same fields feed the [spec 18](18_local_evaluation_harness.md) harness, so
   the metric set is populated by real sessions as well as eval runs.
4. Where a provider does not report usage, the figure is estimated and labelled
   as an estimate. **An estimate is never presented as measured.**
5. Every billed call counts, including retries and calls whose response was lost.
6. Accounting never changes what is sent to a provider in a way that breaks a
   provider that does not support the addition.
7. No API key, prompt text, or repository content is added to any usage record.

## 4. Non-goals

- Enforcing a token or cost ceiling, or stopping a task when one is exceeded.
  That is Phase 2 WP2, which uses these fields rather than adding a second
  accounting path.
- A live per-turn running display. Also Phase 2 WP2.
- A built-in price table for providers and models. Prices change, a stale table
  reports confident wrong numbers, and nothing in the repository can keep one
  current.
- Billing reconciliation against a provider's invoice.
- Cross-session or cross-repository spend reporting, budgets, or alerts.
- Tokenizer-accurate counting. An exact local tokenizer per model family is a
  dependency and a maintenance burden this does not need; where the provider
  reports usage, its number is authoritative, and where it does not, the figure
  is explicitly an estimate.
- Attributing cost to individual tool calls or context components.
- Observing provider capabilities to decide whether usage reporting is supported.
  That is Phase 1 WP3, which is not in this phase's minimum slice; §5.2 handles
  the unknown case without it.

## 5. Design

### 5.1 Usage on `ModelRun`

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum UsageSource {
    /// Reported by the provider for this call.
    Measured,
    /// Derived locally from payload size. Never presented as measured.
    Estimated,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TokenUsage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub source: UsageSource,
}
```

`ModelRun` gains `usage: TokenUsage` and `reported_cost: Option<f64>`.

`reported_cost` is `Option` because almost no provider reports cost in a chat
completion response — most report tokens only. `None` is the normal case and
means "this provider did not tell us", not "free". The UI says so rather than
rendering a zero.

`UsageSource` is per run, not per session, because one task can mix them: a
provider that reports usage on a completed call reports nothing for a call whose
stream was cut, and that run's figure is an estimate while its siblings are
measured.

### 5.2 Getting measured usage

Add `stream_options: {"include_usage": true}` to the request body in
`model_request_json` when `request.stream` is true, gated by a
`ModelProviderConfig` boolean so a provider that rejects the field can turn it
off:

```text
provider_reports_usage=true    # default true; set false for a provider that 400s
```

Requirement 6 needs more than a config escape hatch, because the default will be
wrong for someone. A request that fails with a 4xx whose body mentions
`stream_options` or `include_usage` is retried **once** without the field, and
that retry is not counted as a `retry_count` attempt — it is a capability probe,
not a failed call. The outcome is remembered for the process lifetime so the
probe happens once rather than on every turn, and an audit event records that the
provider does not support usage reporting so the user can set the flag
permanently.

Phase 1 WP3's capability profile is the durable home for this observation. This
spec deliberately does not depend on it — WP3 is Should-tier and outside the
minimum slice — but the probe result is shaped so WP3 can adopt it later instead
of rediscovering it.

Parsing: extend the streaming reader to look for a `usage` object on `data:`
lines rather than only content. Usage arrives on a final chunk whose `choices`
array is empty, which the current parser passes to `extract_content_values` and
gets nothing from — so the addition is a new branch, not a change to content
extraction. Read `prompt_tokens` and `completion_tokens`, and accept
`input_tokens` / `output_tokens` as aliases, since providers differ on naming.

### 5.3 Estimating when there is no measurement

When no `usage` object arrives, estimate with the mechanism already in the
codebase — `ModelAdapter::estimate_tokens`, `payload.len().div_ceil(4)`
(`model.rs:209`) — over the serialised request for input and the accumulated
response content for output, and set `source: Estimated`.

The estimate is deliberately the existing one rather than a better one. It is
already used for context budgeting, so a single approximation keeps the reported
number consistent with the number the context manager made decisions on. A more
accurate estimate that disagreed with the budgeting figure would produce two
different token counts for the same turn.

Requirement 4 is a display rule as much as a data rule: anywhere a figure is
shown, an estimated one is marked — `~12,400 tokens (estimated)` — and a task
mixing measured and estimated runs is marked estimated overall, because a total
is only as trustworthy as its weakest term.

### 5.4 Per-task aggregation

A task's usage is the sum over its runs. Following
[spec 17](17_durable_task_state_and_crash_recovery.md)'s rule that the session
log is append-only, usage is appended, never rewritten:

```json
{"seq":418,"eventType":"task_usage_recorded","taskId":"task_…",
 "runId":"modelrun_…","inputTokens":11902,"outputTokens":812,
 "source":"measured","reportedCost":null}
```

One event per run. The task total is the sum of its events, computed the same way
`read_task_statuses` replays status (`session.rs:237`) — which means a crash
mid-task loses at most the usage of the run that was in flight, and the partial
total is still correct for the runs that completed.

`SessionStore` gains `read_task_usage(session_id) -> HashMap<String, TaskUsage>`,
where `TaskUsage` carries the summed tokens, the cost sum when every contributing
run reported one, the run count, and the aggregate `source` — `Estimated` if any
contributing run was estimated.

Requirement 1 says "stored alongside the task". `Task` is not a stored record, so
adding fields to the struct would store nothing; the event log is where task
facts live, and this follows that. `Task` gains no new fields, which also means
no migration: a session written before this change simply has no usage events and
reports zero runs, distinguishable from a task that genuinely used nothing by the
absence of any event rather than by a zero.

### 5.5 Lost and retried calls

Requirement 5 is where honest accounting differs from convenient accounting.

- **Retries**: `retry_count` (`model.rs:675`) means the provider was called more
  than once. Each attempt that reached the provider gets its own usage event.
  When a failed attempt returns no usage — the common case — its tokens are
  estimated from the request that was sent, since the input was transmitted and
  billed even though no answer came back.
- **Cancelled turns**: a turn stopped by the user
  ([spec 08](08_stop_and_progress.md)) has already sent its input and received
  partial output. Record what was streamed, estimated, rather than discarding the
  call. `ModelRun::cancelled_before_start` (`model.rs:180-183`) is the one case
  with genuinely zero usage, and it is recorded as measured zero.
- **Crashed turns**: a call in flight when the app died was billed and its
  response is gone. [Spec 17](17_durable_task_state_and_crash_recovery.md)
  classifies the task; recovery appends a usage event estimated from the request,
  marked `source: "estimated"` with the reason, so a crash does not silently
  reduce the reported spend.

The rule behind all three: a call that reached the provider is counted. Under-
reporting is the failure mode to avoid, because it makes Damaian look cheaper
than it is.

### 5.6 Surfaces

- **Completion report**: tokens in, tokens out, run count, cost when reported,
  with the estimated marker where it applies.
- **Task state**: the same figures on the task, so a reloaded conversation shows
  what each past turn used.
- **Audit log**: a `task_usage_recorded` event through `AuditLog::record`. Numbers
  and ids only — requirement 7 is satisfied by construction, since no prompt or
  file content enters the record.
- **Eval harness**: `read_task_usage` is what
  [spec 18](18_local_evaluation_harness.md)'s `tokens` and `cost` fields read,
  and `measured: false` there is this spec's `UsageSource::Estimated`.

Optional cost estimation from user-configured rates
(`price_per_million_input_tokens`, `price_per_million_output_tokens` per
provider) is **in scope only as configuration the user opts into**. When unset,
cost is `None` and nothing is displayed. When set, the computed figure is labelled
estimated and attributed to the user's own rates — never to the provider. This is
how requirement 1's "cost" becomes useful for the majority of providers that
report only tokens, without shipping a price table this repository cannot keep
current.

### 5.7 Documentation

`docs/USER_GUIDE.md`: where to see what a turn used, what "estimated" means and
why a figure may be estimated, and how to set price rates for a cost figure.
`docs/TROUBLESHOOTING.md`: why usage may be missing for a provider, what the
`stream_options` probe does, and how to set `provider_reports_usage=false`.

## 6. Acceptance Criteria

- A completed task records input tokens, output tokens, and run count, and
  reports cost when the provider supplies it.
- A provider that reports usage produces `source: "measured"`; one that does not
  produces `source: "estimated"`, and the difference is visible wherever a figure
  is shown.
- A task mixing measured and estimated runs reports an estimated total.
- A provider that rejects `stream_options` is retried once without it, succeeds,
  is not counted as a `retry_count` attempt, and the unsupported observation is
  audited — asserted with `MockModelTransport::failing`.
- A retried call records usage for every attempt that reached the provider.
- A turn cancelled mid-stream records the partial usage, estimated; a turn
  cancelled before the provider was called records measured zero.
- A task interrupted by a crash has a usage event appended at recovery, marked
  estimated with a reason.
- Sessions written before this change load and report no usage runs, rather than
  a zero total.
- `read_task_usage` returns the same figures the eval harness reports.
- No API key, prompt text, or file content appears in any usage record or audit
  field — asserted by test.
- Cost is `None` and nothing is displayed when no rates are configured; a
  configured-rate figure is labelled estimated and attributed to the user's
  rates.
- The five quality-gate commands from `AGENTS.md` pass.

## 7. Implementation Notes

To be completed during implementation. Record:

- Which providers were tested for usage reporting, and whether
  `stream_options: {"include_usage": true}` was accepted.
- Measured versus estimated token counts for the same request against a provider
  that reports usage, so the accuracy of the `len / 4` estimate is known rather
  than assumed. If it is badly wrong, say so here — a later work package can
  improve it, but nobody should discover it by accident.
