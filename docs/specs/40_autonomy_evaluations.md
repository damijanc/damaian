# Feature Spec: Autonomy Evaluations

Status: Not started
Order: 40 of 40
Roadmap: `docs/ROADMAP/06_phase_6_advanced_autonomy.md`, Phase 6, Work
Package 7 (Must). That directory is local-only and not committed, so the
reference is a name rather than a link; this spec is self-contained.
Related spec sections: `ai_coding_assistant_specification.md` section 19
(recommended technology direction). Related implementation specs:
[`18_local_evaluation_harness.md`](18_local_evaluation_harness.md) (the harness
this extends — this spec adds scenarios and metrics, it does not build a second
harness), [`19_token_and_cost_accounting.md`](19_token_and_cost_accounting.md)
(the usage figures amplification is computed from),
[`21_task_plan_progress_and_budget.md`](21_task_plan_progress_and_budget.md),
[`38_subagent_model.md`](38_subagent_model.md),
[`39_coordination_and_conflict_handling.md`](39_coordination_and_conflict_handling.md).

## 1. Motivation

This spec exists to answer one question: **is delegated work better than the
single-agent path, and at what cost?**

Without it, Phase 6 ships a more expensive and less observable execution mode on
faith. Multi-agent execution is unambiguously costlier — several agents, each
with its own assembled context and its own model calls — and the benefit is a
hypothesis. A hypothesis that is expensive to run and hard to observe is exactly
the kind that survives on impression rather than evidence.

The phase file says something no other phase says: the correct outcome may be
"measured, found not to help, documented, and abandoned", and reaching that
conclusion with evidence is a **success**. That reframes this work package. It is
not a reporting layer bolted onto a feature; it is the decision instrument, and
it has to be built such that a negative result is publishable rather than
awkward. §5.7 makes the abandonment path a first-class recorded outcome.

There is also a narrower obligation. [Spec 38](38_subagent_model.md) §5.4 stages
delivery: read-only subagents first, editing subagents only if stage 1 measures
at least as well as the baseline. That gate is meaningless unless something
measures it, and this spec is that something.

## 2. Current State

Nothing in this phase exists. What it builds on:

- **The harness exists**, from [spec 18](18_local_evaluation_harness.md): a
  workspace crate with a `damaian-eval` binary (§5.1 there), fixture
  repositories materialised into temporary directories with `DAMAIAN_DATA_DIR`
  isolation (§5.2), a deterministic tier driven by `MockModelAdapter` via
  scenario files (§5.3), thirteen scenarios (§5.4), a per-scenario run record
  (§5.5), full coverage of the roadmap's metric set (§5.6), and a committed
  reviewed baseline at `evals/baseline.json` (§5.7).
- **The deterministic tier runs inside `cargo test --workspace --locked`**
  ([spec 18](18_local_evaluation_harness.md) §5.8), adding no new quality-gate
  command.
- **Two metric rows were deferred and are now filled.**
  [Spec 18](18_local_evaluation_harness.md) §5.6 marks memory recall usefulness
  and memory correction rate `notApplicable: "phase-3b"`, and
  [spec 30](30_memory_retrieval_and_lifecycle.md) §5.8 supplies them. This spec
  adds rows rather than filling deferred ones.
- **Per-task usage aggregates upward.**
  [Spec 19](19_token_and_cost_accounting.md) §5.4 records usage per run and sums
  per task, and [spec 38](38_subagent_model.md) §5.10 makes a parent's total
  include its descendants' — which is what makes amplification computable
  without a second accounting path.
- **`MockModelAdapter` supports scripted multi-round conversations**
  (`model.rs:215`): ordered responses, per-response tool calls, truncation
  simulation, and a record of every request received.
- **The baseline is single-agent** by construction, since it was recorded before
  Phase 6 existed. That is the comparison target.

## 3. Requirements

Extend the [spec 18](18_local_evaluation_harness.md) harness to measure:
delegation correctness; redundant work; conflicting edits; parent integration
quality; permission propagation; cancellation and timeout behaviour; cost and
latency amplification; recovery after child failure; and remote/local result
equivalence where applicable.

1. Compare advanced execution against the recorded single-agent baseline on the
   same representative tasks.
2. Report **cost amplification** explicitly.
3. **Do not ship an autonomous path that is less reliable than the single-agent
   baseline without clearly labelling it experimental.**

## 4. Non-goals

- A second harness. This adds scenarios, metrics, and a comparison mode to the
  existing one.
- Model-graded quality scoring. Every assertion stays mechanical, per
  [spec 18](18_local_evaluation_harness.md) §4 — a model judging whether
  delegation produced a better answer is not evidence, particularly when it is
  the same model that did the delegating.
- Tuning delegation. This measures; changing prompts or agent kinds in response
  is separate work informed by the measurement.
- Remote/local equivalence as implemented behaviour. Phase 6 WP5 (Remote
  Sandbox) is Could-tier and unspecified, so §5.4 defines the metric and marks
  it not-applicable until that ships — the same treatment
  [spec 18](18_local_evaluation_harness.md) gave the memory rows.
- Benchmarking against other assistants.
- A dashboard. Machine-readable output plus a concise report, as before.

## 5. Design

### 5.1 A comparison mode, not a separate suite

The harness gains an execution-mode axis:

```sh
damaian-eval run --tier deterministic --execution single
damaian-eval run --tier deterministic --execution multi
damaian-eval compare --against evals/baseline.json
```

`--execution multi` runs **the same scenario definitions** with delegation
enabled. That is the design decision that makes requirement 1 meaningful: a
separate multi-agent scenario set would measure different tasks and the
comparison would be worthless. One definition, two execution modes, identical
assertions.

The deterministic tier scripts subagent conversations the same way it scripts
single-agent ones — `MockModelAdapter` already supports ordered responses with
per-response tool calls (`model.rs:215`), and a scenario file gains an optional
`[[agent]]` block declaring the children and their scripted turns. So the
multi-agent tier remains credential-free, network-free, and runnable inside
`cargo test --workspace --locked`.

### 5.2 Amplification is a ratio, reported as such

Requirement 2. For each scenario, run both modes and report:

```json
{
  "scenario": "multi_file_patch",
  "single": { "tokens": 41200, "durationMs": 18400, "status": "completed" },
  "multi":  { "tokens": 118900, "durationMs": 11200, "status": "completed" },
  "amplification": { "tokens": 2.89, "duration": 0.61, "modelCalls": 3.4 }
}
```

Ratios rather than absolute deltas, because the useful statement is "delegation
cost 2.9× the tokens to finish in 0.6× the time" — a trade a user can evaluate.
An absolute token delta means nothing without knowing the baseline.

Three properties this needs to be honest:

- **Children's usage is included in the parent's total.** Already true via
  [spec 19](19_token_and_cost_accounting.md) and
  [spec 38](38_subagent_model.md) §5.10. An amplification figure computed from
  the parent's own calls alone would understate the cost by most of it, which is
  the single easiest way to make delegation look free.
- **Estimated usage is marked.** Where a provider does not report usage,
  [spec 19](19_token_and_cost_accounting.md) §5.3 labels the figure estimated;
  an amplification ratio built from estimates is labelled the same way. A ratio
  of two estimates is not a measurement.
- **Deterministic-tier duration measures Damaian's own work only**, since the
  mock returns instantly — recorded as such, per
  [spec 18](18_local_evaluation_harness.md) §5.6's treatment of latency.
  Wall-clock amplification is a live-tier figure.

### 5.3 The new scenarios

Each is mechanical and asserts a property [spec 38](38_subagent_model.md) or
[spec 39](39_coordination_and_conflict_handling.md) claims:

| Scenario | Asserts |
|---|---|
| Delegation correctness | The parent delegates the parts its plan marked delegable, and the children's results cover them — no gap between what was delegated and what came back |
| Redundant work | Two exploration children given overlapping questions do not both read the same files; overlap above a threshold is reported as redundancy |
| Conflicting edits | Two Implementation children with overlapping scope are refused at spawn ([spec 39](39_coordination_and_conflict_handling.md) §5.1) |
| Integration quality | Serialized integration applies patches in deterministic order, revalidates hashes between, and reports a moved target as a conflict |
| Combined-check regression | One child renames a symbol, another references the old name, each child's checks pass, and the combined rerun **fails** ([spec 39](39_coordination_and_conflict_handling.md) §5.5) |
| Permission propagation | A profile deny propagates to a grandchild; a child requesting a tool the parent lacks is narrowed ([spec 38](38_subagent_model.md) §5.3) |
| Cancellation | Cancelling the parent cancels all descendants; cancelling one child leaves siblings running |
| Timeout | A child exceeding `max_duration_ms` terminates with a partial result and does not fail the parent |
| Recovery after child failure | A failed child releases its claim, siblings' completed work survives, and a patch already integrated is not rolled back automatically |
| Depth limit | Spawning beyond `agent_max_depth` is refused, reported as a `Finding`, and does not fail the requester |
| Child result is untrusted | A child result containing instruction-like text changes no policy, mode, or approval outcome |
| Remote/local equivalence | `notApplicable: "phase-6-wp5"` until Remote Sandbox ships |

The cancellation and permission-propagation scenarios matter most, because both
assert properties whose failure is silent. A deny that stopped propagating at
depth two would not produce a visible error — it would produce a grandchild
quietly doing something it should not.

### 5.4 New metric rows

Added to the metric set alongside the roadmap's Section 8.1 rows, which continue
to be emitted:

| Measure | Definition |
|---|---|
| Delegation correctness | Delegated plan steps whose child result covers them, over delegated steps |
| Redundant read overlap | Files read by more than one sibling, over total distinct files read |
| Conflicting-edit refusals | Spawn refusals due to overlapping scope. Expected non-zero in the conflict fixture, expected 0 elsewhere |
| Integration failure rate | Combined runs where the post-integration check set failed, over combined runs |
| Permission-propagation violations | Descendants holding a capability an ancestor denies. **Asserted 0** |
| Cancellation completeness | Descendants still running after parent cancellation. **Asserted 0** |
| Token amplification | Multi-agent tokens over single-agent tokens, per scenario and median |
| Latency amplification | Same for duration |
| Model-call amplification | Same for call count |
| Child-failure recovery | Runs where a child failure left siblings' completed work intact, over child-failure runs |
| Remote/local equivalence | `notApplicable: "phase-6-wp5"` |

Two rows are asserted zero rather than tracked — permission-propagation
violations and cancellation completeness — following the roadmap's pattern for
approval-policy and secret violations. A non-zero value is a test failure, not a
metric that trended badly.

### 5.5 The experimental label is derived, not chosen

Requirement 3 says not to ship a less reliable autonomous path without labelling
it experimental. A label someone remembers to apply is a label that eventually
goes stale, so it is computed:

```json
{
  "executionMode": "multi",
  "comparedAgainst": "evals/baseline.json",
  "verdict": "experimental",
  "reason": "task completion 0.86 vs baseline 0.91"
}
```

The harness emits a verdict per execution mode: `at_or_above_baseline` when task
completion and check pass rate are at least the baseline's and both
asserted-zero rows are zero, `experimental` otherwise. The application reads that
verdict and labels the multi-agent path in the UI accordingly.

So the UI label is a function of the last recorded comparison rather than a
developer's judgement, and a regression that pushes multi-agent below baseline
re-labels it automatically. The verdict file is committed alongside the baseline,
which also means the label is reviewable in a diff.

An execution mode with **no** recorded comparison is labelled experimental. The
default is the cautious one, so shipping delegation without measuring it is not
a path that quietly produces an unlabelled feature.

### 5.6 Baseline handling

The existing `evals/baseline.json` stays the single-agent reference and is **not
overwritten** by multi-agent runs. A multi-agent comparison writes
`evals/autonomy-comparison.json`, referencing the baseline it compared against by
version.

Keeping them separate matters: if multi-agent results were folded into the
baseline, the next comparison would measure against a number partly produced by
the thing being evaluated, and the single-agent reference would be lost. The
phase's abandonment option requires the pre-Phase-6 baseline to remain
intact and citable.

Both files are human-reviewed before commit, per
[spec 18](18_local_evaluation_harness.md) §5.7's rule that an unread baseline is
not a baseline.

### 5.7 Abandonment is a recorded outcome

The phase file's most unusual instruction, made concrete.

If the comparison shows multi-agent execution is not better — lower completion,
no latency benefit worth the cost, or integration failures at a meaningful rate —
the correct action is to record that and stop. So this spec defines what "stop"
produces:

- `evals/autonomy-comparison.json` committed with the measurements.
- A decision recorded in §7 of this spec and in the phase file: what was
  measured, against which baseline, and the conclusion.
- The multi-agent path either removed or left disabled and labelled
  experimental — **not** left enabled by default because the code exists.
- [Spec 38](38_subagent_model.md) §5.4's stage 2 not started, or reverted.

Because [spec 38](38_subagent_model.md) requires that disabling subagents leaves
every single-agent capability working, abandonment is a configuration change and
a documentation change rather than an extraction. That property was specified for
this purpose, and it is what makes a negative result cheap to act on.

### 5.8 Documentation

`docs/DEVELOPMENT.md`: how to run each execution mode, how to produce and review
a comparison, and how the experimental verdict is derived.
`docs/USER_GUIDE.md`: what the experimental label on the multi-agent path means
and what it is derived from.

## 6. Acceptance Criteria

- Multi-agent and single-agent runs execute the **same** scenario definitions
  with the same assertions, differing only in execution mode.
- The multi-agent deterministic tier runs with no credentials and no network,
  inside the existing `cargo test --workspace --locked`.
- Cost, latency, and model-call amplification are reported as ratios per
  scenario and as medians.
- Amplification includes children's usage in the parent's total — asserted by a
  test that a parent's reported tokens exceed its own calls' tokens when it has
  children.
- An amplification ratio computed from estimated usage is labelled estimated.
- All twelve scenarios in §5.3 are implemented and pass, including the
  combined-check regression case constructed so each child passes alone.
- Permission-propagation violations and cancellation completeness are asserted
  zero, and a seeded violation fails the run.
- The experimental verdict is **derived** from the recorded comparison, not set
  by hand, and the UI label follows it.
- An execution mode with no recorded comparison is labelled experimental.
- A comparison showing multi-agent below baseline produces
  `verdict: "experimental"` and a reason naming the failing measure.
- `evals/baseline.json` is not overwritten by a multi-agent run, and the
  comparison file references the baseline version it used.
- Both files are human-reviewed before commit.
- Every new metric row appears in the machine-readable output with a value or an
  explicit `notApplicable` marker naming the work package that would supply it.
- The remote/local equivalence row is `notApplicable: "phase-6-wp5"` and does
  not block the metric-coverage assertion.
- No model-graded scoring is used anywhere.
- The five quality-gate commands from `AGENTS.md` pass, and the harness adds no
  new quality-gate command.

## 7. Implementation Notes

To be completed during implementation. **This section is the phase's deliverable
as much as the code is.** Record:

- **The comparison result**: token, latency, and model-call amplification, task
  completion and check pass rate for both modes, against which baseline version,
  on which scenarios.
- **The verdict, and the decision taken.** If multi-agent measured at or above
  baseline, say so with the numbers. If it did not, record the decision to leave
  it disabled or remove it — and state plainly that this was the correct outcome
  rather than a failure, per the phase file. A reader in a year should be able to
  tell whether delegation was tried and rejected on evidence, or tried and
  quietly left half-enabled.
- Whether stage 1 (read-only subagents) and stage 2 (editing subagents) were
  measured separately, per [spec 38](38_subagent_model.md) §5.4. Read-only
  delegation may measure well while editing delegation does not, and the honest
  outcome could be to ship one and not the other.
- The redundant-read overlap figure. High overlap between siblings means the
  parent is delegating poorly, which is a prompt problem the measurement should
  surface rather than a reason to distrust the metric.
- How often the combined-check rerun caught a failure the children missed, which
  [spec 39](39_coordination_and_conflict_handling.md) §7 also asks for — record
  it in one place and cross-reference.
