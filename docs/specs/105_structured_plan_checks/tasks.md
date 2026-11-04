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
| 4 · Mutation-batch generation and evidence staleness | Done 2026-11-04 | `TaskPlan::mutation_generation()` derives the mutation-batch generation from the primary (non-`verify_`) plan steps — the count that have left `Pending`, minus one — so the first step is generation 0 and each later step advances it. Resolved context.md §6: the generation is **derived, not stored** — the step boundaries are already durable in the log, and a stored counter would be a second source of truth that can disagree after recovery; a task with no plan or no primary step reads as generation 0, matching historical plans (backward compatibility falls out rather than needing a stored default). `VerificationEvent::Check` and `CheckRun` gained `generation: u32` (`#[serde(default)]`, omitted on the wire when 0 so a first-step check keeps the pre-spec event shape); the fold copies it, and `CheckRun::is_stale(current_hashes, current_generation)` now marks a run stale on a changed digest *or* an advanced generation, without deleting the run. `VerificationReport::build` derives the current generation from the plan it already receives, so every staleness site picks it up. `run_verification_checks` records the generation on each check event and implements requirement 9's "once per batch": a discovered check that already passed at the current generation and whose files still match is not re-run for another patch in the same primary step. The comparison is scoped to the task's own continuation chain rather than the whole session (two independent plans can both sit at generation 0, and an unrelated task's check must not suppress this one), and it is guarded to plan-scoped tasks so plan-less per-patch feedback is unchanged; a failed or stale run is never skipped, so repair still re-runs. Tests: `two_patches_in_one_implementation_step_are_one_generation`, `closing_the_final_check_step_records_the_current_generation`, `a_mutation_or_an_advanced_generation_makes_a_check_stale_without_deleting_history`, and the updated `events_serialise_in_the_shape_the_log_holds` (generation 0 stays off the wire). Deviation from the written plan: the plan named `CheckOutcome::Ran.file_hashes`; the digest actually lives on `CheckRun`/`VerificationEvent::Check`, which is where `generation` now lives too, and the free `is_stale` became `CheckRun::is_stale` so the "once per batch" filter can reuse the same decision. Scoped: `cargo nextest run -p workspace-engine --test verification_loop --test task_segments --test within_turn_request_preparation` (127 passed, 0 skipped); `cargo clippy -p workspace-engine --all-targets --locked -- -D warnings` clean; `cargo fmt --all -- --check` clean; `npm run specs:check -- --strict` passed. |
| 5 · Completion gate over structured required checks | Done 2026-11-04 | `CompletionGateReport::evaluate` (`completion.rs`) no longer derives required checks from `verification.selected`; it iterates the plan's primary steps, maps each `required: true` `PlannedCheck` to `CheckId::new(working_directory, command)`, and reads evidence from one of two existing stores — never a third. A non-deferred check that ran through the loop has a `CheckRun` in `verification.checks`; currentness is `CheckRun::is_stale(current_hashes, current_generation)` (generation is now computed directly as `plan.mutation_generation()`, and the `VerificationReport::build` call was dropped from `evaluate`). A deferred check that Task 2 removed from the selection was run by the worker as an ordinary command, which left an `Evidence::CommandExit` on the step; the gate matches the latest `CommandExit` whose `command_hash == sha256(command)` and reads currentness from the step's position (step terminal and no later primary step left `Pending`). The `CheckRun` is authoritative when both exist. The passed/waived/failed/missing/stale/skipped matrix and all `required_check_*` codes are unchanged; a declined check is still a named `CompletionWaiver`, but only for a loop-run check (a deferred check runs as a command and never produces a `SkipReason`). Discovered checks that are not on the plan are no longer gate obligations — their failure still settles `failed` through the verification settlement while the gate report names only the open plan work (spec 105 requirement 6 + non-goal 3). Tests: the `closed_task_completion` reason-matrix test was re-pointed at structured required checks (six terminal primary steps at generation 5, stale via file hash), four deferred/command-path tests added (`a_deferred_required_check_passes_through_its_step_command`, `..._fails_...`, `..._without_a_command_is_missing`, `..._is_stale_when_a_later_step_begins`), and `completion_settlement`'s `a_crashed_failed_verification_settles_failed_with_its_report` plus `verification_loop`'s `a_failed_verification_is_not_continued` and `two_checks_needing_approval_produce_one_batch_and_one_resume` were re-pointed to the new no-discovered-check-obligation behaviour. Scoped: `cargo fmt --all -- --check`, `cargo clippy -p workspace-engine --all-targets --locked -- -D warnings`, `cargo nextest run -p workspace-engine --test closed_task_completion --test completion_settlement --test verification_loop --test task_control` (126 passed, 0 skipped), `npm run specs:check -- --strict`. |
| 6 · Recovery, continuation, documentation and close-out | Done 2026-11-04 | Closed requirements 3, 8 and 10 with three focused tests, wrote the user-facing docs and a deterministic scenario, and reconciled the spec records. Requirement 3 (plan approval is not command approval): `approving_a_plan_with_a_required_check_runs_no_command_and_the_check_still_needs_approval` (plan_turn.rs) proposes a plan carrying a `required: true` `cargo test`, approves it through the orchestrator, then runs the check as a `run_command` — the result is a `command_proposal` with status `WaitingForApproval`, and the settled plan still carries the check, proving the approval ran nothing and the check still follows command policy. Requirement 8 (repository text cannot auto-approve): `a_proposed_check_never_carries_an_approval_field` (chat.rs) proves a `propose_plan` `check` object smuggling `"approved"`/`"trusted"` keys still parses to a plain `PlannedCheck` with no authority bit, and the tool schema exposes no approval field. Requirement 10 (recovery/continuation): `a_prose_only_plan_invents_no_required_check_for_the_gate` (closed_task_completion.rs) proves the gate reads structured checks, never step prose — a step whose text names `cargo test` but carries no `check` is eligible — while the existing continuation test already pins that the gate reads the carried plan's structured checks through `resume_plan`, and `a_pre_spec_plan_with_no_check_field_replays_with_none` pins the historical no-invention load. Docs: `USER_GUIDE.md` gained the required-checks review-card paragraph, the completion-obligation dispositions (passed/failed/stale/waived), and the own-approval note; `TROUBLESHOOTING.md` gained how a required check reads as stale or missing and the durable plan/verification evidence to inspect. Eval: new deterministic scenario `approved_plan_required_check` (propose a plan carrying a check, approve it, complete the steps without running the check, assert `final_task_status = "incomplete"`); `cargo run -p eval-harness -- run --tier deterministic` passes (exit 0, scenario `pass`). Scoped checks passed locally: `scripts/quality-gate.sh fmt node lint typos deny`, `cargo clippy -p workspace-engine -p desktop-shell -p eval-harness --all-targets --locked -- -D warnings`, `cargo nextest run -p workspace-engine --test closed_task_completion --test completion_settlement --test verification_loop --test plan --test plan_turn`, the deterministic tier, and `npm run specs:check -- --strict && git diff --check`. Workspace-wide clippy and tests are left to CI; the full gate is not claimed. |

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
