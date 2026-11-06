# Spec 105 tasks

**Started:** 2026-11-03 — **Done:** 2026-11-04

Implementation plan for [`proposal.md`](proposal.md). The design and the
planning corrections live there and in [`context.md`](context.md).

## Global constraints

- The engine owns the structured-check model, the deferral rule, the completion
  obligation, the mutation-batch generation and the staleness decision. The
  shell and browser transport and edit the proposed checks but never decide
  which checks are required or whether a task may complete.
- A structured check is authoritative only after the plan is approved. A
  `propose_plan` call is model output; it cannot authorise a command. Every
  check still runs through command policy and its ordinary approval route.
- Matching is by `CheckId` identity (working directory + command), never by
  parsing plan prose. Historical prose-only plans load with no structured check
  and invent nothing.
- `PlanStep.check` is `Option<PlannedCheck>` with `#[serde(default)]`; historical
  plan JSON and replayed plans stay readable. The settled plan stored on
  `plan_approved` records exactly what the user approved, edits included.
- The completion gate may stop or continue work; it cannot approve anything.
- Each task is a fresh agent session in the normal case. Scoped checks run per
  task; the workspace-wide clippy and test suite remain for CI under `AGENTS.md`.

| Task | State | Durable result |
|---|---|---|
| 1 · Structured check model, schema, parse and replay | Done 2026-11-03 | Versioned `PlannedCheck` (`version: u32` = `PLANNED_CHECK_VERSION`, plus `command`, repository-relative `working_directory` with empty = root, `purpose`, `required: bool`) in `plan.rs`, exported from `lib.rs`. `PlanStep.check: Option<PlannedCheck>` carries `#[serde(default, skip_serializing_if = "Option::is_none")]` so historical plan JSON and plans whose steps carry no check keep the pre-spec wire shape. `propose_plan_tool_definition`'s step schema gains an optional `check` object, and `tool_action_from_call` parses it into a `PlannedCheck` (a check without a `command` is dropped; `working_directory` defaults to `""`, `required` to `true`), which the `ProposePlan` dispatch then copies onto the built `PlanStep`. `validate_plan_structure` / `validate_plan_approval` already accept the field without re-validating a check's command — step content is deliberately not re-validated because the user may edit it — so no session.rs change was needed. The remaining `PlanStep { .. }` literals across workspace-engine and desktop-shell gained `check: None`. Tests: a plan with a structured check round-trips (camelCase wire), a step without a check omits the `check` key, a replayed pre-spec `plan_created`/`plan_approved` loads with `None`, and two parse tests (`propose_plan_parses_a_structured_check`, `propose_plan_drops_a_check_without_a_command_and_defaults_missing_fields`). |
| 2 · Structured deferral in the verification loop | Done 2026-11-03 | Retired `plan_has_open_check_command` / `plan_step_names_command` and replaced the filter in `run_verification_checks` with `plan_defers_check`: a `CheckId { working_directory, command }` equality between the selected check and a `required: true` `PlannedCheck` on an open (non-terminal, non-`verify_`) plan step. Only a `required: true` check defers; a different directory, a different command, or a `required: false` check still runs as immediate repair feedback. Historical prose-only plans carry no `PlannedCheck`, so they defer nothing — the deliberate #97 behaviour change (requirement 10), recorded in context.md §2. Rewrote `verification_defers_a_planned_final_check_without_two_steps_in_progress` to attach a structured `PlannedCheck` to the final step, keeping its single-in-progress replay assertions, and added `a_planned_check_with_a_different_directory_or_command_is_not_deferred` covering a different directory, a different command, and a `required: false` check. Scoped: `cargo nextest run -p workspace-engine --test verification_loop --test task_segments --test within_turn_request_preparation` (123 passed, 0 skipped). |
| 3 · Authoritative approval and the plan review card | Done 2026-11-03 | `PlanRevisionStep` gained a `check: PlanRevisionCheck` field, where the new `PlanRevisionCheck` enum (`Preserve`/`Remove`/`Set(PlannedCheck)`, `#[derive(Default)]` = `Preserve`) maps the revision wire's three `check` spellings: absent = preserve (a pre-check client, never a silent removal), `null` = remove, object = set. `apply_plan_revision` now applies the intent (`Preserve` leaves it, `Remove` clears it, `Set` replaces it). `parse_plan_revision` (desktop-shell) parses those three spellings. `plan_json` gained a per-step `check` field via `check_json` (command + workingDirectory + purpose + required + version, or `null`; version is transported, not displayed). `createPlanReview` (app.js) lists each check as `command (directory)` with add/change/remove controls, sends the full check object or `null` back through `parse_plan_revision`, and `revisionOrNull` compares checks by value. Tests: shell `the_plan_card_lists_a_step_check_command_and_directory` and `a_plan_revision_round_trips_a_structured_check`; engine `a_revision_carries_the_users_check_edits_into_the_settled_plan` (change/remove/add ride through `approve_plan_with_budget_and_plan`) and `the_deferral_follows_the_settled_plan_not_the_proposed_plan` (an approval that removes a proposed check stops the deferral — the deferral reads `read_task_plan`, never a fresh `propose_plan` call). Boundary recorded: the completion-gate half of "an unapproved plan gates nothing" is Task 5, not built here. Scoped: `cargo nextest run -p workspace-engine --test plan --test plan_turn --test verification_loop` (204 passed) and `cargo nextest run -p desktop-shell` (283 passed, 18 skipped); `node --check` + `npm run lint:web` clean. Follow-up (same day): the editor now also edits `purpose` and toggles `required` (a `required: false` check renders as "Check (optional)"), and `check_json` transports `version` so a re-approval of an unchanged check does not re-stamp it — the editor keeps the existing `version` on change and omits it on add, letting the server default a new check. Tests updated: `the_plan_card_lists_a_step_check_command_and_directory` asserts `version` present; `a_plan_revision_round_trips_a_structured_check` asserts `version` preserved and a `required: false` check parses; `a_revision_carries_the_users_check_edits_into_the_settled_plan` asserts a toggled `required: false` and changed `purpose`. Scoped: `cargo clippy -p workspace-engine -p desktop-shell`, `cargo nextest run -p desktop-shell` (283 passed), `node --check` + `npm run lint:web`, `npm run specs:check -- --strict`. |
| 4 · Mutation-batch generation and evidence staleness | Done 2026-11-04 | `TaskPlan::mutation_generation()` derives the mutation-batch generation from the primary (non-`verify_`) plan steps — the count that have left `Pending`, minus one — so the first step is generation 0 and each later step advances it. Resolved context.md §6: the generation is **derived, not stored** — the step boundaries are already durable in the log, and a stored counter would be a second source of truth that can disagree after recovery; a task with no plan or no primary step reads as generation 0, matching historical plans (backward compatibility falls out rather than needing a stored default). `VerificationEvent::Check` and `CheckRun` gained `generation: u32` (`#[serde(default)]`, omitted on the wire when 0 so a first-step check keeps the pre-spec event shape); the fold copies it, and `CheckRun::is_stale(current_hashes, current_generation)` now marks a run stale on a changed digest *or* an advanced generation, without deleting the run. `VerificationReport::build` derives the current generation from the plan it already receives, so every staleness site picks it up. `run_verification_checks` records the generation on each check event and implements requirement 9's "once per batch": a discovered check that already passed at the current generation and whose files still match is not re-run for another patch in the same primary step. The comparison is scoped to the task's own continuation chain rather than the whole session (two independent plans can both sit at generation 0, and an unrelated task's check must not suppress this one), and it is guarded to plan-scoped tasks so plan-less per-patch feedback is unchanged; a failed or stale run is never skipped, so repair still re-runs. Tests: `two_patches_in_one_implementation_step_are_one_generation`, `closing_the_final_check_step_records_the_current_generation`, `a_mutation_or_an_advanced_generation_makes_a_check_stale_without_deleting_history`, and the updated `events_serialise_in_the_shape_the_log_holds` (generation 0 stays off the wire). Deviation from the written plan: the plan named `CheckOutcome::Ran.file_hashes`; the digest actually lives on `CheckRun`/`VerificationEvent::Check`, which is where `generation` now lives too, and the free `is_stale` became `CheckRun::is_stale` so the "once per batch" filter can reuse the same decision. **2026-11-06 review fix R11:** the guarantee is a skip only while the earlier run's validated files still match. An end-to-end `plan_turn` test now proves two patches to different files in one final step run the check once, while a second patch to the same file makes the first run stale and it reruns (2 runs) — requirement 9 and the acceptance text were corrected to say so, rather than deferring verification to the step's close. Scoped: `cargo nextest run -p workspace-engine --test verification_loop --test task_segments --test within_turn_request_preparation` (127 passed, 0 skipped); `cargo clippy -p workspace-engine --all-targets --locked -- -D warnings` clean; `cargo fmt --all -- --check` clean; `npm run specs:check -- --strict` passed. |
| 5 · Completion gate over structured required checks | Done 2026-11-04 | `CompletionGateReport::evaluate` (`completion.rs`) no longer derives required checks from `verification.selected`; it iterates the plan's primary steps, maps each `required: true` `PlannedCheck` to `CheckId::new(working_directory, command)`, and reads evidence from one of two existing stores — never a third. A non-deferred check that ran through the loop has a `CheckRun` in `verification.checks`; currentness is `CheckRun::is_stale(current_hashes, current_generation)` (generation is now computed directly as `plan.mutation_generation()`, and the `VerificationReport::build` call was dropped from `evaluate`). A deferred check that Task 2 removed from the selection was run by the worker as an ordinary command, which left an `Evidence::CommandExit` on the step; the gate matches the latest `CommandExit` whose `command_hash == sha256(command)` and whose recorded `working_directory` equals the planned directory (directory added by the 2026-11-04 review fix R3; earlier events read as the root), and reads currentness from the step's position plus the absence of a later mutation (2026-11-05 review fix R4: step terminal, no later primary step left `Pending`, and no `PatchApplied` after the exit in durable plan order). The `CheckRun` is authoritative when both exist. The passed/waived/failed/missing/stale/skipped matrix and all `required_check_*` codes are unchanged; a declined check is still a named `CompletionWaiver`, but only for a loop-run check (a deferred check runs as a command and never produces a `SkipReason`). Discovered checks that are not on the plan are no longer gate obligations — their failure still settles `failed` through the verification settlement while the gate report names only the open plan work (spec 105 requirement 6 + non-goal 3). Tests: the `closed_task_completion` reason-matrix test was re-pointed at structured required checks (six terminal primary steps at generation 5, stale via file hash), four deferred/command-path tests added (`a_deferred_required_check_passes_through_its_step_command`, `..._fails_...`, `..._without_a_command_is_missing`, `..._is_stale_when_a_later_step_begins`), and `completion_settlement`'s `a_crashed_failed_verification_settles_failed_with_its_report` plus `verification_loop`'s `a_failed_verification_is_not_continued` and `two_checks_needing_approval_produce_one_batch_and_one_resume` were re-pointed to the new no-discovered-check-obligation behaviour. **Superseded 2026-11-06 by review fix R15:** decision D1 restored the discovered-check floor, so those two tests again assert the waived/failed discovered check; see the R15 note below. **Superseded 2026-11-06 by review fix R8:** an explicit user decline of the exact deferred command is also a named `CompletionWaiver`; the record and matching are described in the R8 note below. Scoped: `cargo fmt --all -- --check`, `cargo clippy -p workspace-engine --all-targets --locked -- -D warnings`, `cargo nextest run -p workspace-engine --test closed_task_completion --test completion_settlement --test verification_loop --test task_control` (126 passed, 0 skipped), `npm run specs:check -- --strict`. |
| 6 · Recovery, continuation, documentation and close-out | Done 2026-11-04 | Closed requirements 3, 8 and 10 with three focused tests, wrote the user-facing docs and a deterministic scenario, and reconciled the spec records. Requirement 3 (plan approval is not command approval): `approving_a_plan_with_a_required_check_runs_no_command_and_the_check_still_needs_approval` (plan_turn.rs) proposes a plan carrying a `required: true` `cargo test`, approves it through the orchestrator, then runs the check as a `run_command` — the result is a `command_proposal` with status `WaitingForApproval`, and the settled plan still carries the check, proving the approval ran nothing and the check still follows command policy. Requirement 8 (repository text cannot auto-approve): `a_proposed_check_never_carries_an_approval_field` (chat.rs) proves a `propose_plan` `check` object smuggling `"approved"`/`"trusted"` keys still parses to a plain `PlannedCheck` with no authority bit, and the tool schema exposes no approval field. Requirement 10 (recovery/continuation): `a_prose_only_plan_invents_no_required_check_for_the_gate` (closed_task_completion.rs) proves the gate reads structured checks, never step prose — a step whose text names `cargo test` but carries no `check` is eligible — while the existing continuation test already pins that the gate reads the carried plan's structured checks through `resume_plan`, and `a_pre_spec_plan_with_no_check_field_replays_with_none` pins the historical no-invention load. Docs: `USER_GUIDE.md` gained the required-checks review-card paragraph, the completion-obligation dispositions (passed/failed/stale/waived), and the own-approval note; `TROUBLESHOOTING.md` gained how a required check reads as stale or missing and the durable plan/verification evidence to inspect. Eval: new deterministic scenario `approved_plan_required_check` (propose a plan carrying a check, approve it, complete the steps without running the check, assert `final_task_status = "incomplete"`); `cargo run -p eval-harness -- run --tier deterministic` passes (exit 0, scenario `pass`). Scoped checks passed locally: `scripts/quality-gate.sh fmt node lint typos deny`, `cargo clippy -p workspace-engine -p desktop-shell -p eval-harness --all-targets --locked -- -D warnings`, `cargo nextest run -p workspace-engine --test closed_task_completion --test completion_settlement --test verification_loop --test plan --test plan_turn`, the deterministic tier, and `npm run specs:check -- --strict && git diff --check`. Workspace-wide clippy and tests are left to CI; the full gate is not claimed. |

### Review fix 2026-11-04 (R3): deferred check identity includes the directory

Task 5's deferred evidence match read the `CommandExit`'s command hash alone, so
the same command at the repository root satisfied a required check declared in
`crates/foo`. `Evidence::CommandExit` now records `working_directory`,
repository-relative and normalized by the same `relative_location` call that
builds a `CheckId` directory (`""` is the root); the gate matches the command
hash **and** the directory. The field is `#[serde(default,
skip_serializing_if = "String::is_empty")]`. An event written before it reads as
the root: it does not satisfy a subdirectory check and still satisfies a root
check, which is the pre-fix behaviour for a matching command, chosen so an
in-flight session's existing evidence stays meaningful. `step_is_current` was
left unchanged by R3; the R4 note below narrows when a deferred exit is current.

Regression tests were written first:
`a_required_check_in_a_subdirectory_is_missing_when_the_command_ran_at_the_root`
(plan_turn.rs) and
`a_command_exit_written_before_the_directory_was_recorded_does_not_satisfy_a_subdirectory_check`
(closed_task_completion.rs) both failed with `left: []`, `right:
[RequiredCheckMissing]`. `a_required_check_in_a_subdirectory_passes_when_the_command_ran_there`
drives the same real `run_command` path in `crates/foo`, asserts the recorded
directory, and passes; the root legacy case and the serde shape are pinned by
`a_command_exit_written_before_the_directory_was_recorded_still_satisfies_a_root_check`
and `a_command_exit_working_directory_is_omitted_at_the_root_and_read_back_elsewhere`.

Local checks 2026-11-04: `scripts/quality-gate.sh fmt node lint typos deny` all
passed (after `npm ci`); `cargo clippy -p workspace-engine -p desktop-shell -p
damaian-cli -p eval-harness --all-targets --locked -- -D warnings` passed;
`cargo nextest run -p workspace-engine -p desktop-shell -p damaian-cli -p
eval-harness --locked --no-fail-fast` ran 2917 passed, 50 skipped, 0 failed. No
loopback-port test failed.

Follow-up, same day: the plan side recorded the directory as the model or card
wrote it, so `"."`, `"./crates/foo/"` and similar produced a check that could
never match the normalized evidence or a discovered `CheckId`.
`normalize_planned_check_directory` (plan.rs, exported) now resolves the input
against the repository root and reuses the `relative_location` lexical
normalization that builds a `CheckId` directory. It is applied in the
`propose_plan` dispatch, the engine's plan revision, and the desktop revision
parser; the deferral match also normalizes the stored directory, so a legacy
`"./crates/foo"` defers `crates/foo`. A directory outside the repository is
dropped from a proposal or rejected in a revision with a clear error. Tests:
`a_plan_check_with_a_dot_directory_matches_the_command_run_at_the_root` and
`a_plan_check_with_a_relative_subdirectory_matches_the_command_run_there` failed
with the raw directory stored; `a_plan_check_with_a_directory_outside_the_repository_is_dropped`
failed with the check present; the desktop normalization test failed with
`left: [".", "./crates/foo/", "crates/foo"]`; the desktop rejection test parsed
the outside directory instead of refusing it. They pass now with the normalizer,
deferral and engine-revision tests above.

Follow-up checks 2026-11-04: `cargo fmt --all`; `cargo clippy -p
workspace-engine -p desktop-shell --all-targets --locked -- -D warnings` clean;
`cargo nextest run -p workspace-engine -p desktop-shell --locked --no-fail-fast`:
one run passed 2786, 48 skipped (1 leaky), while two later runs failed only the
known timing-sensitive `shell_path::a_shell_that_does_not_answer_is_killed_at_the_timeout`
(`shell_path.rs:160:10`, `called Result::unwrap() on an Err value: Os { code: 2,
kind: NotFound, message: "No such file or directory" }`); it passes alone. No
loopback-port test failed.

### Review fix 2026-11-05 (R4): deferred check evidence tracks later mutations

Task 5's deferred currentness rule (`step_is_current`) read only the check step's
position, so a `PatchApplied` recorded after the check's `CommandExit` on the
same step was invisible. The check looked current and the task could settle
`complete` on an unverified tree (review of specs 101–105, finding R4).

`completion.rs` now keeps the latest matching `CommandExit`'s index and decides
currentness with `deferred_evidence_is_current`: `step_is_current` **and** no
`PatchApplied` after that index in durable plan order, on the same step or a
later one. `step_is_current` is retained because it adds the batch boundary
`TaskPlan::mutation_generation` counts — a later primary step beginning advances
the generation even before it mutates — while `mutation_after` adds what the
boundary cannot see. There is no new counter and no wall-clock ordering. A
patch that is only proposed, declined or never applied records no
`PatchApplied`; a rerun of the exact check in the exact directory is the newest
matching exit. The `CheckRun` verification-loop path is unchanged.

Regression tests were written first.
`closed_task_completion::a_patch_applied_after_the_deferred_check_makes_its_evidence_stale`
failed with `assertion failed: !report.eligible`;
`plan_turn::a_patch_applied_after_the_deferred_check_makes_its_evidence_stale`
failed with `left: Complete`, `right: Incomplete` after the check ran, a real
patch was applied and the step closed. Both pass now. Guards:
`a_deferred_check_rerun_after_a_patch_is_current`,
`rerunning_the_deferred_check_after_the_patch_makes_it_current`,
`a_patch_applied_in_a_later_step_makes_the_check_stale`,
`a_patch_applied_in_a_later_step_makes_the_check_step_stale`, and
`a_patch_that_is_declined_or_never_applied_does_not_make_the_check_stale`.
The later-step guards already passed before the fix because `step_is_current`
covered that position; they keep it from regressing.

Local checks 2026-11-05: `scripts/quality-gate.sh fmt node lint typos deny` all
passed; scoped clippy for workspace-engine, desktop-shell, damaian-cli and
eval-harness clean; `cargo nextest run -p workspace-engine` passed 2508, 30
skipped, and `-p desktop-shell -p damaian-cli -p eval-harness` passed 424, 20
skipped. A combined four-crate run reproduced the known local timing failure
`shell_path::a_shell_that_does_not_answer_is_killed_at_the_timeout`
(`shell_path.rs:160:10`, `called Result::unwrap() on an Err value: Os { code: 2,
kind: NotFound, message: "No such file or directory" }`); it passes alone, as
recorded in the R3 note above. No loopback-port test failed. The workspace gate
is left to CI.

### Task 1: structured check model, schema, parse and replay

Add the typed model and make it survive a round trip without breaking anything
that already exists.

- `PlannedCheck` (versioned) with `command`, repository-relative
  `working_directory` (empty = root), `purpose`, and `required: bool`. It is a
  plan-level concept; do not fold `purpose` into the runtime `CommandPurpose`.
- `PlanStep.check: Option<PlannedCheck>` with `#[serde(default)]`
  (`plan.rs:116`).
- Extend `propose_plan_tool_definition`'s step schema (`chat.rs:16234`) with an
  optional `check` object, and the `ToolAction::ProposePlan` parse path that
  builds `PlanStep` from the model's objects.
- `validate_plan_structure` / `validate_plan_approval` (`session.rs:244`) accept
  the new field the way they accept retitle/reorder — they do not re-validate a
  check's command, because the user may edit it.
- Pin historical compatibility: a plan with no `check` field, and a replayed
  `plan_created`/`plan_approved` from before this spec, load with `None` and no
  invented check.

Scoped evidence: model + parse tests, historical-plan-load tests, and the
existing `plan`/`plan_turn`/`plan_budget_review` suites still passing.

### Task 2: structured deferral in the verification loop

Replace #97's prose matching with structured identity (requirement 4).

- Retire `plan_has_open_check_command` / `plan_step_names_command`
  (`chat.rs:14426`) and change the filter in `run_verification_checks`
  (`chat.rs:2904`) to defer a selected check whose `CheckId` matches a required
  `PlannedCheck` on an open (non-terminal) plan step. Smaller, different checks
  still run after intermediate mutations.
- The match is `CheckId { working_directory, command }` equality. A required
  final-suite check is deferred until its own plan step; an optional or
  non-required check is not a deferral target.
- Historical prose-only plans (no structured check) defer nothing, per
  requirement 10 — record this as the deliberate behaviour change from #97.
- Rewrite `verification_defers_a_planned_final_check_without_two_steps_in_progress`
  to attach a structured check to the final step, and keep the single-in-progress
  invariant assertions.

Scoped evidence: the rewritten deferral test, plus `verification_loop`,
`task_segments` and `within_turn_request_preparation` suites.

### Task 3: authoritative approval and the plan review card

Make structured checks bind only on approval, and let the user shape them
(requirements 2 and 7).

- The deferral and the completion gate read `PlannedCheck`s only from the
  persisted plan `read_task_plan` returns (which `plan_approved` settles), never
  from a fresh `propose_plan` call. Pin that a proposed-but-unapproved plan
  imposes no completion obligation.
- `plan_proposal_json` (`desktop-shell/lib.rs`) and `app.js createPlanReview`
  list each proposed check's command and directory on the review card, and the
  edit path carries check changes (add, remove, retitle, change directory)
  through `approve_plan_with_budget_and_plan` so the settled `TaskPlan` records
  exactly what the user approved.

Scoped evidence: shell route/JSON tests for the card payload and the edited
check round-trip, and an engine test that an unapproved plan gates nothing while
an approved one does.

### Task 4: mutation-batch generation and evidence staleness

Add the generation that makes evidence current or stale (requirements 5 and 9).

- Add a mutation-batch generation to task state. Applied patches and modifying
  commands within one primary plan step share one generation; it advances when
  that step closes or a new primary step begins.
- Check evidence records the generation and the changed-file digest it
  validated. A later generation or a changed digest marks the evidence stale
  without deleting history. Reuse `CheckOutcome::Ran.file_hashes` and
  `is_stale`; the generation is the new axis.
- Verification runs once per relevant mutation batch or final plan step, not
  once merely because the model emitted another patch in the same step.

Scoped evidence: engine tests proving two patches in one step are one batch,
that closing the final check step records current evidence, and that mutating a
validated file makes the required check stale.

### Task 5: completion gate over structured required checks

Point the gate at the approved plan's required checks (requirement 6).

- `CompletionGateReport::evaluate` (`completion.rs:206`) requires current
  terminal evidence for every `required: true` `PlannedCheck` on the approved
  plan, looked up by `CheckId` in `VerificationState.checks` rather than derived
  from `verification.selected`.
- Passed evidence satisfies it; failed, missing or stale blocks; a declined
  check is a named `CompletionWaiver` only. Keep `CompletionReasonCode`'s
  `required_check_*` codes, but source the check list from the plan.

Scoped evidence: the full reason matrix in `closed_task_completion` re-pointed at
structured checks, plus new tests for pass/fail/missing/stale/waived across a
structured required check.

### Task 6: recovery, continuation, documentation and close-out

Close the loop and the spec (requirements 3, 8 and 10).

- Pin that recovery and continuation retain structured identities and evidence
  without reinterpreting prose, and that a continued or recovered task's gate
  still reads the carried plan's required checks.
- Update `USER_GUIDE.md` (structured required checks on the review card, the
  completion obligation), `TROUBLESHOOTING.md` (a required check is stale or
  missing), and the plan tool description; add a deterministic eval scenario
  exercising an approved plan with a required check.
- Reconcile the spec status, `docs/specs/README.md`, and the `CHANGELOG.md`
  `Unreleased` entry, then record the scoped checks and the deterministic run.

Scoped evidence: deterministic eval pass, `npm run specs:check -- --strict`, and
the full scoped local list. Workspace-wide clippy and tests remain CI-only.

### Review fix 2026-11-06 (R15): the discovered-check floor

Decision D1 and review fix R15 restore the discovered checks Task 5 had removed
from the completion obligation. `CompletionGateReport::evaluate` now merges the
plan's `required: true` checks with a floor computed by
`verification::floor_checks`: the detected checks of the owning and enclosing
roots of every path in `SessionStore::task_touched_paths` (applied patch
decisions plus command-changed `Due` paths over the continuation chain),
excluding documentation-only work. `CompletionGateService::report` loads the
persisted repository map with `RepositoryMapStore::load_stored_map`, so the gate
stores no proposal and runs no command.

The merge is by `CheckId`: an identical plan check and floor check is one
obligation, a plan's own checks are added to the floor rather than replacing it,
and a floor check with no `CheckRun` and no plan step to defer on is
`RequiredCheckMissing`. Declining a floor check that needs approval is the
existing `DeclinedByUser` waiver; auto-runnable floor checks simply run. R14's
task-start baseline now also covers the floor for the first patch's touched
roots; a root first touched by a later patch remains `uncertain` (spec 102).

Regression tests were written first and recorded failing:
`verification_loop::a_changed_source_without_a_plan_check_requires_the_discovered_floor_check`
(`left: Complete, right: Incomplete`),
`a_member_change_puts_the_workspace_root_check_in_the_floor` (same),
`declining_a_floor_check_is_a_named_waiver_and_the_task_completes`
(`left: 0, right: 1` waivers), `two_checks_needing_approval_produce_one_batch_and_one_resume`
(restored waiver, same), `a_failed_verification_is_not_continued` (restored
`RequiredCheckFailed`, report held only `OpenPlanSteps`); `plan_turn::a_trivial_plan_check_does_not_replace_the_discovered_floor`
(report named only `ls`), `floor_checks_are_baselined_at_task_start_and_a_later_root_is_uncertain`
(empty baselines) and `a_floor_check_for_a_root_first_touched_later_is_uncertain_not_pre_existing`
(later root recorded as `Failed`); and
`completion_settlement::a_crashed_failed_verification_settles_failed_with_its_report`
(`assertion failed: !report.eligible`). All pass now.

Local checks were recorded in the R15 row of
`docs/PLAN/2026-11-04-review-fixes-specs-101-105.md`; workspace clippy and tests
remain CI-only.

### Review fix 2026-11-06 (R8): a declined deferred check is a named waiver

Task 5 left one gap: a required check deferred to its plan step runs as an
ordinary `run_command`, so it produces no `CheckRun` with
`SkipReason::DeclinedByUser`. If the user declined that command, the gate saw no
result and reported `RequiredCheckMissing` forever — the task could not complete
and the user could not remove the check after approval.

The decline is now durable plan evidence. `Evidence::CommandDeclined` carries
the proposal id, the command hash and the repository-relative working directory,
normalized exactly as `CommandExit` records them. It is written only in the
command-approval decline branch of `resume_after_command_decision`; a policy
refusal, an approval refusal, a timeout and a model that never requests the
command record nothing. The variant is neutral for step status: declining a
check neither fails the step nor claims it passed.

The gate matches declines and exits with one identity function,
`matching_command_evidence`, which keeps the full `CheckId` — command hash plus
normalized directory. The latest matching command evidence decides: the newest
exit after a decline supersedes it (a rerun that passes completes, one that
fails reports `RequiredCheckFailed`), while a decline with no later run is the
named `CompletionWaiver`. A plan-declared check reads its own step; the R16 floor
reads anywhere in the plan, so the same rule covers a floor check the model ran
itself and the user declined. `floor_checks_needing_verification` also treats a
current decline as handled, so the answer-time completion run does not ask for a
check the user has just waived.

Regression tests were written first in
`crates/workspace-engine/tests/deferred_check_waiver.rs` and recorded failing:
`declining_the_deferred_required_check_is_a_named_waiver_and_completes`
(`left: Incomplete, right: Complete`; report
`CompletionGateReport { eligible: false, reasons: [RequiredCheckMissing], waivers: [] }`),
`a_declined_check_waiver_names_the_check_and_is_never_a_pass` (`left: 0,
right: 1` waivers), `the_decline_is_durable_plan_evidence` (step evidence was
empty) and `declining_a_model_run_floor_check_is_a_named_waiver_and_completes`
(the answer-time floor run paused again on the same check, task
`WaitingForApproval`). The guards `declining_an_unrelated_command_is_not_a_waiver`,
`declining_the_same_command_in_another_directory_is_not_a_waiver`,
`a_later_approved_run_decides_instead_of_the_decline_when_it_passes` and
`..._when_it_fails` pass now; the two "not a waiver" cases already passed before
the fix because a missing waiver and a missing result share one outcome, and
they keep the identity rule from overcorrecting.
