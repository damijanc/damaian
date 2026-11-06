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
   Any later mutation of those files makes the evidence stale, including one
   applied after the check in the same step; rerunning the same check in the
   same directory after the mutation makes it current again.
6. #101's completion gate requires current terminal evidence for every
   required check. Required checks are the plan's `required: true` checks plus
   the discovered-check floor: the detected checks of the owning and enclosing
   roots of every file the task touched. Passed evidence satisfies it; failed or
   missing evidence does not. A declined check satisfies only an explicit waiver
   and is named in the final disposition: whether the user declines it on the
   verification check card or declines the exact deferred command at its plan
   step. A policy refusal, a timeout or the model not running the command is
   never a waiver.
7. The plan review card clearly lists required checks and their directories.
   The user may remove or change a proposed check before approval; that edit is
   recorded as part of the approved plan.
8. Checks named by the task instructions or repository agent instructions are
   proposed by the model for review, not silently trusted from repository
   configuration. Repository text cannot auto-approve a command.
9. A mutation batch is verified as it happens, not deferred to the step's
   close: an applied patch records a verification due immediately, so repair
   feedback is never delayed. Within one primary plan step, a check that already
   passed in the current batch and whose validated files still hash the same is
   not run again merely because another patch landed. A later patch that changes
   a file the check validated makes the evidence stale, so the check runs again;
   a check is never reported current for a file changed after it ran.
10. Recovery and continuation retain structured identities and evidence
    without reinterpreting old prose plans. Historical plans remain supported
    with no required structured checks unless a user explicitly adds them.

## 3. Non-goals

- Parsing arbitrary `AGENTS.md` prose into trusted commands in the engine.
- Automatically approving a check because the plan was approved.
- Replacing automatic check discovery. Discovered checks are the completion
  floor (spec 101/105 review fix R15), while plan-declared checks are added to
  it; neither replaces the other.
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

The generation is the batch boundary, not the verification schedule. A patch
records a verification due as soon as it is applied, so repair feedback is not
delayed until the step closes. Within one primary step a check that already
passed in the current batch and whose validated files still hash the same is not
run again for a later patch; a later patch that changes a validated file makes
the run stale and it runs again. "Once per batch" therefore never means
reporting evidence that no longer matches the tree (review fix R11,
2026-11-06).

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

Deferred evidence currentness is decided from the durable plan record, never a
wall-clock comparison that can tie. The evidence must still belong to the
current mutation batch — its step terminal and no later primary step begun,
which is the boundary [`TaskPlan::mutation_generation`] counts — and no
[`Evidence::PatchApplied`] may follow the matching `CommandExit` in plan order,
whether on the same step or a later one. A patch merely proposed, declined or
never applied records no `PatchApplied` and so leaves the check current. A rerun
of the exact command in the exact directory after the patch is the newest
matching exit and is current again. This reuses spec 21's applied-patch evidence
and spec 105 Task 4's batch boundary rather than adding another counter.

An explicit user decline of that exact command is durable plan evidence too
(`Evidence::CommandDeclined`, review fix R8), carrying the same command hash and
normalized directory as the exit. The gate matches declines and exits with the
same identity function and lets the latest matching command evidence decide: a
later approved run supersedes an earlier decline, while a decline with no later
run is a named waiver, never a pass. Only the explicit decline records it; a
policy refusal, a timeout or a model that never runs the check does not. The R16
floor uses the same matcher over the whole plan, so declining a floor check the
model ran itself is the same named waiver.

The completion obligation is not plan-only. Spec 101/105 review fix R15
(decision D1) adds the **completion floor**: the detected checks of the owning
and enclosing roots of every file the task touched, selected with spec 23
§5.2's root rule and excluding documentation-only work. The gate merges the
floor with the plan's `required: true` checks by `CheckId`, so the same identity
is one obligation and a plan check never replaces a floor check. The floor reads
the persisted repository map and stores no proposal; each check still follows
command policy and the ordinary approval route. Review fix R16 (2026-11-06)
narrows per-change verification to the owning root, as spec 23 §5.2 now
records; the floor keeps the enclosing roots at task completion, which is what
catches cross-root breakage. A completion verification (no open or blocked
primary plan step) runs the floor through the same command-policy path. An
answered turn at completion records a due for the floor checks that are missing
or stale and are not plan-declared, regardless of any other reason, and runs
only those before the answer becomes final. A floor check is satisfied by a
current `CheckRun` or by a current `CommandExit` of the identical command and
directory anywhere in the plan, using the deferred path's matcher and R4
currentness rule.

## 5. Acceptance

- A plan with `cargo nextest run -p workspace-engine` at the repository root
  defers exactly that check without matching another directory or command.
- Plan approval does not run the command; command approval is still required.
- Two patches in one implementation step that leave a check's validated files
  unchanged run that check once (one batch verification), while closing the
  final check step records current evidence. A second patch to a file the check
  validated reruns the check, so its evidence is never current for content it
  did not see.
- Mutating a validated file makes the required check stale and prevents
  `complete` until it reruns or the user explicitly waives it.
- A patch applied after the deferred check in its own step makes that check
  stale; rerunning the same check in the same directory after the patch makes it
  current again and lets the task complete.
- A patch applied in a later step also makes the deferred check stale, while a
  patch that is only proposed, declined or never applied leaves it current.
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
- Declining the exact deferred command of a plan-declared check or a floor check
  the model ran itself is a named waiver and lets the task complete; declining an
  unrelated command or the same command in another directory is not. A later
  approved run of the same check decides instead of an earlier decline.
- A task that changed source files has the detected checks of their owning and
  enclosing roots as required checks even when its plan declares none; a
  documentation-only task has none.
- A plan's only check may be a trivial command; the floor is still required.
  A plan check with the same identity as a floor check is one obligation, and
  declining a floor check that needs approval is a named waiver.
- In a Cargo workspace, a member change puts the workspace root's check in the
  floor.

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
gate treated every matching command as a root check. The staleness rule was left
for R4, below.

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

## Review fix 2026-11-05: deferred check evidence tracks later mutations (R4)

The same review found that the deferred path's currentness rule,
`step_is_current`, only checked the step's position. A patch applied after the
check but before the step closed therefore left the `CommandExit` looking
current, and the task could settle `complete` on a tree the check never saw. The
finding was verified in the code; a regression test reproduces it.

Currentness for a deferred check is now two durable plan facts, not a
wall-clock comparison. The exit must still belong to the current mutation batch
— its step terminal and no later primary step begun, the boundary
`TaskPlan::mutation_generation` counts — **and** no `Evidence::PatchApplied` may
follow the matching `CommandExit` in plan order, on the same step or a later
one. `step_is_current` is kept because it adds the batch-boundary half: a later
primary step beginning advances the generation even before it mutates anything,
so a check from an earlier batch is stale then too. The new mutation half is
`mutation_after` in `completion.rs`. It reuses spec 21's applied-patch evidence
and spec 105 Task 4's derived generation rather than adding a counter or
timestamp. A patch that is only proposed, declined or never applied records no
`PatchApplied`, so it leaves the check current; a rerun of the exact check in the
exact directory is the newest matching exit and is current again. The
`CheckRun` (verification-loop) staleness path is unchanged.

The regression test was written first. `closed_task_completion`
`a_patch_applied_after_the_deferred_check_makes_its_evidence_stale` failed with
`assertion failed: !report.eligible` — the gate considered the check current
because its step was terminal. `plan_turn`
`a_patch_applied_after_the_deferred_check_makes_its_evidence_stale` failed with
`left: Complete, right: Incomplete`: the check ran, a real patch was applied,
the step closed, and the task still completed. Both pass now. The matching
guards are `a_deferred_check_rerun_after_a_patch_is_current` and
`rerunning_the_deferred_check_after_the_patch_makes_it_current` (the rerun is
current), `a_patch_applied_in_a_later_step_makes_the_check_stale` and
`a_patch_applied_in_a_later_step_makes_the_check_step_stale` (the later step
still invalidates it), and
`a_patch_that_is_declined_or_never_applied_does_not_make_the_check_stale`
(a pending or declined card records no mutation). The later-step guards already
passed before the fix because `step_is_current` covered that position; they keep
the behaviour from regressing when staleness starts reading mutations.

## Review fix 2026-11-06: the discovered-check floor (R15)

Task 5 made the plan's structured `required: true` checks the only completion
obligation. The review of specs 101–105 found that this let a plan with no
`check` change source files, run nothing and settle `complete`. Decision D1 and
review fix R15 restore the discovered checks as a floor without giving up the
structured-checks model:

1. A task that changed files has required checks even when its plan declares
   none.
2. The floor is the detected checks of the owning and enclosing roots of every
   file the task touched. Documentation-only tasks get no floor (spec 23 §5.2).
3. Plan-declared required checks are added to the floor, merged by `CheckId`;
   they never replace it, so a trivial declared check cannot escape it.
4. The floor's baseline is R14's pre-patch baseline for the roots the first
   patch touches. A root first touched by a later patch falls back to
   `uncertain` (spec 102's R15 note).
5. Declining a floor check that needs approval is the existing `DeclinedByUser`
   waiver; an auto-runnable floor check simply runs. No new UI.

`CompletionGateReport::evaluate` receives the floor from
`CompletionGateService::report`, which reads `SessionStore::task_touched_paths`
(applied patch decisions plus command-changed `Due` paths over the continuation
chain) and `RepositoryMapStore::load_stored_map`, then calls
`verification::floor_checks`. The merge is by `CheckId`, so a plan check identical
to a floor check produces one reason or waiver. A floor check with no `CheckRun`
and no plan step to defer on is `RequiredCheckMissing`; a failed or skipped
`CheckRun` keeps the existing reason codes.

This reverses Task 5's note that discovered checks are not gate obligations. The
evidence lives in the same two stores: a loop-run check is a `CheckRun` keyed by
`CheckId`, and a deferred plan check is the step's `CommandExit`. The floor adds
no third store and runs no command.

Regression tests were written first and recorded failing in
`verification_loop`: `a_changed_source_without_a_plan_check_requires_the_discovered_floor_check`
(`left: Complete, right: Incomplete`),
`a_member_change_puts_the_workspace_root_check_in_the_floor` (same),
`declining_a_floor_check_is_a_named_waiver_and_the_task_completes`
(`waivers: []`, `left: 0, right: 1`),
`two_checks_needing_approval_produce_one_batch_and_one_resume` (restored waiver,
same failure) and `a_failed_verification_is_not_continued` (restored
`RequiredCheckFailed`, report held only `OpenPlanSteps`). `plan_turn`
`a_trivial_plan_check_does_not_replace_the_discovered_floor` failed because the
report named only the plan's `ls`; `floor_checks_are_baselined_at_task_start_and_a_later_root_is_uncertain`
failed with empty baselines; `a_floor_check_for_a_root_first_touched_later_is_uncertain_not_pre_existing`
failed with the later root recorded as `Failed`. All pass now. The
identical-identity, documentation-only and auto-run cases are guards.

### Review fix 2026-11-06: per-change checks are the owning root (R16)

Review fix R16 (decision D2 piece 2) split spec 23 §5.2's selection. Per-change
verification now selects only `RepositoryMap::root_for_path`; the completion
floor here is unchanged and still uses the owning **and enclosing** roots. The
split is structural, not a shared path: `verification::select_roots` is the
per-change set, `select_floor_roots` and `floor_checks` are the wider floor, and
`BaselineCoordinator::prepare` still merges the plan's required checks with the
floor, so R14's task-start baselines keep covering an enclosing root.

A verification whose plan has no open or blocked primary step is a completion
run: it selects the floor roots and records their checks through the ordinary
approval and command-policy path. At completion, an answered turn records a due
for the floor checks that are missing or stale and are not plan-declared,
regardless of any other reason, and runs only those before the answer becomes
final; a plan-declared check is never auto-run and never suppresses the floor.
A floor check is also satisfied by a current `CommandExit` of the identical
command and directory anywhere in the plan, using the same matcher and R4
currentness rule as the deferred path. The gate still runs nothing itself. R16's
orchestrator test
`plan_turn::a_member_change_runs_only_the_members_check_per_change_and_the_root_check_at_completion`
drives a member patch (member check per change), the workspace root at
completion (`new_regression` from the task-start baseline), the repair turn and
the passing rerun. The follow-up tests
`a_plan_declared_member_check_does_not_hide_the_missing_workspace_root_floor`,
`a_missing_floor_check_runs_even_when_a_plan_check_is_also_missing` and
`a_model_run_floor_command_satisfies_the_floor_without_an_automatic_run` pin that
the floor is always satisfiable and that a model-run floor command avoids a
second automatic run.

### Review fix 2026-11-06: a declined deferred check is a named waiver (R8)

The review of specs 101–105 found that the deferred path above had no waiver.
A required check deferred to its plan step runs as an ordinary `run_command`, so
it produces no `CheckRun`; the gate's only waiver source was
`SkipReason::DeclinedByUser` on a `CheckRun`. When the user declined the
command, nothing the gate read changed and the task stayed `RequiredCheckMissing`
forever — the user could not complete the task or remove the check after
approval.

The explicit decline is now durable plan evidence. `Evidence::CommandDeclined`
(on the in-progress plan step, attached in observation order like any other
evidence) carries the proposal id, the command hash and the repository-relative
working directory, normalized exactly as `CommandExit` records it. Only the
decline branch of the command-approval route writes it: a policy refusal or an
approval refusal, a timeout, and a model that never requests the command record
nothing. It is neutral for step status; declining a check neither fails the step
nor claims it passed.

The gate reads both kinds with one function, `matching_command_evidence`, which
keeps the full `CheckId` identity (command hash plus normalized directory). The
latest matching command evidence decides: a later approved run of the same check
supersedes an earlier decline (pass completes, fail reports
`RequiredCheckFailed`), and a decline with no later run is the named
`CompletionWaiver` shown with the check's label. A plan-declared check reads its
own step; the R16 floor reads anywhere in the plan, so declining a floor check
the model ran itself is the same waiver. `floor_checks_needing_verification`
treats a current decline as handled, so the answer-time completion run does not
ask again for a check the user just waived.

Regression tests were written first in
`crates/workspace-engine/tests/deferred_check_waiver.rs`.
`declining_the_deferred_required_check_is_a_named_waiver_and_completes` failed
with `left: Incomplete, right: Complete` and the report
`CompletionGateReport { eligible: false, reasons: [RequiredCheckMissing], waivers: [] }`;
`a_declined_check_waiver_names_the_check_and_is_never_a_pass` failed with
`left: 0, right: 1` waivers; `the_decline_is_durable_plan_evidence` found the
closed step's evidence empty; and
`declining_a_model_run_floor_check_is_a_named_waiver_and_completes` left the
task `WaitingForApproval` because the missing floor check started another
answer-time run. All pass now.
`declining_an_unrelated_command_is_not_a_waiver`,
`declining_the_same_command_in_another_directory_is_not_a_waiver`,
`a_later_approved_run_decides_instead_of_the_decline_when_it_passes` and
`a_later_approved_run_decides_instead_of_the_decline_when_it_fails` are the
guards; the two identity counters already passed before the fix because a
missing waiver and a missing result share one outcome.

## Review fix 2026-11-06: two patches in one step (R11)

Review fix R11 found that "two patches in one implementation step produce one
verification" was only covered by a unit test of
`TaskPlan::mutation_generation`. The end-to-end behaviour is two things, and the
old requirement 9 conflated them:

1. **When verification runs.** Each applied patch records a `verification_due`
   immediately; the step's close is not when verification starts. Deferring it
   to the step's last mutation would withhold the repair feedback the loop
   depends on, so the behaviour is deliberately per applied mutation.
2. **When a check is skipped.** Within a primary step, `run_verification_checks`
   skips a check that already passed at the current generation and whose
   recorded validated files still hash the same. A second patch to a *different*
   file therefore runs the check once for the batch. A second patch to the
   *same* file makes the earlier run stale, so the check runs again.

The review's same-file finding is correct, but the re-run is required, not a
bug: requirement 5 forbids reporting evidence current for a file that changed
after it ran. The chosen fix is therefore (B), correcting requirement 9 and the
acceptance text rather than changing the code. Deferring verification to the
step's close (option A) would delay repair feedback and would still have to
rerun a check whenever a later patch touched a validated file.

The end-to-end tests were written first, driven through the orchestrator with
`MockModelAdapter`, the ordinary plan/command approvals and the pre-apply
baseline gate, and `complete_step`: one implementation step (the final primary
step, so closing it does not advance the generation), two applied patches, a
verification after each, then `complete_step`.

- `plan_turn::two_patches_to_different_files_in_one_step_run_each_selected_check_once`
  passed with one `verification_check` run; the second patch left the check's
  recorded file unchanged, so the batch was not re-verified.
- `plan_turn::two_patches_to_the_same_file_in_one_step_run_the_check_once`
  was written first against the old "one verification for the whole step"
  wording and failed with `left: 2, right: 1`. It now asserts the correct
  behaviour as
  `two_patches_to_the_same_file_in_one_step_rerun_the_check`: the first run is
  stale after the second patch (asserted directly with `CheckRun::is_stale`
  over the new file hash), the check runs a second time, the second run records
  the new hash, and the task still completes.
