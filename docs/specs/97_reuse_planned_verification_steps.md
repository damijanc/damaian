# Feature Spec: Defer Planned Whole-Suite Verification

Status: Done 2026-10-31 (specified and built the same day; local checks in §6,
the full gate is left to CI).
Order: 97 of 97
Plan: none. A defect reported in use, like #73–#96.
Depends on: [#21](21_task_plan_progress_and_budget/proposal.md) (plan state)
and [#23](23_verification_loop/proposal.md) (verification loop), both built.

## 1. Motivation

A task plan can end with **Run `cargo test`**. After every patch is applied,
automatic verification also selects `cargo test`. That suite already tests the
whole repository, so running it after each worker patch is expensive and does
not describe the final state the plan says it validates. It also used to append
and activate a new `verify_` step while the plan's implementation step was
still in progress, and the panel correctly displayed its integrity warning.

The warning is evidence, not a condition to suppress. A whole-suite check the
plan explicitly schedules belongs to its final plan step, not to each
post-patch verification turn. Secondary automatic checks must not claim a
second primary step while another step is open.

## 2. Requirements

1. An open non-verification plan step that names an exact selected check command
   defers that command from automatic post-patch verification. The worker runs
   it once when it reaches the planned step through the usual command and
   approval flow.
2. A deferred command neither runs nor adds a `verify_` duplicate to the
   carried plan. The verification report honestly records that nothing ran
   when no remaining automatic check runs.
3. A newly added automatic verification substep stays pending while another primary step
   is already in progress. It still records its eventual terminal result.
4. No durable log prefix produced by the reproduction contains more than one
   `in_progress` step. The UI warning remains unchanged as the diagnostic for
   malformed historical or external state.
5. Check approval, repair, and recovery behavior are unchanged. Only the
   scheduling of a plan-owned whole-suite check changes.

## 3. Design

`run_verification_checks` first removes checks the open carried plan explicitly
owns. A command match is exact, either in a backtick-delimited command or as a
whitespace token sequence, so an incidental word such as “testing” cannot
defer a validation step. Existing `verify_` rerun behavior remains unchanged.
The continued agentic turn receives the open plan, eventually reaches its
final validation step, and runs that command once.

`start_verification_step` is the one transition that marks a check step active.
If a different plan step is already active, the check is a secondary validation
substep and remains pending until its result closes it. This preserves the
current primary work instead of silently completing or rewriting it.

## 4. Acceptance Criteria

`verification_defers_a_planned_final_check_without_two_steps_in_progress` in
`workspace-engine/tests/verification_loop.rs` creates a plan with an active
implementation step and a pending final **Run `cargo test`** step, then applies
a patch and runs automatic verification. It asserts that the planned test step
stays pending, no `verify_` duplicate exists, the report says nothing ran, and
every replayed session-log prefix preserves the single-in-progress invariant.

## 5. Verification

Local checks, 2026-10-31:

- `cargo nextest run -p workspace-engine --test verification_loop verification_defers_a_planned_final_check_without_two_steps_in_progress --locked`.
- `cargo nextest run -p workspace-engine --test verification_loop --locked` (70 passed).
- `cargo clippy -p workspace-engine --all-targets --locked -- -D warnings`.
- `scripts/quality-gate.sh fmt node lint typos deny` (the `deny` subcheck was
  rerun directly because the sandbox prevents its advisory-cache lock; all
  checks passed).
- `cargo run -p eval-harness -- run --tier deterministic` (46 scenarios
  passed; rerun with localhost stub-server permission because the sandbox
  forbids port binding).
- `npm run specs:check`.

The workspace-wide clippy and test suite are left to CI. No commit or push was
made.
