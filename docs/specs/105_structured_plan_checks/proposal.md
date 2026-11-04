# Feature Spec: Approved Plans Own Structured Required Checks

Status: Done (started 2026-11-03; Tasks 1–6 done 2026-11-04 — see
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
   completion. The directory is normalized to `CheckId` form (`""` is the root,
   `/` separators); input that cannot be repository-relative is refused. The
   human-readable step text remains.
2. Structured checks become authoritative only when the user approves the
   plan. Model output before approval cannot authorize command execution.
3. Plan approval is not command approval. Each check still follows command
   policy and its ordinary one-shot or persistent approval route when reached.
4. The verification loop matches checks by structured identity, never by
   parsing plan prose. A required final-suite check is deferred until its plan
   step; smaller different checks may still run after intermediate mutations.
5. Check evidence records command, directory, exit, findings and a digest of
   the task-changed files it validated — including when a deferred check runs
   as an ordinary command at its plan step. Identity is directory plus command.
   Any later mutation of those files makes the evidence stale.
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

A check's directory is normalized where a `PlannedCheck` is created from model
or user input: `propose_plan` and the plan-revision path. One normalizer, built
on the lexical `relative_location` used to build a `CheckId`, resolves the input
against the repository root and returns `""` for the root, `/` separators, no
leading `./` and no trailing `/`. That is the same form the executed command's
evidence records. A directory that is absolute outside the repository, or that
escapes it with `..`, cannot be repository-relative: a proposed check is dropped
the way a check without a command is, and a plan revision is rejected with a
clear error rather than storing a check that can never match.

A deferred check leaves an `Evidence::CommandExit` on its plan step rather than
a `CheckRun`. That exit records the repository-relative working directory,
normalized exactly as `CheckId` normalizes it (`""` for the root, `/`
separators), and the completion gate matches directory *and* command hash. An
exit written before the directory was recorded reads as the root: it cannot
satisfy a check in a subdirectory, and it still satisfies a root check, which is
what the pre-fix gate assumed of every matching command.

## 5. Acceptance

- A plan with `cargo nextest run -p workspace-engine` at the repository root
  defers exactly that check without matching another directory or command.
- Plan approval does not run the command; command approval is still required.
- Two patches in one implementation step trigger one batch verification, while
  closing the final check step records current evidence.
- Mutating a validated file makes the required check stale and prevents
  `complete` until it reruns or the user explicitly waives it.
- Old prose-only plans still load and do not invent required checks.
- A deferred check matches only the directory it requires: the same command run
  at the root does not satisfy a check in a subdirectory, and a pre-directory
  exit satisfies only a root-directory check.
- A planned check's directory is stored in `CheckId` form from both
  `propose_plan` and the review card; `"."`, `"./crates/foo/"` and
  `"crates/foo"` name the root and the same subdirectory, while a directory
  outside the repository is dropped from a proposal or rejected in a revision.
- The completion report lists every required check as passed, failed, stale or
  waived with its working directory.

## Review fix 2026-11-04: deferred check evidence carries its directory (R3)

The review of specs 101–105 found that `Evidence::CommandExit` recorded only
`command_hash`, and the deferred path in `completion.rs` matched on that hash
alone. Spec 105's identity is directory plus command, so a plan requiring
`cargo test` in `crates/foo` was counted by `cargo test` at the repository root.

The exit now records `working_directory`, repository-relative and normalized
with the same `relative_location` code that builds a `CheckId` directory (`""`
is the root). The deferred path matches the full identity: command hash and
directory. The field is `#[serde(default, skip_serializing_if =
"String::is_empty")]`, so existing session logs load with the root. That is the
deliberate choice for an event written before this change: it cannot satisfy a
subdirectory check, and it still satisfies a root check, because the pre-fix
gate treated every matching command as a root check. The staleness rule
(`step_is_current`) is unchanged; R4 owns it.

The regression tests were written first. `plan_turn`
`a_required_check_in_a_subdirectory_is_missing_when_the_command_ran_at_the_root`
and `closed_task_completion`
`a_command_exit_written_before_the_directory_was_recorded_does_not_satisfy_a_subdirectory_check`
both failed with `left: []`, `right: [RequiredCheckMissing]` — the gate was
eligible because the command alone matched. Both pass now.
`a_required_check_in_a_subdirectory_passes_when_the_command_ran_there` drives the
same `run_command` path in `crates/foo`, asserts the directory recorded on the
step, and passes; `a_command_exit_written_before_the_directory_was_recorded_still_satisfies_a_root_check`
pins the root-compatibility choice, and
`a_command_exit_working_directory_is_omitted_at_the_root_and_read_back_elsewhere`
pins the serde shape.

**Follow-up, same day.** The evidence directory is now part of check identity, so
the plan side had to record the same form: `propose_plan` only trimmed
`workingDirectory` and the desktop revision route took it as sent. A model that
wrote `"."`, `"./"`, `"./crates/foo"` or `"crates/foo/"` therefore stored a check
that could never match the normalized evidence or a discovered `CheckId` — the
gate reported `RequiredCheckMissing` forever. `normalize_planned_check_directory`
(exported from `plan.rs`) is now the one normalizer: it resolves the input
against the repository root and calls the same `relative_location` used to build
a `CheckId`, yielding `""` at the root, `/` separators, no leading `./` and no
trailing `/`. It is applied where a `PlannedCheck` is created from model or user
input — the `propose_plan` dispatch and both plan-revision paths — and the
deferral match normalizes the stored directory too, so a legacy `"./crates/foo"`
still defers `crates/foo`. A directory that is absolute outside the repository or
escapes it with `..` is refused: a proposed check is dropped like a check with no
command, and a revision is rejected with a clear error before the pending review
is consumed.

Tests written first: `plan_turn`
`a_plan_check_with_a_dot_directory_matches_the_command_run_at_the_root` failed
with `left: Some(".")`, `right: Some("")`;
`a_plan_check_with_a_relative_subdirectory_matches_the_command_run_there` failed
with `left: Some("./crates/foo/")`, `right: Some("crates/foo")`;
`a_plan_check_with_a_directory_outside_the_repository_is_dropped` found the
outside check still present; `desktop-shell`
`a_plan_revision_normalizes_a_check_directory_like_a_check_id` failed with
`left: [".", "./crates/foo/", "crates/foo"]`, `right: ["", "crates/foo",
"crates/foo"]`; and `a_plan_revision_rejects_a_check_directory_outside_the_repository`
parsed the outside directory instead of refusing it. All pass now, along with the
normalizer and deferral tests
`a_planned_check_directory_is_normalized_like_a_check_id`,
`a_planned_check_with_a_dot_prefix_defers_the_normalized_discovered_check` and
`a_plan_revision_with_a_directory_outside_the_repository_is_rejected`.
