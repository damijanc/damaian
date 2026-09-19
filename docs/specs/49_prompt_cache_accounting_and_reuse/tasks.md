# Prompt Cache Accounting Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Implements:** [`proposal.md`](proposal.md) §5.1's first slice · background and
corrections in [`context.md`](context.md)
**Started:** 2026-09-19 — **Done:** —

## Progress

| Task | State | Notes |
|---|---|---|
| 1 · `cached_input_tokens` on `TokenUsage` | Done | 3 tests, all failing to compile before the field existed. Six construction sites: `extract_usage`'s measured arm (task 2 fills it), the two `estimated_cost` calls in `chat.rs`, `recovery.rs` and `desktop-shell/src/lib.rs` (all four build a `TokenUsage` from a `TaskUsage`, so task 5 fills them), and `token_accounting.rs`'s `measured` helper. Each carries a comment naming the task that supplies the real value, so a `None` left behind is not mistaken for a decision. |
| 2 · Parse the split, normalised to a subset | Done | 5 tests (the plan's four plus `a_reported_cache_hit_of_zero_is_kept_as_a_measured_zero`, the other side of the silence-is-not-zero rule). Step 3's open question decided as the plan recommended: `extract_usage` now returns a named `ReportedUsage` rather than a fourth tuple element — four fields, two of them `Option`s of different meaning, is past what positional access reads safely. Alias list is exhaustive, not a prefix match: `prompt_tokens_details.cached_tokens` (OpenAI), `prompt_cache_hit_tokens` (DeepSeek), `cached_tokens`, `cache_read_input_tokens`. The miss count is read only to recognise the shape and never added to anything. Invariant mutation-tested: removing `<=` fails `a_cached_count_above_the_input_count_is_dropped_to_none`. |
| 3 · Capability detection per provider | Done | 4 tests. `reports_cache_split: Option<bool>` on `OpenAICompatibleAdapter`, set from the same place as `supports_usage`. Two deliberate differences from that field, both recorded in doc comments: it needs **no probe** (a split either is or is not in a usage object already parsed, whereas asking for usage is what a provider rejects), and the accessor returns `Option<bool>` rather than collapsing to a default — `probe_supports_usage` must answer before it knows because it gates an outgoing field, this one only describes what was seen. Made **monotone** (`true` sticks) after noticing an unqualified assignment lets a provider that omits the field on one call flip the surface from a hit rate to "not reported" and back. Not generalised into a shared type: the existing mechanism *is* `Option<bool>` plus a reader, and the two want opposite defaults. Both guards falsified — dropping the monotone term fails the later-call test, and keying on `self.provider.contains("deepseek")` fails the no-split test, which is why that test's provider is named `deepseek`. |
| 4 · The cached rate and the upper bound | Done | 6 tests (the plan's five plus `a_reported_cache_miss_is_not_an_upper_bound`). **Return shape decided:** `Option<CostEstimate>`, a struct with private fields, `amount()`/`is_upper_bound()`, no `Deref`, no `From<CostEstimate> for f64` and no public constructor taking a bare number — so reaching the figure means naming the method, and a reviewer can see every place that does. The label is enforced structurally at the one funnel: `task_usage_json` takes the whole `CostEstimate`, and all three shell call sites already went through it. `ChatTurnResult.estimated_cost` changed type with it. **Trust boundary confirmed, not assumed:** a `model_provider.<id>` entry from repository scope is rejected whole (`config.rs`), so the new key inherits Forbidden; pinned anyway in `repository_config_cannot_change_usage_reporting_or_prices`, because the class is per entry rather than per field. `push_model_provider_overlay`'s exhaustive destructuring caught the new key and forced the save path — the guard working as its comment says it should. Both guards falsified: collapsing `None` into `Some(0)` fails two tests, and returning `exact` instead of `upper_bound` fails the bound test. |
| 5 · Per-task aggregation | Done | 5 tests. `TaskUsage` gains three fields, not one: `cached_input_tokens` (sum over reporting runs, `None` when none did), `cache_reported_input_tokens` (**the denominator those runs are a rate of**) and `runs_without_cache_report`. The mixed case is why the denominator is separate — dividing a partial numerator by the whole task's input dilutes the rate with runs nobody can see into. The writer **omits** `cachedInputTokens` rather than writing a zero, so an event written today by a silent provider is byte-identical to a pre-field one; both are the same fact. Also wired the four `TaskUsage`→`TokenUsage` sites left as `None` in task 1, so the upper-bound label now propagates to a task's cost with no extra machinery. **Mutation found a fake test:** `one_run_without_a_split_does_not_erase_the_others` originally recorded the cache-reporting run first, where the buggy `= total.input_tokens` coincides with the right answer, so it passed against a broken implementation. Reordered so the non-reporting run comes first (1000 against 6000); the mutation now fails it. |
| 6 · Surfaces: shell and web UI | Not started | |
| 7 · The `cache_hit_rate` harness metric | Not started | |
| 8 · Prefix-stability guards | Not started | |
| 9 · Dormant explicit breakpoints | Not started | |
| 10 · Docs, acceptance criteria, close the slice | Not started | |

**Goal:** Record what a provider served from its prompt cache, price it
correctly where the user configured a cached rate and label it an upper bound
where they did not, detect the capability from what the provider reports, and
surface the per-task hit rate to both the user and the eval harness — without
changing a single request.

**Architecture:** One optional field on `TokenUsage`, normalised to a subset of
`input_tokens` at the parse boundary in `extract_usage`, carried through
`TaskUsage` by the existing `read_task_usage` aggregation, and rendered with an
upper-bound label wherever `estimated_cost` is rendered today. Capability
detection reuses the `supports_usage` pattern. The harness gains one metric read
through the same `read_task_usage`, so it cannot drift from the session surface.

**Tech Stack:** Rust 2024 (workspace edition), no new dependencies.

## Global Constraints

Every task's requirements implicitly include this section.

- **Read [`context.md`](context.md) §3 first.** Two of the flat spec's design
  statements do not survive contact with the code: requirement 5 is already
  satisfied (its test is a guard, not a fix), and requirement 4 is blocked on the
  eight-message window rather than being a small reordering. Do not "fix" a task
  back to the flat spec's wording.
- **This slice changes no request.** Task 9's flag is dormant and Task 8's tests
  assert what assembly already produces. If a task finds itself editing
  `build_model_prompt`, it has left the slice — stop and say so.
- **Silence is not zero.** `None` means the provider did not report a split.
  Every layer — parse, aggregate, render, metric — keeps that distinct from
  `Some(0)`. A surface that prints "0%" for an unreported split is a defect, not
  a rounding choice.
- **Never invent a cache number.** Requirement 3: there is no estimated cache
  hit. `TokenUsage::estimated` sets `None`, always.
- **One count.** `SessionStore::read_task_usage` stays the single definition of
  what a task spent. Do not add a second aggregation for cache figures.
- **Falsify every load-bearing test.** Break what it guards and confirm it
  fails. This repo has a documented history of tests that passed without testing
  anything — see [#47](../47_agent_working_capability/proposal.md) §7.2's
  `toolRounds`, and #21's metrics that read 0.000 by construction.
- **Scope the per-task checks; run the full seven-command gate from
  `AGENTS.md` once, at the end** (Task 10). `cargo nextest run --workspace`
  takes about 5 minutes locally and `cargo clippy --workspace --all-targets` up
  to 18 minutes cold.
- **Never `git commit` unasked.** Each task ends by showing the change and the
  scoped check result and asking. One subject line when asked, no body.

## File Structure

| File | Change |
|---|---|
| `crates/workspace-engine/src/model.rs` | `TokenUsage.cached_input_tokens`; `extract_usage` returns the split; `MockModelAdapter` usage constructors |
| `crates/workspace-engine/src/config.rs` | `price_per_million_cached_input_tokens`, `supports_explicit_cache_breakpoints`, `estimated_cost` split + upper-bound result |
| `crates/workspace-engine/src/session.rs` | `TaskUsage` cached totals; `read_task_usage` aggregation |
| `crates/desktop-shell/src/lib.rs` | The new fields in the task-usage JSON |
| `crates/desktop-shell/static/app.js` | Upper-bound label and hit rate / "not reported" |
| `crates/eval-harness/src/{metrics,record}.rs` | `cache_hit_rate`, `notApplicable` when no provider reported a split |
| `crates/workspace-engine/tests/prompt_cache.rs` | New: the guard tests from Task 8 |
| `docs/USER_GUIDE.md`, `docs/TROUBLESHOOTING.md` | §5.7 of the flat spec's documentation obligation |

## Interface reference

Read before starting; each is load-bearing for a task below.

- `TokenUsage` (`model.rs:198`), `UsageSource` (`model.rs:172`).
- `extract_usage(raw) -> Option<(u64, u64, Option<f64>)>` (`model.rs:1407`).
- `MockModelAdapter` records `requests: Vec<ModelRequest>` (`model.rs:336`) —
  the public seam Task 8 asserts through, since `build_model_prompt` is private.
- `Config::estimated_cost` (`config.rs:909`).
- `TaskUsage` (`session.rs:283`), `SessionStore::read_task_usage`
  (`session.rs:812`), event type `task_usage_recorded`.
- `MetricSet::KEYS` (`metrics.rs:57`), currently 20 entries.
- `formatCost` (`app.js:3921`) already renders a sub-$0.0001 figure as
  `<$0.0001`; the upper-bound label composes with it.

---

## Task 1: `cached_input_tokens` on `TokenUsage`

**Requirements:** 1, 3. **Files:** `model.rs`.

- [x] **Step 1: Write the failing tests**
  - `an_estimated_usage_never_claims_a_cache_hit` — `TokenUsage::estimated(..)`
    has `cached_input_tokens: None`.
  - `a_measured_zero_reports_a_cached_zero` — `measured_zero()` is `Some(0)`,
    because no request was sent and "nothing was cached" is then a fact.
  - `a_usage_event_written_before_this_field_reads_back_as_none` — deserialize a
    `task_usage_recorded` payload with no cache field and assert `None`, not
    `Some(0)`. This is the migration criterion; write it now, not in Task 5.
- [x] **Step 2: Run them and confirm they fail to compile or assert**
- [x] **Step 3: Add the field** with `#[serde(default)]` and the doc comment
      from [`proposal.md`](proposal.md) §5.2 — the one that says what `None`
      means, because that distinction is the whole point of the `Option`.
- [x] **Step 4: Fix the construction sites** the compiler names.
- [x] **Step 5: Scoped checks** — `cargo nextest run -p workspace-engine -E
      'test(usage)'`, `cargo fmt`, `cargo clippy -p workspace-engine`.
- [x] **Step 6: Show the change and the check result, and ask before committing**

## Task 2: Parse the split, normalised to a subset

**Requirements:** 1. **Files:** `model.rs`.

Providers report in two shapes: a hit count alongside the total prompt count, and
hit-plus-miss counts that sum to the total. Both normalise to the subset form
here, at the parse boundary, so no later layer has to know which provider it is
reading.

- [x] **Step 1: Write the failing tests**
  - `a_hit_count_beside_a_total_parses_as_a_subset`.
  - `hit_and_miss_counts_that_sum_to_the_total_parse_as_a_subset`.
  - `a_cached_count_above_the_input_count_is_dropped_to_none` — the invariant.
    A provider reporting this is describing something this spec does not
    understand, and a number whose meaning is unknown is worse than no number.
  - `a_usage_object_with_no_cache_field_parses_as_none`.
- [x] **Step 2: Run to verify they fail**
- [x] **Step 3: Extend `extract_usage`** to return the cached figure. Prefer
      widening the return type into a small named struct over a fourth tuple
      element — the tuple is already at three and the call sites read better for
      it. Record in the Progress table if you decide otherwise and why.
- [x] **Step 4: Update the call sites** the compiler names.
- [x] **Step 5: Mutation-test the invariant** — remove the `<=` check and
      confirm `a_cached_count_above_the_input_count_is_dropped_to_none` fails.
- [x] **Step 6: Scoped checks, then show and ask**

## Task 3: Capability detection per provider

**Requirements:** 6. **Files:** `model.rs`.

- [x] **Step 1: Write the failing test** — after a call whose usage object
      carried no split, the adapter records that this provider does not report
      one; after a call that did, it records that it does. Never keyed on the
      provider's name, and never probed a second time.
- [x] **Step 2: Run to verify it fails**
- [x] **Step 3: Implement** on the `supports_usage` pattern — observed once,
      reused for the process. Read how `supports_usage` does it before writing a
      second mechanism; if the existing one generalises, generalise it.
- [x] **Step 4: Scoped checks, then show and ask**

## Task 4: The cached rate and the upper bound

**Requirements:** 2. **Files:** `config.rs`.

`estimated_cost` returns `Option<f64>` today. It now has to say whether the
figure is exact or an upper bound, so the return type changes — pick a shape
that makes the label impossible to drop on the way to a surface, and say in the
Progress table what you picked.

- [x] **Step 1: Write the failing tests**, one per row of
      [`proposal.md`](proposal.md) §5.3's table:
  - cached rate configured + split reported → two rates, exact.
  - cached rate unset + split reported → full rate on all input, **upper bound**.
  - no split reported → identical to today's figure, not an upper bound.
  - either rate missing → `None`, as today.
  - `a_cached_rate_alone_does_not_produce_a_cost` — #19's both-rates rule is
    softened for the label, never for the requirement that both base rates exist.
- [x] **Step 2: Run to verify they fail**
- [x] **Step 3: Add `price_per_million_cached_input_tokens`** to
      `ModelProviderConfig`, and classify it in
      [`../34_repository_config_trust_boundary.md`](../34_repository_config_trust_boundary.md)'s
      table in the same change. A `model_provider` key is **Forbidden** in
      repository scope; confirm the new key inherits that rather than assuming
      it, and add the trust test if it does not.
- [x] **Step 4: Implement the split**
- [x] **Step 5: Scoped checks, then show and ask**

**Deviation from "Files: `config.rs`":** changing the return type necessarily moved
`ChatTurnResult.estimated_cost` (`chat.rs`) and `task_usage_json`
(`desktop-shell/src/lib.rs`), which now also emits
`estimatedCostIsUpperBound`. Emitting the flag here rather than deferring it to
Task 6 avoids a window in which the JSON carries a ceiling presented as an
exact figure. Task 6 still owns rendering it.

## Task 5: Per-task aggregation

**Requirements:** 2, 8. **Files:** `session.rs`.

- [x] **Step 1: Write the failing tests**
  - `a_tasks_cached_tokens_are_the_sum_of_its_runs`.
  - `a_task_whose_runs_never_reported_a_split_reports_not_reported` — the total
    is `None`, never `Some(0)`.
  - `one_run_without_a_split_does_not_erase_the_others` — decide and pin the
    mixed case. Recommended: sum the runs that reported, and carry a count of
    those that did not, so the hit rate can say what it is a rate *of*. A silent
    sum over a partial denominator is the failure #19's `reported_cost` rule
    already guards against.
  - `a_tasks_cost_is_an_upper_bound_when_any_run_was` — the label propagates on
    the same "weakest term" principle as `UsageSource`.
- [x] **Step 2: Run to verify they fail**
- [x] **Step 3: Extend `TaskUsage` and `read_task_usage`**
- [x] **Step 4: Scoped checks, then show and ask**

## Task 6: Surfaces — shell and web UI

**Requirements:** 2, 8. **Files:** `desktop-shell/src/lib.rs`, `static/app.js`.

- [ ] **Step 1: Write the failing shell test** — the task-usage JSON carries the
      cached figure, the hit rate and the upper-bound flag, and omits or nulls
      them (never zeroes them) when nothing was reported.
- [ ] **Step 2: Implement the shell side**
- [ ] **Step 3: Render it** — the upper-bound label composes with `formatCost`
      (`<$0.0001` stays), and an unreported split renders as "cache usage not
      reported by this provider", never "0%".
- [ ] **Step 4: Verify in the browser** — the static assets are `include_str!`
      embedded, so rebuild and restart before checking, then drive `app.js` from
      the browser console.
- [ ] **Step 5: `node --check`, `npm run lint:web`, then show and ask**

## Task 7: The `cache_hit_rate` harness metric

**Requirements:** 8. **Files:** `eval-harness/src/{metrics,record}.rs`.

- [ ] **Step 1: Write the failing tests**
  - the metric appears in the output (`MetricSet::KEYS` 20 → 21; the existing
    `every_metric_in_the_spec_appears_in_the_output` will hold you to it).
  - it reads through `read_task_usage`, not a second count.
  - it reports `notApplicable` when no run reported a split — the shape the
    harness already has for a metric nothing supplied.
- [ ] **Step 2: Run to verify they fail**
- [ ] **Step 3: Implement**
- [ ] **Step 4: Do NOT regenerate `evals/baseline.json` yet** — it is a human
      review gate (#18 §5.7), and a regeneration that lands with unrelated drift
      is a regeneration nobody reads. Task 10 decides whether this slice is the
      right moment, and if it is, every number gets read before it is committed.
- [ ] **Step 5: Scoped checks, then show and ask**

## Task 8: Prefix-stability guards

**Requirements:** 5, and the assertion half of 4. **Files:**
`crates/workspace-engine/tests/prompt_cache.rs` (new).

These assert what assembly already produces. `context.md` §3.1 confirms the
system prompt is static, so **both tests should pass on the day they are
written** — if either fails, that is a finding and belongs in the Progress table
before anything is changed.

- [ ] **Step 1: Write `two_assemblies_of_an_unchanged_repository_are_identical`**
      — drive two turns through `MockModelAdapter` against the same fixture and
      compare `adapter.requests[0]` byte for byte through the context section.
- [ ] **Step 2: Write `no_clock_value_enters_the_stable_prefix`** — assemble two
      requests a measurable interval apart and assert the prefix is identical.
      This is the test that fails the day someone adds "Current time:" to the
      system prompt.
- [ ] **Step 3: Falsify both** — temporarily interpolate a timestamp into
      `system_prompt()` and confirm each fails. A guard that cannot fail is not
      a guard. Revert.
- [ ] **Step 4: Scoped checks, then show and ask**

## Task 9: Dormant explicit breakpoints

**Requirements:** 7. **Files:** `config.rs`, request assembly.

- [ ] **Step 1: Write the failing tests**
  - with the flag off, the serialized request body is **byte-identical** to
    today's. This is the criterion that keeps the slice honest about changing no
    request.
  - with it on, the body carries the marker.
- [ ] **Step 2: Run to verify they fail**
- [ ] **Step 3: Add `supports_explicit_cache_breakpoints: bool`** defaulting to
      false, and classify it in the trust-boundary table as in Task 4.
- [ ] **Step 4: Scoped checks, then show and ask**

## Task 10: Docs, acceptance criteria, close the slice

- [ ] **Step 1: `docs/USER_GUIDE.md`** — what the cached figure means, why a
      cost may be shown as an upper bound, and how to configure a cached rate.
- [ ] **Step 2: `docs/TROUBLESHOOTING.md`** — why a hit rate may be zero (a
      changed system prompt, a changed tool set, a compaction, or a provider
      that expired the prefix) and that none of those is a malfunction.
- [ ] **Step 3: Walk [`proposal.md`](proposal.md) §6's slice-1 list** and name
      the test that covers each. A criterion with no test is not met.
- [ ] **Step 4: Record §7's implementation notes** — which providers reported a
      split and in which shape, and the measured cost difference on a
      representative task before and after. **The accounting change alone may
      move the reported figure with no caching behaviour changing**, and that
      number is the slice's main result.
- [ ] **Step 5: Decide on `evals/baseline.json`** — regenerate only if this
      slice changed what a scenario measures, and read every number if you do.
      `latencyMedianMs`/`latencyP90Ms` churn on every run and mean nothing.
- [ ] **Step 6: Update the four places** — `proposal.md`'s `Status:` line, the
      `docs/specs/README.md` row, this file's Progress table, and this file's
      `**Done:**` header.
- [ ] **Step 7: Full quality gate** — all seven commands from `AGENTS.md`.
      Report the test count.
- [ ] **Step 8: Show the change and the gate result, and ask before committing**
