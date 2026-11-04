# Spec 105 planning context

Status: planned 2026-11-03, not started. This file records the decisions and
corrections planning found against the code, not the design again; read
[`proposal.md`](proposal.md) first.

## 1. `CheckId` is already the right shared identity

`verification.rs:46` `CheckId { working_directory, command }` carries exactly
the two dimensions a structured check needs: the command and its
repository-relative directory (`""` is the root). Ordering and hashing are on
both fields, so a plan, a report and a deduplicated selection already agree on
identity. `PlannedCheck` should reference the same two fields and the plan →
verification match is `CheckId` equality, never parsed prose. The
`working_directory` in a plan is the step's own directory, and the check's
directory defaults to the repository root when the model leaves it empty —
matching `CheckId`'s existing `""` convention.

## 2. #97's prose matching is what retires

`chat.rs` `plan_has_open_check_command` / `plan_step_names_command` did the
prose match: an exact command in a backtick span or as a whitespace token
sequence, on any non-terminal non-`verify_` step's `title` or `detail`.
Requirement 4 retired this entirely in Task 2 — the deferral filter in
`run_verification_checks` is now `plan_defers_check`, a `CheckId` equality
(`working_directory` + `command`) against a `required: true` `PlannedCheck` on
an open plan step. Two consequences, recorded and deliberately not dodged:

- A historical prose-only plan that says "Run `cargo test`" no longer defers
  that suite after every patch; it reverts to per-patch automatic checks,
  because it carries no structured `PlannedCheck`. That is requirement 10's
  intent ("historical plans remain supported with no required structured checks")
  and is a deliberate behaviour change from #97.
- The #97 test `verification_defers_a_planned_final_check_without_two_steps_in_progress`
  was rewritten to attach a structured check to the final step rather than to
  name a command in prose, and a counter-case was added: a planned check at a
  different directory, with a different command, or with `required: false` is
  not a deferral target and the discovered check still runs.

## 3. The plan tool schema and the parse path

`propose_plan_tool_definition` (`chat.rs:16234`) currently allows only `title`
and `detail` per step. Three places must stay in lock-step when `check` is
added: the schema, the `ToolAction::ProposePlan(Vec<…>)` parse that turns the
model's step objects into `PlanStep`, and `PlanStep`'s serde. `PlanStep.check`
is `Option<PlannedCheck>` with `#[serde(default)]`, so historical plan JSON and
replayed plans load with `None`. `validate_plan_structure` /
`validate_plan_approval` (`session.rs:244`) must accept the field the same way
they accept retitle/reorder: the user may edit a proposed check, so the
structure check does not re-validate its command against the live invariants.

## 4. "Authoritative after approval" is the settled plan, which already exists

#103 already stores the settled plan on approval: `PlanApproval { task_id,
approved_by, budget, plan }` (`session.rs:221`) and
`approve_plan_with_budget_and_plan`. So a structured check becomes authoritative
only by riding on the `TaskPlan` that `plan_approved` stores (and
`read_task_plan` replays). The completion gate and the verification deferral
must read `PlannedCheck`s from that persisted plan, never from a fresh
`propose_plan` tool call. Requirement 2's second sentence (model output before
approval cannot authorise a command) is already true through command policy;
the gate's job is to never treat an unapproved proposed check as a completion
obligation.

## 5. The completion gate's "required checks" source changes

`CompletionGateReport::evaluate` (`completion.rs:206`) derives required checks
from `facts.verification.selected` — the set the loop happened to run
(`completion.rs:257`). That is the coupling this spec breaks: the gate must
instead require current terminal evidence for every `required: true`
`PlannedCheck` on the approved plan. The evidence itself still lives in
`VerificationState.checks` (a `BTreeMap<CheckId, CheckRun>`, `verification.rs:736`),
keyed by `CheckId`, so the gate looks the check up by identity rather than by
selection order. `VerificationReport::build` (`verification.rs:946`) computes
staleness over `state.selected`; the structured-required set and the selected
set overlap but are not the same (a deferred final-suite check is not selected
until it runs at its plan step), so the gate must not reuse `selected` as the
source of "required".

Resolved 2026-11-04 (Task 5): the evidence for a required check is read from
**one of two existing stores, never a third**. The gate iterates the plan's
primary steps, maps each `required: true` `PlannedCheck` to
`CheckId::new(working_directory, command)`, and then:

- **Ran through the loop (non-deferred).** A `CheckRun` in
  `verification.checks` is the evidence. Currentness is
  `CheckRun::is_stale(current_hashes, current_generation)` — the generation the
  run recorded against the plan's current `mutation_generation()`, plus the
  file digest it validated. This is the same rule the loop's own report uses.
- **Deferred to its plan step.** Task 2 removed the check from the selection, so
  there is no `CheckRun`; the worker ran the exact command at the step through
  spec 97's ordinary command flow, which recorded an `Evidence::CommandExit` on
  the plan step. The gate matches the latest `CommandExit` whose `command_hash`
  equals `sha256(planned.command)`. That evidence records no generation, so
  currentness is read from the step's position: the step is terminal **and** no
  later primary step has left `Pending` (a later primary step leaving `Pending`
  is exactly what advances `mutation_generation()`). A non-terminal step, or a
  step followed by a begun step, is stale.

The `CheckRun` is authoritative when both exist (a check deferred and run as a
command can later be re-run by the loop after a continuation, and the `CheckRun`
is the more recent evidence); the `CommandExit` is the fallback. A discovered
check that is not on the plan is **not** a gate obligation at all — its pass,
failure or skip is reported by the verification loop, not by the completion
gate, so a discovered `cargo test` failure settles the task `failed` through
the verification settlement while the gate report itself names only the open
plan work. This is the deliberate consequence of requirement 6 and non-goal 3
("structured required checks define the completion obligation").


## 6. Mutation-batch generation sits beside the existing file-hash staleness

`CheckOutcome::Ran` already records `file_hashes` (`verification.rs`, the
`VerificationEvent::Check.file_hashes` field), and staleness is
`is_stale(run, current_hashes)` — stored hashes versus the current checkout. A
generation counter adds a monotonic batch identity: applied patches and
modifying commands within one primary plan step share one generation, and the
generation advances when the step closes (or a new primary step begins). Check
evidence records the generation it validated; a later generation (or a changed
digest) marks it stale without deleting history.

Resolved 2026-11-04 (Task 4): the generation is **derived**, not a stored
counter. A stored counter would be a second source of truth that can disagree
with the plan after recovery, and the primary-step boundaries are already
durable in the log (`plan_step_updated`). `TaskPlan::mutation_generation()`
derives it as the number of primary (non-`verify_`) steps that have left
`Pending`, minus one, so the first step is generation 0 and each later step
advances it. A task with no plan, or no primary step, reads as generation 0 —
the same value historical plans carry, so backward compatibility is automatic
rather than a `#[serde(default)]` on a stored field. Because `verify_*` steps
are excluded, the verification loop's own steps (which `start_verification_step`
opens while a primary step is still active) never advance the batch, so the
"one step active" rule is unaffected. `CheckRun::is_stale(current_hashes,
current_generation)` is the single place that decides stale — a changed digest
or an advanced generation — and `VerificationReport::build` feeds it the
generation derived from the plan it already holds.

## 7. `purpose` is a new concept, not `CommandPurpose`

`PlanStep` evidence already carries `CommandPurpose::Exploration` /
`Verification` (see `plan.rs` `Evidence::CommandExit`). The `PlannedCheck.purpose`
is a separate, plan-level field (why the check is required — e.g. "validates the
final tree"), shown on the review card, not the runtime command classification.
Do not fold it into `CommandPurpose`.

## 8. Plan review card and edit path

`plan_proposal_json` (`desktop-shell/lib.rs`) builds the review card and
`app.js createPlanReview` renders it; the user already revises steps before
approval. Requirement 7 adds: the card lists each required check's command and
directory, and the edit path carries `PlannedCheck` changes (add, remove,
retitle, change directory) through `approve_plan_with_budget_and_plan`, so the
settled `TaskPlan` records exactly what the user approved.

Task 3 follow-up: the editor (`app.js planCheckControl`) now also edits
`purpose` and toggles `required`, so a `required: false` check is recorded and
rendered as "Check (optional)". `check_json` now transports `version` so a
re-approval of an unchanged future-version check is not silently re-stamped to
`PLANNED_CHECK_VERSION`; the editor keeps the existing `version` on change and
omits it on add, letting the server default a new check to the current version.

## 9. Recovery and continuation carry structured state for free

A `PlannedCheck` lives on the persisted `TaskPlan`, and check evidence already
lives in the verification log. `plan_resumed` ancestry and
`read_task_plan`/`read_task_outcome` already replay both, so recovery and
continuation retain structured identities without a new path. The one thing to
verify by test is that a continued or recovered task's gate still reads the
carried plan's required checks, not a rebuilt prose interpretation.

## 10. What was intentionally not changed

- The verification loop's command approval, repair, recovery and check-card
  flow (#23, #97, #102).
- `CheckOutcome`, `SkipReason`, `CheckRun`, and the file-hash staleness check.
- #101's completion settlement, `incomplete` continuation, and worker outcome
  model; this spec only changes *which* checks the gate treats as required.
- The `TaskPlan`/`PlanStep` JSON shape for steps that carry no check.

## 11. Checks run locally

None yet — this is the planning pass. The implementation tasks record their
scoped checks in `tasks.md`.
