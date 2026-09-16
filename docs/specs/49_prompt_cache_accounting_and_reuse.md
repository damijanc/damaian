# Feature Spec: Prompt Cache Accounting and Reuse

Status: Not started
Order: 49 of 53
Plan: `docs/PLAN/01_phase_1_trust_and_recovery.md`, Phase 1, Work
Package 8 (Should). That directory is local-only and not committed, so the
reference is a name rather than a link; this spec is self-contained.
Depends on: [#19](19_token_and_cost_accounting/proposal.md) (the TokenUsage it
extends) — built; Phase 1 WP3 (provider capability validation) —
**unspecified**. Everything else named below is a cross-reference, not a
prerequisite.
Related implementation specs:
[`19_token_and_cost_accounting/proposal.md`](19_token_and_cost_accounting/proposal.md)
(owns `TokenUsage`, the measured/estimated distinction and `estimated_cost`;
this spec extends it additively and changes none of its rules),
[`18_local_evaluation_harness/proposal.md`](18_local_evaluation_harness/proposal.md)
(gains one metric), [`26_context_assembly.md`](26_context_assembly.md) (owns the
ordering this spec depends on),
[`21_task_plan_progress_and_budget/proposal.md`](21_task_plan_progress_and_budget/proposal.md)
(owns the per-turn ceiling the saved tokens are counted against), and
[`48_provider_limits_and_backpressure/proposal.md`](48_provider_limits_and_backpressure/proposal.md)
(sibling work package reading the same response metadata).

## 1. Motivation

**This is not a future problem. The cost figures Damaian reports today are
already wrong for at least one configured provider.**

Providers that cache prompt prefixes bill a cache hit at a fraction of an
uncached input token — commonly around a tenth — and report the split in the
usage object. `Config::estimated_cost` applies one rate to every input token:

```rust
Some(
    (usage.input_tokens as f64 / 1_000_000.0) * input_rate
        + (usage.output_tokens as f64 / 1_000_000.0) * output_rate,
)
```

For a provider whose caching is automatic and whose usage object separates hits
from misses, that overstates the bill on every round after the first — and the
overstatement grows with the conversation, because the cached prefix is the part
that grows. The measured figure spec 19 worked to make honest is measured on one
axis and assumed on another.

The second half is the opportunity. An agentic turn re-sends a growing prefix —
system prompt, tool definitions, repository context, prior messages — on every
round of the tool loop. That prefix is the largest and most repetitive part of
the request, and it is exactly what a prompt cache is for. Nothing in the
current pipeline is designed to keep it stable, so any hit Damaian gets today is
an accident, and any change that reorders context silently ends it.

The roadmap calls model routing "the single largest cost lever". On a multi-round
tool loop, prefix reuse is the same order of magnitude, and unlike routing it
changes no answer: the same model sees the same tokens and returns the same
result, for less.

## 2. Current State

- **`TokenUsage` has three fields** — `input_tokens`, `output_tokens`, `source`
  — and no notion of a cached subset (`crates/workspace-engine/src/model.rs`).
- **`extract_usage(raw) -> Option<(u64, u64, Option<f64>)>`** pulls prompt
  tokens, completion tokens and cost. The cache fields providers report
  alongside them are parsed by nothing.
- **`ModelProviderConfig` carries two rates**,
  `price_per_million_input_tokens` and `price_per_million_output_tokens`, both
  `Option<f64>`, with spec 19's rule that both must be set or no cost is shown.
- **There is no built-in price table**, deliberately (spec 19 §4). This spec does
  not add one.
- **The usage probe is the precedent for capability detection.**
  `OpenAICompatibleAdapter` sends once asking for usage, once without if the
  provider rejects the field, and sets `supports_usage` from what happened —
  never from a list of provider names.
- **Request assembly is per round.** `model_request_json(&request)` rebuilds the
  whole body each time from the current `ModelRequest`; no part of it is
  constructed once and held.
- **Context is reassembled per turn.** `ContextManager` produces a `ContextPlan`
  whose `items: Vec<ContextItem>` are packed against
  `Config::context_token_budget`. Ordering is whatever the assembly produced;
  nothing declares it stable, and [spec 26](26_context_assembly.md) is the spec
  that will replace the current first-come-first-served packing.

## 3. Requirements

1. `TokenUsage` records how many of its input tokens the provider served from
   cache, distinguishing "the provider did not say" from "none were cached".
2. Cost accounts for cached tokens at their own rate where the user has
   configured one. Where cached tokens are reported and no cached rate is
   configured, the figure is computed at the full input rate and labelled an
   upper bound — never presented as exact.
3. Cache hit counts are **measured only**. There is no estimated cache hit; an
   estimate that invented one would be spec 19's cardinal error in a new place.
4. The stable part of a request — system prompt, tool definitions, repository
   context — is assembled in a deterministic order and placed ahead of the
   volatile part, so a cache has something to hit.
5. Nothing that changes every request may appear in the stable prefix. A clock
   value, a run id or a round counter in the system prompt makes every request a
   miss.
6. Cache support is detected per provider from what it reports, never assumed
   from its name, and recorded the way spec 19 records usage support.
7. Where a provider requires explicit cache breakpoints rather than caching
   automatically, the capability is declared and off by default, and adding it
   changes no behaviour for providers that cache automatically.
8. Per-task cache hit rate is visible to the user and emitted by the
   [spec 18](18_local_evaluation_harness/proposal.md) harness as a metric.

## 4. Non-goals

- **Caching responses locally.** A local response cache answers a question with
  a previous answer. That is a different feature with a different risk — a stale
  answer about changed code — and this spec does not open it.
- **Guaranteeing cache hits.** Providers expire cached prefixes on their own
  schedule. This spec makes hits possible and measures them; it does not promise
  them, and no acceptance criterion below asserts a hit rate.
- **A built-in cache price table.** Spec 19 §4 rules out a price table and the
  reasoning is unchanged: rates move, and a stale table reports confident wrong
  numbers.
- **Restructuring context assembly.** [Spec 26](26_context_assembly.md) owns
  that. Requirement 4 asks it for a deterministic order — an obligation, not a
  redesign.
- **Cross-session or on-disk caching of prompts.** The cache lives at the
  provider.
- **Changing the estimate for providers that report no usage at all.** Those
  stay exactly as spec 19 left them.

## 5. Design

### 5.1 One field, and why it is an `Option`

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
    /// the error spec 19 exists to prevent.
    pub cached_input_tokens: Option<u64>,
    pub source: UsageSource,
}
```

`cached_input_tokens` is a **subset** of `input_tokens`, not an addition to it.
Providers differ here — some report a total prompt count alongside a hit count,
others report hit and miss counts that sum to the total — so `extract_usage`
normalises to the subset form at the parse boundary, and the invariant
`cached_input_tokens <= input_tokens` is asserted there. A provider that
violates it is reporting something this spec does not understand, and the value
is dropped to `None` rather than recorded as a number whose meaning is unknown.

`TokenUsage::estimated` sets `None`: there is no way to estimate a cache hit
from payload size, and requirement 3 forbids inventing one.
`TokenUsage::measured_zero` sets `Some(0)`, because no request was sent and
"nothing was cached" is then as much a fact as "nothing was billed".

**Migration.** `TokenUsage` is serialised into the session log. The new field is
`#[serde(default)]`, so every existing event reads back as `None` — correct,
since those calls predate any cache measurement — and no log is rewritten. This
is the same read-side default [spec 17](17_durable_task_state_and_crash_recovery/proposal.md)
established for added fields.

### 5.2 Cost

`ModelProviderConfig` gains `price_per_million_cached_input_tokens:
Option<f64>`, and `Config::estimated_cost` becomes:

| Configured | Cache reported | Result |
|---|---|---|
| Input + output rates, cached rate set | Yes | `(input − cached) × input_rate + cached × cached_rate + output × output_rate`, exact within spec 19's existing labelling |
| Input + output rates, cached rate unset | Yes | Full input rate on all input tokens, **labelled an upper bound** |
| Input + output rates | No (`None`) | Exactly as today |
| Either rate missing | Either | `None`, as today |

The upper-bound row is a deliberate softening of spec 19's "both rates or
nothing" rule, and the reason it is safe is the direction of the error: without
a cached rate the figure can only be too high, and a cost shown as "at most
$0.04" is useful where a silent blank is not. A figure that could be too *low*
would get spec 19's treatment instead, which is to show nothing.

The label travels with the number to every surface that renders it, the same way
`UsageSource` does. Spec 19's `toFixed(4)` finding applies unchanged: a
sub-$0.0001 figure must not render as `$0.0000`.

### 5.3 What makes a prefix stable

A prompt cache matches on an exact prefix. Everything before the first differing
token is reusable; everything after it is not. So request assembly orders the
body from least to most volatile:

1. System prompt — constant for the session.
2. Tool definitions — constant unless the enabled tool set changes.
3. Repository context items — stable within a turn, changing between turns.
4. Conversation messages — append-only.
5. Plan and progress text, and the current round's tool results — volatile.

Two obligations fall out, and both are testable:

- **Deterministic ordering of context items.** Two assemblies over an unchanged
  repository with unchanged inputs must produce byte-identical context. Today
  nothing asserts this. [Spec 26](26_context_assembly.md) is where the ordering
  is decided; this spec's contribution is the assertion, which belongs in the
  test suite whether or not spec 26 has landed.
- **No clock in the prefix.** A timestamp, a run id, a session id or a round
  counter anywhere in sections 1–3 makes every request a miss and the feature
  silently does nothing. This is checked by a test that assembles two requests a
  measurable interval apart and asserts the prefix through section 3 is
  identical — a test that fails loudly the day someone adds "Current time:" to
  the system prompt.

Appending rather than rewriting matters for the same reason: a turn that
re-summarises earlier messages in place invalidates the whole prefix. That is a
constraint [spec 26](26_context_assembly.md) and Phase 3 WP6 compaction should
know about — compaction necessarily breaks the cache, which is a reason to
compact on a threshold rather than continuously, not a reason to avoid it.

### 5.4 Capability detection

`extract_usage` reports whether the usage object carried a cache split.
`OpenAICompatibleAdapter` records the answer per provider, exactly as
`supports_usage` is recorded: observed once, reused for the process, never
inferred from the provider's name. A provider that reports no split is not
retried, probed, or nagged — its `cached_input_tokens` is simply `None` forever
and every surface says "not reported" rather than "0".

### 5.5 Explicit breakpoints, declared and dormant

Some providers cache only where the request marks a breakpoint. For those,
`ModelProviderConfig` gains `supports_explicit_cache_breakpoints: bool`,
defaulting to false, and `model_request_json` emits the marker on the last
message of section 3 only when it is true.

It is declared now and left off because declaring it costs nothing and adding it
later is a config-schema change across persisted provider settings — the same
argument [spec 22](22_findings_model_and_panel.md) makes for declaring
`FindingSource::LanguageServer` before Phase 3 needs it. No provider currently
configured in this repository requires it, so nothing turns it on and no
acceptance criterion below depends on it.

### 5.6 Reporting

Per task, alongside spec 19's existing totals: cached input tokens, and the hit
rate as a percentage of input tokens over the task. Where every run reported
`None`, the surface says the provider does not report cache usage — never 0%,
which reads as "caching is broken" when the truth is "we cannot see it".

[Spec 18](18_local_evaluation_harness/proposal.md)'s metric set gains
`cache_hit_rate`, sourced from the same `read_task_usage` the session surface
reads, so the two cannot drift — the property spec 19 established and this spec
inherits rather than re-derives. It reports `notApplicable` for providers that
report no split, which the harness already has a shape for.

### 5.7 Documentation

`docs/USER_GUIDE.md`: what the cached figure means, why cost may be shown as an
upper bound, and how to configure a cached rate. `docs/TROUBLESHOOTING.md`: why
a hit rate may be zero — a changed system prompt, a changed tool set, a
compaction, or a provider that expired the prefix — and that none of those is a
malfunction.

## 6. Acceptance Criteria

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
- Two context assemblies over an unchanged repository produce byte-identical
  request prefixes through the context section.
- Two requests assembled a measurable interval apart have identical prefixes —
  the test that fails if a clock value enters the system prompt.
- Per-task cache hit rate is reported, and the harness emits `cache_hit_rate`
  read through `read_task_usage`.
- Turning `supports_explicit_cache_breakpoints` on changes the request body;
  leaving it off produces a body byte-identical to today's.
- Every quality-gate command from `AGENTS.md` passes.

## 7. Implementation Notes

To be completed during implementation. Record:

- Which configured providers reported a cache split, in which shape, and the
  measured hit rate over a representative multi-round task. This is the number
  that says whether §5.3's ordering work paid for itself.
- Whether anything in the stable prefix had to be moved or removed to make hits
  possible, and what it was. A finding here is worth more than the feature.
- The measured cost difference on a representative task before and after, since
  the accounting change alone may move the reported figure without any caching
  behaviour changing at all.
