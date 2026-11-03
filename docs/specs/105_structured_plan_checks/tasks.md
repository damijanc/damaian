# Spec 105 tasks

**Started:** 2026-11-03

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
| 2 · Structured deferral in the verification loop | Not started | |
| 3 · Authoritative approval and the plan review card | Not started | |
| 4 · Mutation-batch generation and evidence staleness | Not started | |
| 5 · Completion gate over structured required checks | Not started | |
| 6 · Recovery, continuation, documentation and close-out | Not started | |

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
