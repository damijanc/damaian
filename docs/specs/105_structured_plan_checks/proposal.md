# Feature Spec: Approved Plans Own Structured Required Checks

Status: Not started (planned 2026-11-03; see [context.md](context.md) and
[tasks.md](tasks.md))
Order: 105 of 106
Plan: none. This closes the verification-scheduling gap exposed by the
2026-10-31 spec 86 attempt.
Depends on: [#21](../21_task_plan_progress_and_budget/proposal.md) (approved
plans), [#23](../23_verification_loop/proposal.md) (automatic verification),
[#97](../97_reuse_planned_verification_steps.md) (deferring an exact command),
and [#101](../101_closed_task_completion.md) (completion gate) — #21, #23, #97
and #101 are built; **nothing here is unbuilt, so this spec is ready to start**.
Everything else named below is a cross-reference, not a prerequisite.

## 1. Motivation

#97 recognizes a planned final check by finding an exact command in plan-step
prose. That is intentionally narrow but still treats execution data as text.
It cannot reliably distinguish a required check from a mention, bind the
working directory, determine whether a later result is current, or tell the
completion gate which checks the approved plan requires.

The spec 86 attempt consequently ran broad `cargo test` checks after
intermediate patches, repeated work, repaired failures, and still never had an
engine-owned statement that all requested checks had passed on the final tree.

## 2. Requirements

1. A plan step may carry a structured `check` containing command, repository-
   relative working directory, purpose and whether it is required for task
   completion. The human-readable step text remains.
2. Structured checks become authoritative only when the user approves the
   plan. Model output before approval cannot authorize command execution.
3. Plan approval is not command approval. Each check still follows command
   policy and its ordinary one-shot or persistent approval route when reached.
4. The verification loop matches checks by structured identity, never by
   parsing plan prose. A required final-suite check is deferred until its plan
   step; smaller different checks may still run after intermediate mutations.
5. Check evidence records command, directory, exit, findings and a digest of
   the task-changed files it validated. Any later mutation of those files makes
   the evidence stale.
6. #101's completion gate requires current terminal evidence for every
   required check. Passed evidence satisfies it; failed or missing evidence
   does not. A declined check satisfies only an explicit waiver and is named in
   the final disposition.
7. The plan review card clearly lists required checks and their directories.
   The user may remove or change a proposed check before approval; that edit is
   recorded as part of the approved plan.
8. Checks named by the task instructions or repository agent instructions are
   proposed by the model for review, not silently trusted from repository
   configuration. Repository text cannot auto-approve a command.
9. Verification runs once per relevant mutation batch or final plan step, not
   once merely because the model emitted another patch in the same step.
10. Recovery and continuation retain structured identities and evidence
    without reinterpreting old prose plans. Historical plans remain supported
    with no required structured checks unless a user explicitly adds them.

## 3. Non-goals

- Parsing arbitrary `AGENTS.md` prose into trusted commands in the engine.
- Automatically approving a check because the plan was approved.
- Replacing automatic check discovery. Discovered targeted checks may augment
  the plan; structured required checks define the completion obligation.
- Claiming a skipped check passed.

## 4. Design

Extend `PlanStep` with an optional versioned `PlannedCheck`. The plan tool
schema accepts it, the plan-review UI edits it, and the approval event stores
the settled structure. `CheckId` becomes the shared identity between the plan
and verification loop.

Add a mutation-batch generation to task state. Applied patches and modifying
commands increment it at the boundary of a primary plan step, while several
patches within that step remain one batch until the step closes. Required-check
evidence names the generation and changed-file digest; later mutations mark it
stale without deleting history.

## 5. Acceptance

- A plan with `cargo nextest run -p workspace-engine` at the repository root
  defers exactly that check without matching another directory or command.
- Plan approval does not run the command; command approval is still required.
- Two patches in one implementation step trigger one batch verification, while
  closing the final check step records current evidence.
- Mutating a validated file makes the required check stale and prevents
  `complete` until it reruns or the user explicitly waives it.
- Old prose-only plans still load and do not invent required checks.
- The completion report lists every required check as passed, failed, stale or
  waived with its working directory.
