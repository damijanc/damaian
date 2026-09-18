# Feature Spec: Prompt Cache Accounting and Reuse

Status: In progress. Split into a folder and planned on 2026-09-18. The
accounting slice — requirements 1, 2, 3, 6 and 8, plus requirement 5's guard —
is specified in [`tasks.md`](tasks.md) and is what gets built first. The reuse
slice — requirement 4 — is blocked on a decision this spec does not own; see
§5.8.
Order: 49 of 56
Plan: `docs/PLAN/01_phase_1_trust_and_recovery.md`, Phase 1, Work Package 8
(Should). That directory is local-only and not committed, so the reference is a
name rather than a link; this spec is self-contained. It is the **last
unstarted Phase 1 work package**.
Depends on: [#19](../19_token_and_cost_accounting/proposal.md) (the `TokenUsage`
it extends) — built; Phase 1 WP3 (provider capability validation) —
**unspecified**. Everything else named below is a cross-reference, not a
prerequisite.
Background and the corrections found while planning:
[`context.md`](context.md).
Related implementation specs:
[`19_token_and_cost_accounting/proposal.md`](../19_token_and_cost_accounting/proposal.md)
(owns `TokenUsage`, the measured/estimated distinction and `estimated_cost`;
this spec extends it additively and changes none of its rules),
[`18_local_evaluation_harness/proposal.md`](../18_local_evaluation_harness/proposal.md)
(gains one metric), [`26_context_assembly.md`](../26_context_assembly.md) (owns
the ordering this spec depends on),
[`55_conversation_compaction.md`](../55_conversation_compaction.md) (owns the
eight-message window §5.8 is blocked on, and recorded the ordering problem
first),
[`21_task_plan_progress_and_budget/proposal.md`](../21_task_plan_progress_and_budget/proposal.md)
(owns the per-turn ceiling the saved tokens are counted against), and
[`48_provider_limits_and_backpressure/proposal.md`](../48_provider_limits_and_backpressure/proposal.md)
(sibling work package reading the same response metadata).

## 1. Motivation

See [`context.md`](context.md) §1. In one line: `estimated_cost` bills every
input token at the uncached rate, so for a provider that caches automatically
and reports the split, the figures #19 made honest are already overstated, and
the overstatement grows with the conversation.

## 2. Current State

See [`context.md`](context.md) §2, where every claim is checked against the code
with file and line.

## 3. Requirements

Unchanged from the flat spec. What changed is which slice each lands in.

1. `TokenUsage` records how many of its input tokens the provider served from
   cache, distinguishing "the provider did not say" from "none were cached".
2. Cost accounts for cached tokens at their own rate where the user has
   configured one. Where cached tokens are reported and no cached rate is
   configured, the figure is computed at the full input rate and labelled an
   upper bound — never presented as exact.
3. Cache hit counts are **measured only**. There is no estimated cache hit; an
   estimate that invented one would be #19's cardinal error in a new place.
4. The stable part of a request — system prompt, tool definitions, repository
   context — is assembled in a deterministic order and placed ahead of the
   volatile part, so a cache has something to hit.
5. Nothing that changes every request may appear in the stable prefix. A clock
   value, a run id or a round counter in the system prompt makes every request a
   miss.
6. Cache support is detected per provider from what it reports, never assumed
   from its name, and recorded the way #19 records usage support.
7. Where a provider requires explicit cache breakpoints rather than caching
   automatically, the capability is declared and off by default, and adding it
   changes no behaviour for providers that cache automatically.
8. Per-task cache hit rate is visible to the user and emitted by the
   [#18](../18_local_evaluation_harness/proposal.md) harness as a metric.

## 4. Non-goals

- **Caching responses locally.** A local response cache answers a question with
  a previous answer — a different feature with a different risk (a stale answer
  about changed code), and this spec does not open it.
- **Guaranteeing cache hits.** Providers expire cached prefixes on their own
  schedule. This spec makes hits possible and measures them; no acceptance
  criterion asserts a hit rate.
- **A built-in cache price table.** #19 §4's reasoning is unchanged.
- **Restructuring context assembly.** #26 owns that.
- **Replacing the eight-message conversation window.** #55 owns that; §5.8 says
  why this spec cannot finish requirement 4 without it.
- **Cross-session or on-disk caching of prompts.** The cache lives at the
  provider.
- **Changing the estimate for providers that report no usage at all.**

## 5. Design

### 5.1 The two slices

**Slice 1 — accounting.** Requirements 1, 2, 3, 6 and 8, plus requirement 5's
guard test and requirement 7's dormant flag. Additive and self-contained: it
touches the parse boundary, the cost function, the per-task aggregation, the
surfaces that render a figure, and the harness. It changes no request and no
assembly, so it cannot regress a turn. It fixes a number that is wrong today,
and it produces the before-measurement §7 asks for.

**Slice 2 — reuse.** Requirement 4. Blocked; see §5.8.

The slices are in this order because the reverse cannot be checked. Changing
assembly first would mean reordering a prefix with no measurement of what the
current order achieves — which is the mistake
[#47](../47_agent_working_capability/proposal.md) §7.2 records the cost of.

### 5.2 One field, and why it is an `Option`

```rust
pub struct TokenUsage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    /// How many of `input_tokens` the provider served from its prompt cache.
    /// A subset of `input_tokens`, never additional to it.
    ///
    /// `None` means the provider did not report a cache split — not that
    /// nothing was cached. `Some(0)` means it reported a split and none hit.
    /// Collapsing the two would turn silence into a measured zero, which is
    /// the error #19 exists to prevent.
    pub cached_input_tokens: Option<u64>,
    pub source: UsageSource,
}
```

`cached_input_tokens` is a **subset** of `input_tokens`. Providers differ —
some report a total prompt count alongside a hit count, others report hit and
miss counts that sum to the total — so `extract_usage` normalises to the subset
form at the parse boundary, where the invariant
`cached_input_tokens <= input_tokens` is asserted. A provider that violates it
is reporting something this spec does not understand, and the value is dropped
to `None` rather than recorded as a number whose meaning is unknown.

`TokenUsage::estimated` sets `None`: there is no way to estimate a cache hit
from payload size, and requirement 3 forbids inventing one.
`TokenUsage::measured_zero` sets `Some(0)`, because no request was sent and
"nothing was cached" is then as much a fact as "nothing was billed".

**Migration.** `TokenUsage` is serialised into the session log. The new field is
`#[serde(default)]`, so every existing event reads back as `None` — correct,
since those calls predate any cache measurement — and no log is rewritten. The
same read-side default [#17](../17_durable_task_state_and_crash_recovery/proposal.md)
established for added fields.

### 5.3 Cost

`ModelProviderConfig` gains `price_per_million_cached_input_tokens:
Option<f64>`, and `Config::estimated_cost` becomes:

| Configured | Cache reported | Result |
|---|---|---|
| Input + output rates, cached rate set | Yes | `(input − cached) × input_rate + cached × cached_rate + output × output_rate`, exact within #19's existing labelling |
| Input + output rates, cached rate unset | Yes | Full input rate on all input tokens, **labelled an upper bound** |
| Input + output rates | No (`None`) | Exactly as today |
| Either rate missing | Either | `None`, as today |

The upper-bound row is a deliberate softening of #19's "both rates or nothing"
rule, and the reason it is safe is the direction of the error: without a cached
rate the figure can only be too high, and a cost shown as "at most $0.04" is
useful where a silent blank is not. A figure that could be too *low* would get
#19's treatment instead, which is to show nothing.

The label travels with the number to every surface that renders it, the same way
`UsageSource` does. #19's `toFixed(4)` finding applies unchanged: a sub-$0.0001
figure must not render as `$0.0000`, and `formatCost` in `app.js` already
handles that — the upper-bound label composes with it rather than replacing it.

### 5.4 Capability detection

`extract_usage` reports whether the usage object carried a cache split.
`OpenAICompatibleAdapter` records the answer per provider, exactly as
`supports_usage` is recorded: observed once, reused for the process, never
inferred from the provider's name. A provider that reports no split is not
retried, probed or nagged — its `cached_input_tokens` stays `None` and every
surface says "not reported" rather than "0".

### 5.5 Explicit breakpoints, declared and dormant

`ModelProviderConfig` gains `supports_explicit_cache_breakpoints: bool`,
defaulting to false, and the request emits a marker only when it is true.
Declared now because declaring it costs nothing and adding it later is a
config-schema change across persisted provider settings — the same argument
[#22](../22_findings_model_and_panel.md) makes for declaring
`FindingSource::LanguageServer` early. No configured provider requires it, so
nothing turns it on.

### 5.6 Reporting

Per task, alongside #19's existing totals: cached input tokens, and the hit rate
as a percentage of input tokens over the task. Where every run reported `None`,
the surface says the provider does not report cache usage — **never 0%**, which
reads as "caching is broken" when the truth is "we cannot see it".

#18's metric set gains `cache_hit_rate`, sourced from the same
`read_task_usage` the session surface reads, so the two cannot drift — the
property #19 established and this spec inherits rather than re-derives. It
reports `notApplicable` for providers that report no split, a shape the harness
already has.

### 5.7 What makes a prefix stable

A prompt cache matches on an exact prefix, so request assembly should order the
body from least to most volatile:

1. System prompt — constant for the session.
2. Tool definitions — constant unless the enabled tool set changes.
3. Repository context items — stable within a turn, changing between turns.
4. Conversation messages — append-only.
5. Plan and progress text, and the current round's tool results — volatile.

Two obligations fall out, and **both are testable in slice 1 without touching
assembly**, because both are assertions about what assembly already produces:

- **Deterministic ordering of context items.** Two assemblies over an unchanged
  repository with unchanged inputs must produce byte-identical context. Nothing
  asserts this today. #26 decides the ordering; this spec contributes the
  assertion.
- **No clock in the prefix.** A timestamp, a run id, a session id or a round
  counter anywhere in sections 1–3 makes every request a miss and the feature
  silently does nothing. `context.md` §3.1 confirms the system prompt is clean
  today, so this test passes on the day it is written and exists to fail the day
  someone adds "Current time:" to it.

### 5.8 Why requirement 4 is a second slice, and what it is blocked on

`build_model_prompt` concatenates conversation, then user request, then
repository context into one `ModelMessage::user` — the reverse of §5.7, as
`context.md` §3.2 shows and #55 §2 recorded first. Reordering that string is a
small edit.

The blocker is what precedes it. The conversation section is a **sliding window
of the last eight messages** (`context.md` §3.3): past the eighth message every
turn drops the oldest, so that section changes at its first byte on every turn.
Ordering the sections moves the instability later in the prefix; it does not
remove it. A prefix is only worth caching up to its first differing token, so
the value of slice 2 depends entirely on what replaces the window — which is
#55's compaction, or #26's assembly, and not this spec.

Requirement 4 therefore waits on that decision. What slice 1 contributes to it
is the measurement: with accounting in place, the hit rate under today's order
is a number, and the case for reordering can be made from it rather than from
first principles.

## 6. Acceptance Criteria

### Slice 1 — accounting

- A usage object reporting a cache split parses into `cached_input_tokens` as a
  subset of `input_tokens`, for both the hit-count and hit-plus-miss reporting
  shapes.
- A usage object reporting a cached count greater than the input count yields
  `None`, not a recorded value — asserted by test.
- A provider reporting no split yields `None`, and every surface renders "not
  reported" rather than zero.
- An estimated `TokenUsage` always has `cached_input_tokens: None`.
- Cost with a configured cached rate splits the input tokens across two rates;
  cost without one is computed at the full rate and carries the upper-bound
  label to every surface that renders it.
- Existing session-log events without the field read back as `None` and no log
  is rewritten.
- Per-task cache hit rate is reported, and the harness emits `cache_hit_rate`
  read through `read_task_usage`.
- Two context assemblies over an unchanged repository produce byte-identical
  request prefixes through the context section.
- Two requests assembled a measurable interval apart have identical prefixes —
  the test that fails if a clock value enters the system prompt.
- Turning `supports_explicit_cache_breakpoints` on changes the request body;
  leaving it off produces a body byte-identical to today's.

### Slice 2 — reuse

- The request body orders system prompt, tool definitions and repository context
  ahead of conversation and volatile content.
- The measured hit rate over a representative multi-round task is recorded
  before and after the reordering, against the same provider.

### Both

- Every quality-gate command from `AGENTS.md` passes.

## 7. Implementation Notes

To be completed during implementation. Record:

- Which configured providers reported a cache split, in which shape, and the
  measured hit rate over a representative multi-round task. This is the number
  that says whether §5.7's ordering work would pay for itself, and it is the
  reason slice 1 comes first.
- Whether anything in the stable prefix had to be moved or removed to make hits
  possible, and what it was. A finding here is worth more than the feature.
- The measured cost difference on a representative task before and after, since
  the accounting change alone may move the reported figure without any caching
  behaviour changing at all.
