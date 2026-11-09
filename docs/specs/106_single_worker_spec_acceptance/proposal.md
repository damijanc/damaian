# Feature Spec: Single-Worker Specification Acceptance Gate

Status: In progress (started 2026-11-09; Tasks 1–7 in [tasks.md](tasks.md))
Order: 106 of 106
Plan: none. This is the executable capability gate requested after one Damaian
worker failed to finish the reserved spec 86 on 2026-10-31. The planning pass
and its decisions are in [context.md](context.md).
Depends on: [#18](../18_local_evaluation_harness/proposal.md) (evaluation
harness), [#98](../98_task_scoped_protected_patch_paths.md) (scope fence),
[#101](../101_closed_task_completion.md),
[#102](../102_repair_provenance_and_stagnation.md),
[#103](../103_bounded_worker_continuation.md), [#104](../104_worker_questions/proposal.md),
and [#105](../105_structured_plan_checks/proposal.md) — #18, #98, #101, #102, #103,
#104 and #105 are built; **nothing here is unbuilt, so this spec is ready to
start**.

## 1. Motivation

Unit tests prove individual transitions, and deterministic eval scenarios
script model tool calls. Neither proves that a real worker can take one
specification-sized request through planning, several edits, verification,
repair, durable task-record updates and truthful completion.

The product must not claim autonomous specification work from a collection of
mechanics. It needs a repeatable gate whose unit is the outcome the user wants:
one worker finishes one spec without generic rescue prompts.

## 2. Requirements

1. Add a `single_worker_spec` evaluation fixture large enough to require
   repository exploration, a reviewed plan, at least three source/document
   changes, two mutation batches, structured required checks and a task-record
   update.
2. Seed one pre-existing failing check that the task must report but not repair,
   and one regression caused by the scripted first implementation that the
   worker must repair.
3. Seed a protected unrelated file and make relevant repository text tempting
   enough that an unconstrained repair could touch it. No proposal or applied
   patch may include it.
4. Include one material clarification whose answer changes an acceptance
   detail. The worker must ask and resume rather than guess.
5. The deterministic tier mechanically exercises every state transition and
   remains part of the workspace test suite. It is not evidence that a model
   chooses the right tools.
6. The live tier runs through the real worker host in a fresh worktree and
   isolated `DAMAIAN_DATA_DIR`, against an explicitly selected real provider.
   It is ignored by default and documents the exact manual command.
7. Mandatory patch, command, check and question decisions may pause the run.
   Generic **Continue task**, **Continue repair**, task reset, manual source
   edits and out-of-band Git repair are forbidden interventions.
8. Success requires: every approved plan step terminal; required checks current
   and passed, explicitly waived, or failing only with a pre-existing failure
   that is unchanged against a real task-start baseline (requirement 11); the
   introduced regression repaired; the pre-existing failure unchanged and
   classified correctly; protected paths untouched; task records truthful; no
   pending decision; and final `TaskStatus::Complete` accepted by #101's gate.
9. Run the live scenario successfully three consecutive times from fresh state
   before marking this specification Done. Record provider/model, duration,
   rounds, tokens, approval counts, repair attempts and compactions for each
   run. A failed attempt resets the consecutive count.
10. A quality-gate pass cannot substitute for the live acceptance, and live
    acceptance cannot substitute for CI. Both are recorded in this spec's task
    history when it is implemented.
11. Added in planning (2026-11-09, [context.md](context.md) D1): the completion
    gate accepts a required check whose only failure is pre-existing and
    unchanged — provenance `pre_existing` or `unchanged_failure` measured
    against a real task-start baseline — as a named completion disposition
    shown in the report, never as passed. `uncertain` never qualifies, and any
    introduced finding in the same check still blocks until repaired. Without
    this, requirement 8 is unsatisfiable: #101's gate assigns no provenance, and
    a floor check (spec 101/105 review fix R15) that already failed cannot be
    declined, so any task touching a root with a known failing check could
    never complete.

## 3. Non-goals

- Multi-worker delegation, coordination or branch integration. Specs 38–40 are
  downstream and remain out of this gate.
- Requiring zero user safety decisions. Existing patch, command and question
  boundaries are part of the product contract.
- Benchmarking Damaian against another assistant.
- Using the Damaian repository itself as an automated test fixture. The live
  fixture is disposable and cannot modify the user's checkout.

## 4. Design

The fixture is a small Rust workspace plus documentation and a folder-style
spec record. Its task asks for a behavior change with repository instructions
modelled on this repository: scoped checks, progress-row updates and an
explicit no-touch path. A deterministic response script covers the complete
state machine, including baseline comparison, one repair, a question, internal
segment continuation and final completion gating.

The live runner starts the same request through the desktop worker host rather
than calling `run_agentic_turn` directly. Decisions are supplied by a fixture
driver only where the UI would present an allowed decision; the driver refuses
generic rescue actions and fails on any unexpected proposal.

Both tiers share one fixture, one decision driver and one acceptance report
(planning decisions D2–D9 in [context.md](context.md)). The deterministic tier
hosts the worker in process with a scripted model; the live tier spawns the
real worker binary through `workers::Supervisor`. Patches are applied the way
the desktop shell applies them, followed by the same automatic `verify` the UI
sends, so neither tier has a side door the product lacks.

## 5. Acceptance report

The final report contains, for each of the three consecutive live runs:

- terminal task status and completion-gate reasons;
- plan-step and required-check dispositions;
- changed and protected paths;
- baseline and repair classifications;
- rounds, segments, compactions, tokens and wall time;
- every user decision by kind; and
- confirmation that no forbidden intervention occurred.

Until that report exists, Damaian may describe the underlying worker features
but must not claim that one worker can autonomously finish a specification.
