# Verification Loop Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Implements:** [`proposal.md`](proposal.md) in full · corrections and the
decisions it left open in [`context.md`](context.md)
**Started:** not yet (planned 2026-10-09)

## Progress

| Task | State | Notes |
|---|---|---|
| 1 · Verification types, the event fold and the report | Not started | No `chat.rs`. Pure module, the report's rules |
| 2 · Check selection by root | Not started | No `chat.rs`. Record the clippy/build gap in `OBSERVATIONS.md` |
| 3 · Verification due on a settled patch; session events | Not started | No `chat.rs`. `edit.rs`, `session.rs` |
| 4 · The verification turn: run, one card, record | Not started | **Edits `chat.rs`.** Read Global Constraints first |
| 5 · Repair, rerun, the limit and the failed status | Not started | **Edits `chat.rs`.** Run the deterministic eval tier |
| 6 · Browser behaviour verification | Not started | **Edits `chat.rs`** |
| 7 · Recovery of a due verification and a waiting card | Not started | `chat.rs` only if `PausedTurns` needs an accessor. Check the pairing rule first if it does |
| 8 · Shell API, the check card, the report, the CLI | Not started | |
| 9 · Eval scenarios | Not started | Measure the fixture's check before choosing it (`context.md` §11) |
| 10 · Docs, acceptance criteria, close the spec | Not started | Runs the full seven-command gate |

**Goal:** after a Code-mode patch is applied, Damaian discovers the checks of
every root the change touched, runs the approved ones, turns failures into
findings, lets the model repair within `agent_tool_retry_limit`, reruns, and
reports. No unrun check is ever reported as passed.

**Architecture:**
- **Pure core (Tasks 1–2).** A new `verification.rs` holds the check
  identity, the outcomes, the event types, `fold_verification`, the report
  builder and the root selection. No store, model or filesystem, so the
  report's rules are tested directly (`context.md` §2, §4, §8).
- **Trigger (Task 3).** The apply path records `verification_due` when a
  Code-mode patch settles with something applied (`context.md` §1). It lives
  in `edit.rs`, beside the `PatchApplied` evidence.
- **The loop (Tasks 4–6).** `ChatOrchestrator::verify_due` is a continuation
  turn. It proposes through `propose_detected_validations` on a map
  restricted to the selected roots, runs the auto-runnable checks, pauses
  once for the rest (`context.md` §3), records evidence, findings and events,
  hands failures to the model, and finishes with the report. A repair is a
  patch, so each repair attempt crosses a turn boundary (`context.md` §5).
- **Durability (Task 7).** Recovery reattaches the new pause kind and offers
  a due verification again (`context.md` §9).
- **Surfaces (Tasks 8–9).** The shell endpoints, one check card, the report
  card, a CLI `verify` subcommand, and two eval scenarios.

**Tech Stack:** Rust 2024, `serde`/`serde_json` (already dependencies of
`workspace-engine`), vanilla JS in `app.js`. No new dependencies.

## Global Constraints

Every task's requirements implicitly include this section.

- **Read [`context.md`](context.md) in full before Task 1.** Several of its
  decisions override the proposal's original wording:
  - §1: the apply path triggers the loop; a task is one turn, so
    verification is a continuation.
  - §2: selection by owning root and enclosing roots; restrict the map before
    proposing; `cargo test` is the only Cargo check detection finds.
  - §3: one card is a new pause kind, and every resume refuses another
    kind's pause.
  - §4: steps in the plan, everything else in session events, one pure fold.
  - §5: the attempt count is in the log; repair crosses turns.
  - §6: `stale`, not `stale_after_partial_acceptance`.

  Do not re-derive any of these from `proposal.md` alone.
- **#23 owns `chat.rs` while it runs.** Tasks 4, 5 and 6 edit `chat.rs`, and
  Task 7 does only if recovery needs a `PausedTurns` accessor. While any of
  those is in progress, #23 may pair with #25 and with **no other spec that
  edits `chat.rs`** (#32, #50, #56, #57). Before starting Task 4, read the
  progress table of every one of those specs that is in flight. If one has
  a `chat.rs` task in progress, wait, or agree with the user which track
  rebases.
- **Every cargo command runs with `CARGO_TARGET_DIR="$PWD/target"` from the
  worktree root.** Parallel worktrees must not share or thrash one target
  directory. Shell state does not persist between commands, so put it on
  every invocation, for example
  `CARGO_TARGET_DIR="$PWD/target" cargo clippy -p workspace-engine --all-targets --locked -- -D warnings`.
  The commands below omit it for brevity. Never omit it when running them.
- **An unrun check is never passed.** Passed means `CommandExit` with
  `exit_code == Some(0)`. Not `!is_error`, not the absence of a failure, and
  never `unwrap_or(0)`. Any code that decides a check's outcome is
  mutation-tested against `None`.
- **The approval boundary is unchanged.** Every check goes through
  `propose_command_at` and `run_proposal`. No task widens the allowlist,
  lowers a risk, or runs a declined or blocked check. `Allow Always` stays
  exact-command and root-qualified.
- **`propose_detected_validations` is the only discovery path.** No task
  adds a command to `detect_in` or proposes a check by any other route.
- **Existing behaviour without verification is unchanged.** A patch in Ask,
  Plan or Review mode, a patch with nothing applied, and a turn that proposes
  no patch behave exactly as before. Each task that touches those paths pins
  it with a test.
- **Spec 34's and spec 31's tests are the floor.**
  `tests/repository_config_trust.rs` and `tests/permission_profiles.rs` pass
  unchanged after every task. No task adds a config key, so neither file is
  edited.
- **New engine tests go in `crates/workspace-engine/tests/verification_loop.rs`**,
  which Task 1 creates. Fixtures that build an engine or an index set
  `enable_index_watcher: false`.
- **Tests never spawn a real login shell.** A test that needs a check to run
  sets `shell` to `/usr/bin/true` (passes) or `/usr/bin/false` (fails) and
  asserts on `CommandExecution`, as spec 24 did. Never write a fake
  executable into a fixture (`context.md` §11). A model-dependent test uses
  `MockModelAdapter` or `DAMAIAN_MOCK_MODEL_RESPONSE`.
- **Falsify every load-bearing test.** Break what it guards, confirm it fails
  for the stated reason, revert, and record the mutation in the progress row.
- **Scope per-task checks. Run the full seven-command gate from `AGENTS.md`
  once, in Task 10.** Task 5 also runs the deterministic eval tier, because it
  changes model-dependent workflow.
- **Never `git commit` unasked.** Each task ends by showing the change and the
  scoped check results, then asking. When asked, write one subject line with
  no body and no trailer. Never push.

## File Structure

| File | Change |
|---|---|
| `crates/workspace-engine/src/verification.rs` | New (Task 1): `CheckId`, `CheckOutcome`, `SkipReason`, `VerificationEvent`, `VerificationState`, `fold_verification`, `VerificationReport`. Selection (Task 2) |
| `crates/workspace-engine/src/repository_map.rs` | `RepositoryMap::restricted_to` (Task 2) |
| `crates/workspace-engine/src/edit.rs` | `verification_due` on a settled Code-mode patch (Task 3) |
| `crates/workspace-engine/src/session.rs` | Verification event append and read (Task 3) |
| `crates/workspace-engine/src/chat.rs` | **Tasks 4, 5, 6 only** (and 7 if needed): `verify_due`, `AgentCheckBatch`, the `"checks"` pause and `resume_after_check_decision`, repair, browser check |
| `crates/workspace-engine/src/recovery.rs` | The `"checks"` kind and due verifications (Task 7) |
| `crates/workspace-engine/src/lib.rs` | Module registration and re-exports (Tasks 1–4) |
| `crates/workspace-engine/tests/verification_loop.rs` | New (Task 1). Every later engine task adds to it |
| `crates/damaian-cli/src/main.rs` | `verify` subcommand (Task 8) |
| `crates/desktop-shell/src/lib.rs` | Verify, check-decision and report endpoints (Task 8) |
| `crates/desktop-shell/static/app.js`, `index.html`, `style.css`, `docs/ui-style-guide.html` | Check card, report card, stale marks (Task 8) |
| `crates/eval-harness/src/scenario.rs`, `runner.rs`, `scenarios/*.toml` | Verification options and two scenarios (Task 9) |
| `docs/specs/18_local_evaluation_harness/proposal.md` | The two scenarios (Task 9) |
| `docs/USER_GUIDE.md`, `docs/TROUBLESHOOTING.md` | Proposal §5.9 (Task 10) |

## Interface reference

Checked on 2026-10-09. Re-check a line before editing near it: other specs
edit these files.

- `ValidationOrchestrator` (`validation.rs:159`): `propose_command_at`
  (`:237`), `propose_detected_validations(repository_root, &RepositoryMap)`
  (`:273`), `run_proposal` (`:301`), `allow_command_always` (`:365`),
  `reject_proposal` (`:440`). `CommandProposal` (`:17`).
  `CommandStore::save_execution` (`:104`).
- `CommandExecution` (`command_runner.rs:17`): `exit_code: Option<i32>`
  (`:25`), `termination` (`:30`).
- `detect_in` (`command_policy.rs:308`): the five `package.json` script
  names, `test-shortcut`, and one command per other manifest.
  `allow_always_eligible` (`:368`).
- `RepositoryMap` (`repository_map.rs:270`), `ProjectRoot` (`:243`),
  `root_for_path` (`:525`), `RootOverride::Removed`.
  `WorkspaceEngine::repository_map(root)` returns a `MapLoad` and the map.
- `plan.rs`: `Evidence` (`:70`), `PlanStep` (`:100`), `StepStatus::Skipped`
  (`:21`), `status_from_evidence` (`:158`), `TaskPlan::report` (`:278`),
  `PlanReport::summary` (`:385`).
- `session.rs`: `TaskStatus` (`:55`, `Validating`, `Failed`), `session_mode`
  (`:752`), `resume_plan` (`:1274`), `append_step_evidence` (`:1303`),
  `read_task_plan` (`:1365`), `read_session_web_diagnostics` (`:1481`),
  `record_finding` (`:1508`), `append_session_event` (`:2023`, private).
- `edit.rs`: `apply_stored_patch` (`:560`), its mode check (`:579-580`), the
  `PatchApplied` append (`:657-676`), `reject_stored_patch_files` (`:759`).
  `ProposedFilePatch.status` (`patch_engine.rs:24`).
- `chat.rs`: `AgentCommandProposal` (`:277`), `ChatTurnResult` (`:340`),
  `TurnProposals` (`:379`), `resume_after_command_decision_with_options`
  (`:829`, the plan-review refusal at `:837-848`), `record_command_findings`
  (`:1327`), `record_findings` (`:1376`), `carry_plan_from_the_previous_turn`
  (`:1480`), `run_agentic_turn` (`:1513`), the patch stops (`:2508-2531`,
  `:2690-2716`), `PendingChatTurn` (`:3540`), `PausedTurns` (`:3612`),
  `tool_budget_exhausted_response` (`:4759`), `run_and_record_web_diagnostic`
  (`:574`).
- `finding/repair.rs`: `RepairRequest` (`:46`), `render` (`:99`).
  `finding.rs`: `resolve_finding_path` (`:268`), `findings_from_execution`
  (`:347`).
- `recovery.rs`: the `PendingApprovalRef.kind` match (`:314-336`).
- Config: `agent_tool_retry_limit` (`config.rs:316`, default 2, Preference at
  `:2296`), `require_approval_for_all_commands` (`:261`).
- Shell: `/api/apply-patch` (`desktop-shell/src/lib.rs:1172`),
  `/api/reject-patch-files`, `/api/rollback-patch` (`:338`),
  `/api/resume-command-stream` (`:333`). `app.js`: `runApply`,
  `reportResolvedIfSettled` (`:6400`).
- Eval: `Scenario` (`eval-harness/src/scenario.rs:80`), the runner's apply
  (`runner.rs:331-336`), `base_config` (`:234`).

---

## Task 1: Verification types, the event fold and the report

**Requirements:** 7 and the "never represented as passed" rule. Acceptance
criteria "`Checks passed` contains only checks that ran and exited zero,
including `exit_code: None`", "every proposed check appears exactly once",
"no detected checks is distinct from checks passing", and "the summary line
never says complete when …". **Files:** create
`crates/workspace-engine/src/verification.rs` and
`crates/workspace-engine/tests/verification_loop.rs`; modify
`crates/workspace-engine/src/lib.rs`.

This task stands alone and changes no behaviour. Nothing outside tests calls
the new module yet. It is `pub`, so `clippy -D warnings` raises no dead-code
warning.

**Interfaces produced (later tasks rely on these names):**
- `pub struct CheckId { pub working_directory: String, pub command: String }`,
  repository-relative, `""` for the root. `Ord`, `Hash`, serde camelCase.
  `CheckId::label()` renders "`cargo test` (crates/engine)" or "`cargo test`
  (repository root)".
- `pub enum SkipReason { DeclinedByUser, Blocked { reason: String }, NoRunningTarget }`,
  serde `snake_case` tag `reason`, so the wire strings are `declined_by_user`
  and `no_running_target` as the proposal names them.
- `pub enum CheckOutcome { Ran { exit_code: Option<i32>, marker_id: String }, Skipped(SkipReason) }`.
- `pub enum VerificationOutcome { Passed, Failed, RepairLimitExhausted, NoRepairApplied, NoChecksAvailable, OnlyDocumentationChanged }`.
- `pub enum VerificationEvent`, serde tag `kind`:
  `Due { task_id, patch_id }`, `Started { task_id, attempt: u32, checks: Vec<CheckId> }`,
  `Check { task_id, check: CheckId, outcome: CheckOutcome, file_hashes: BTreeMap<String, String> }`,
  `Finished { task_id, outcome: VerificationOutcome }`.
- `pub struct VerificationState`, and
  `pub fn fold_verification<'a>(events: impl IntoIterator<Item = &'a VerificationEvent>) -> VerificationState`.
  The state holds: `due` (a `Due` with no later `Finished`), `attempt`, the
  selected checks in first-selected order, the **latest** outcome and hashes
  per check, the checks that passed at an earlier attempt and fail at a later
  one (`regressions`), and the last `Finished` outcome.
- `pub struct VerificationReport { files_changed, passed, failed, skipped, stale, behaviour_verified, unverified_steps, regressions, outcome, due }`
  and `VerificationReport::build(state, plan: &TaskPlan, files_changed: &[String], current_hashes: &BTreeMap<String, String>) -> Self`,
  plus `summary(&self, plan_report: &PlanReport) -> String` and
  `render(&self) -> String` (proposal §5.7's layout, paths as plain
  repository-relative text).

- [ ] **Step 1: Write the failing tests** in `tests/verification_loop.rs`.
  Build states from event vectors, never from a store. At least:
  1. `a_check_passes_only_on_exit_code_zero`: three checks with `Some(0)`,
     `Some(1)` and `None`. Only the first is in `passed`. `None` is in
     `failed` with the reason "killed or signalled", not in `passed` and not
     missing.
  2. `every_selected_check_is_in_exactly_one_section`: a state with a pass,
     a fail, a decline, a block, a `no_running_target` and a stale check.
     Assert the union of the four sections equals the selected set, and
     that their sizes sum to its size.
  3. `no_checks_available_is_not_checks_passed`: a finished state with
     `NoChecksAvailable` has empty `passed`, its `render()` says
     "No checks were available", and its `summary()` does not say
     "passed".
  4. `the_latest_attempt_decides_a_check`: a check failing at attempt 0 and
     passing at attempt 1 is passed. A check passing at 0 and failing at 1 is
     failed **and** in `regressions`.
  5. `a_check_whose_files_changed_is_stale_not_passed`: a passing check whose
     recorded hash of `src/a.rs` differs from `current_hashes` is in `stale`,
     not `passed`. A deleted file (absent from `current_hashes`) is a
     mismatch too.
  6. `the_summary_never_says_complete_when_anything_is_wrong`: one case each
     for a failed check, `RepairLimitExhausted`, `NoRepairApplied` and a
     state still `due`. Each with a `PlanReport` whose `is_complete` is true,
     so the verification rule is what withholds the word.
  7. `the_fold_does_not_depend_on_how_events_were_batched`: folding all
     events at once equals folding a prefix and then the rest from the log
     re-read. This pins that the fold has no hidden state outside the events.
  8. `events_serialise_in_the_shape_the_log_holds`: one `serde_json::json!`
     pin per variant, including `"reason":"declined_by_user"`.

- [ ] **Step 2: Run them and confirm they fail to compile**

  Run: `cargo nextest run -p workspace-engine --test verification_loop`

- [ ] **Step 3: Implement `verification.rs`** with the types above, and
  register it in `lib.rs` (`pub mod verification;`, re-exports, then
  `cargo fmt --all`). The pass rule is one function,
  `fn passed(outcome: &CheckOutcome) -> bool { matches!(outcome, CheckOutcome::Ran { exit_code: Some(0), .. }) }`,
  with a comment saying why `None` is not zero. `build` computes staleness
  before sorting a ran check into passed or failed: a stale check is in
  `stale` whatever its exit code was.

- [ ] **Step 4: Run the tests and confirm they pass.** Do not weaken an
  assertion to make one pass.

- [ ] **Step 5: Mutation-test.** Do each, confirm the named test fails,
  revert, and record all of them:
  1. `passed` as `!matches!(exit_code, Some(c) if c != 0)` (lets `None`
     through). Test 1 fails.
  2. A skipped check left out of `build`'s sections. Test 2 fails.
  3. `NoChecksAvailable` rendered as an empty "Checks passed" line. Test 3
     fails.
  4. First outcome per check wins instead of the latest. Test 4 fails.
  5. Staleness checked after the pass sort. Test 5 fails.
  6. `summary` ignoring `due`. Test 6 fails.

- [ ] **Step 6: Scoped checks**

  ```bash
  cargo fmt --all -- --check
  cargo clippy -p workspace-engine --all-targets --locked -- -D warnings
  typos
  ```

- [ ] **Step 7: Update this file's Task 1 row, then show the change and the
  check results and ask before committing.** Suggested subject:
  `Add verification types and an honest completion report`.

## Task 2: Check selection by root

**Requirements:** 2, and proposal §5.2. Acceptance criteria "a change in a
sub-root runs that root's checks and its enclosing roots' checks, each in its
own working directory, and no other root's" and "a change to documentation
only runs no checks and says so". **Files:** `verification.rs`,
`repository_map.rs`, `lib.rs`, `tests/verification_loop.rs`. Implements
`context.md` §2. No `chat.rs`.

- `pub fn is_documentation(path: &str) -> bool`: extension `md`, `txt`,
  `rst` or `adoc`, case-insensitive. Not `detect_language`.
- `pub fn select_roots(map: &RepositoryMap, changed: &[String]) -> Vec<String>`:
  for each non-documentation path, `root_for_path` and every non-removed
  enclosing root. Sorted, de-duplicated. Empty when only documentation
  changed.
- `RepositoryMap::restricted_to(&self, roots: &[String]) -> RepositoryMap`:
  a clone keeping only those roots, and within each, dropping
  `test-shortcut` when the root also has `npm run test`. Everything else,
  `excluded` and the fingerprint included, is unchanged. It is never
  persisted.
- `pub fn propose_checks(orchestrator, repository_root, map, changed) -> Result<Selection>`
  where `Selection` is `OnlyDocumentation`, `NoChecksAvailable { roots }` or
  `Proposed(Vec<(CheckId, CommandProposal)>)`. It calls
  `propose_detected_validations(repository_root, &map.restricted_to(..))`
  and nothing else to discover.

Tests (fixture: a temp repository with a Cargo workspace root, members
`crates/engine` and `crates/cli`, `packages/web` with a `package.json`
naming `test`, and `docs/guide.md`; the map built through
`WorkspaceEngine::repository_map` with a scratch `data_dir`):
- A change to `crates/engine/src/lib.rs` selects `crates/engine` and `""`,
  not `crates/cli` or `packages/web`. The proposals' working directories end
  in `crates/engine` and the repository root.
- A change to `packages/web/src/index.ts` selects `packages/web` and `""`.
  Its proposals hold `npm run test` and not `npm test`.
- `docs/guide.md` alone selects nothing (`OnlyDocumentation`). `Makefile`
  and `build.sh` are not documentation.
- A removed root is never selected; its files select their nearest
  non-removed root.
- Only the selected roots' proposals are stored: count the proposal files in
  the command store before and after.
- A repository whose selected roots detect nothing gives
  `NoChecksAvailable`, not an empty `Proposed`.

Mutations to record: select only the owning root (the enclosing-root test
fails); treat `text`-language files as documentation (the `Makefile` case
fails); propose on the unrestricted map (the stored-proposal count fails);
drop `test-shortcut` across roots rather than within one.

When it lands, record in `docs/PLAN/OBSERVATIONS.md` (if present) that
detection finds no `cargo clippy` or `cargo build` and misses `lint:web`, so
the loop cannot check either (`context.md` §2). Do not fix it here.

## Task 3: Verification due on a settled patch; session events

**Requirements:** 1, and the trigger half of `context.md` §1. Acceptance
criteria "the loop does not run in Ask, Plan or Review mode" (the trigger
half) and "verification state survives a restart" (the log half).
**Files:** `edit.rs`, `session.rs`, `lib.rs`, `tests/verification_loop.rs`.
No `chat.rs`.

- `SessionStore::append_verification_event(session_id, &VerificationEvent)`
  and `read_verification_events(session_id, task_ids: &[&str]) -> Result<Vec<VerificationEvent>>`.
  One session-event kind per variant (`verification_due`,
  `verification_started`, `verification_check`, `verification_finished`).
  The reader takes a task chain, because each verification turn is a new
  task carrying the plan (`context.md` §1).
- `apply_stored_patch` and `reject_stored_patch_files`: after their existing
  work, if the patch has a `task_id` and a session, no file's `status` is
  still pending, at least one file was applied, and
  `session_mode(session) == Code`, append `Due { task_id, patch_id }`.
  Append it once per patch: a second settle of the same patch (an apply after
  a reject of the rest) does not append again.
- Requirement 1: `Due` is appended after the `PatchApplied` evidence, whose
  applied hashes are what the loop later compares against.

Tests:
- Applying every file of a Code-mode patch appends one `Due`.
- Applying two of three files appends nothing. Rejecting the third then
  appends one `Due`.
- Rejecting every file appends nothing.
- A patch in an Ask-mode session never gets a `Due` (it cannot be applied:
  pin the refusal and the absence of the event together).
- A patch with no session (`session_id` empty) appends nothing and still
  applies.
- Events round-trip through a fresh `SessionStore` on the same data dir, and
  `fold_verification` over them reports `due`.

Mutations to record: append on the first apply rather than on settle; skip
the mode check; append for an all-rejected patch.

## Task 4: The verification turn: run, one card, record

**Requirements:** 2, 3 and 4, and `context.md` §3–§4. Acceptance criteria
"a check the user declined is skipped with `declined_by_user` and the task
continues", "a blocked check is skipped with the policy reason and never
retried", "multiple checks requiring approval produce one approval card",
"`require_approval_for_all_commands` is honoured for read-only checks", and
"the loop does not run in Ask, Plan or Review mode". **Files:** `chat.rs`,
`lib.rs`, `tests/verification_loop.rs`. **Edits `chat.rs`: read Global
Constraints first.**

- `ChatOrchestrator::verify_due(session_id, model_adapter, sink) -> Result<ChatTurnResult>`.
  Refuses with `InvalidInput` when nothing is due or the session is not in
  Code mode now. Otherwise: start a new task, carry the plan
  (`resume_plan`), set `TaskStatus::Validating`, read the changed files from
  the plan's `PatchApplied` evidence, re-hash them (requirement 1), select
  and propose (Task 2), append `Started`, add one plan step per check, and:
  - run each auto-runnable check with `run_proposal`, recording
    `CommandExit` evidence, findings through `record_command_findings`, and
    a `Check` event with the current hashes;
  - record each blocked check as a `Skipped` step and a
    `Check { Skipped(Blocked { reason }) }` event. Never run it;
  - if any check needs approval, pause once with `AgentCheckBatch` and the
    `"checks"` pending kind;
  - otherwise finish (Task 5 adds what happens on a failure). With every
    check passed, append `Finished { Passed }` and respond with the report.
  `OnlyDocumentationChanged` and `NoChecksAvailable` finish at once with
  their outcome and report.
- `resume_after_check_decision(batch_id, decisions, approved_by, ..)`: runs
  approved checks, records declined ones as skipped with `DeclinedByUser`
  (through `reject_proposal` too, so the command store agrees), then
  continues as above.
- Every resume entry point refuses another kind's pause (`context.md` §3):
  the command resume refuses a batch, and the check resume refuses a command
  and a plan review. Each refusal puts the paused turn back.
- `ChatTurnResult` gains `check_batch` and `verification_report:
  Option<VerificationReport>`. `TurnProposals` gains `checks`.

Tests (shell `/usr/bin/true` or `/usr/bin/false`; `MockModelAdapter`, which
this task does not yet call for a passing run):
- All checks auto-runnable and passing: no card, `Finished { Passed }`, plan
  steps completed with `CommandExit` `Some(0)`, report lists them as passed.
- Two checks needing approval produce **one** `check_batch` with both, and
  no `command_proposal`.
- Declining one and approving the other: the declined one is skipped with
  `declined_by_user`, the approved one ran, the turn continued to a report.
- A blocklisted check is skipped with its policy reason, never reaches the
  card, and is not in any `CommandExecution`.
- With `require_approval_for_all_commands`, a read-only check that would
  auto-run is on the card instead.
- `verify_due` in a session switched to Ask after the apply is refused, and
  nothing runs.
- Resuming a batch id through `resume_after_command_decision` is refused and
  the batch is still resumable through `resume_after_check_decision`. The
  reverse too.

Mutations to record: one card per check; run a declined check; let the
command resume accept a batch; skip the mode check in `verify_due`.

## Task 5: Repair, rerun, the limit and the failed status

**Requirements:** 4, 5 and 6, and `context.md` §5. Acceptance criteria "a task
that breaks a test reports the failure, attempts repair within
`agent_tool_retry_limit`, reruns, and reports", "exhausting the limit
produces a report stating what still fails, with status `failed`", "a repair
attempt with no applied patch stops the loop", "a repair that breaks a
previously passing check is detected by the regression rerun", and "the
summary never says complete when …". **Files:** `chat.rs`,
`tests/verification_loop.rs`. **Edits `chat.rs`.**

- On any failed check in a verification turn whose attempt is below
  `agent_tool_retry_limit`: render the failing checks' findings with
  `RepairRequest::render`, put it in front of the model as the repair
  request, and continue the agentic loop with the Code-mode tools. A
  proposed patch ends the turn as usual. When that patch settles, Task 3's
  `Due` makes attempt *n + 1* due.
- If the repair turn ends without a patch, append
  `Finished { NoRepairApplied }`, set `TaskStatus::Failed`, and respond with
  the report. A repair patch settled with nothing applied gets no `Due`
  (Task 3), so `verify_due` on that task records the same outcome when it is
  next asked.
- At the limit: `Finished { RepairLimitExhausted }`, `TaskStatus::Failed`,
  report leading with the failures.
- The rerun set for attempt *n + 1* is every check selected so far plus
  whatever the new changed files select (`context.md` §5). A check that had
  passed and now fails is a regression in the report.
- Reaching `agent_max_tool_rounds` or the task token ceiling in a repair turn
  stops it the way any turn stops, with the plan intact and nothing marked
  passed.

Tests (`MockModelAdapter` scripting the repair turns. The fixture's detected
check never really runs: its outcome is decided by `shell`, `/usr/bin/false`
to fail and `/usr/bin/true` to pass. To flip it between attempts, build a new
orchestrator over the same data dir with the other `shell`. That also proves
the attempt count comes from the log and not from memory. Never a real build):
- Fail, repair patch applied, rerun passes: `Finished { Passed }`, attempt 1,
  the report lists the check as passed.
- Fail on every attempt with `agent_tool_retry_limit = 2`: exactly two repair
  turns, `RepairLimitExhausted`, status `Failed`, summary without
  "complete".
- The repair turn answers without a patch: `NoRepairApplied`, one repair
  turn, not two.
- Check A fails, check B passes; the repair fixes A and breaks B: B is in
  `regressions` and the outcome is not `Passed`.
- The repair request names every finding id and holds no raw log beyond the
  findings' excerpts.

After the tests pass, run the deterministic eval tier
(`cargo run -p eval-harness -- run --tier deterministic`) and record that no
existing scenario changed.

Mutations to record: rerun only the failing check (the regression test
fails); keep repairing after a turn with no patch; count attempts in memory
rather than from the log (a test that rebuilds the orchestrator between
attempts fails); report `Passed` at the limit.

## Task 6: Browser behaviour verification

**Requirements:** 7's "behaviour verified through browser diagnostics", and
`context.md` §7. Acceptance criterion "a web verification with no running
target is skipped with `no_running_target`, never inferred as verified".
**Files:** `chat.rs`, `verification.rs` (a `CheckId` for the browser check,
whose `command` is the diagnostic and URL), `tests/verification_loop.rs`.
**Edits `chat.rs`.**

- When a changed path is a web asset and the session has a web diagnostic
  record for this task chain, rerun it through
  `run_and_record_web_diagnostic`, under spec 12's approval rules unchanged.
  Record findings as today. It is "Behaviour verified" only when it ran and
  recorded no `Error` finding.
- No record, or an unreachable page: `Skipped(NoRunningTarget)`.
- No web asset changed: no browser check at all, not a skip.

Tests use a stub `WebDiagnosticsRunnerHandle` (`set_web_diagnostics_runner`),
as spec 12's tests do: one reachable page with no error, one that throws,
one unreachable, and one session with no record.

Mutations to record: treat an unreachable page as verified; run a browser
check for a Rust-only change.

## Task 7: Recovery of a due verification and a waiting card

**Requirements:** acceptance criterion "verification state survives a
restart … a verification that was due, or a check card that was waiting,
is offered again", and `context.md` §9. **Files:** `recovery.rs`,
`tests/verification_loop.rs`, and `chat.rs` only if `PausedTurns` needs an
accessor for the batch (check the pairing rule first if it does).

- Recovery recognises `PendingApprovalRef.kind == "checks"` and reattaches
  the batch, rather than failing the task.
- A `Due` with no `Finished`, whose task is not running, is listed for the
  shell as a verification to offer. Nothing is run automatically.
- A verification turn interrupted mid-check (an `action_started` with no
  `action_finished`) is classified by spec 17's existing rules. Its check is
  not recorded as passed.

Tests: reopen the store after a batch pause and after a `Due`, and assert
what recovery reports. Kill a turn mid-check with the spec 17 helper and
assert the check's outcome is absent, not passed.

Mutations to record: drop the `"checks"` arm (the task is failed instead of
reattached); run a due verification on recovery.

## Task 8: Shell API, the check card, the report, the CLI

**Requirements:** the user-facing half of 3 and 7, and proposal §5.7's
clickable paths. **Files:** `desktop-shell/src/lib.rs`, `static/app.js`,
`index.html`, `style.css`, `docs/ui-style-guide.html`,
`damaian-cli/src/main.rs`.

- `POST /api/verify-stream` calls `verify_due` and streams like
  `/api/resume-command-stream`. `POST /api/resume-checks-stream` takes the
  batch id and one decision per proposal id.
- `app.js`: when a patch card reports the patch settled with something
  applied, and the session is in Code mode, call `/api/verify-stream`. The
  check card lists every check with its root, risk, and approve, decline and
  (where `allowAlways`) Allow Always per check, and one submit. The report
  card renders proposal §5.7's sections, with paths through the existing
  clickable-reference renderer and `stale` checks marked. A reattached batch
  and a due verification from recovery are shown like reattached approvals.
- Follow `docs/UI_STYLE_GUIDE.md` and add the two cards to
  `docs/ui-style-guide.html`.
- CLI: `damaian verify <repo> <session>` runs `verify_due` with the
  configured model, prints the report, and answers a check batch
  interactively, or declines it all with `--decline-checks`.

Tests: shell endpoint tests in `desktop-shell/src/lib.rs` beside the
apply-patch test. Verify the UI by hand per the desktop-shell rule: rebuild,
restart on a port other than 4765 with a scratch `DAMAIAN_DATA_DIR`, drive
`app.js` from the browser console, and kill the process by PID only.

## Task 9: Eval scenarios

**Requirements:** proposal §5.8 and its acceptance criterion. **Files:**
`eval-harness/src/scenario.rs`, `runner.rs`, two new
`eval-harness/scenarios/*.toml`, and
`docs/specs/18_local_evaluation_harness/proposal.md`.

- Scenario options: `verify_after_apply = true`, `check_decisions` for the
  batch card, and scripted turns that continue across the verification
  continuation and its repair turns.
- **First measure** (`context.md` §11): time `cargo test` on the
  `rust-workspace` fixture with a shared `CARGO_TARGET_DIR`, cold and warm.
  If it is cheap enough for the deterministic tier, use it. If not, record the
  measurement and pick the cheapest detected check that can be made to fail
  and then pass by a scripted patch. Never a freshly written executable.
- `verified_repair`: request a change, plan, edit, a test fails, repair, it
  passes, complete. Asserts the report's passed list holds only checks that
  ran with exit 0.
- `repair_limit_exhausted`: the negative. Asserts the report does not claim
  success and the task status is `failed`.
- Add both to spec 18's scenario list.

Mutations to record: make the report optimistic at the limit (the negative
scenario fails); count a `None` exit as passed.

## Task 10: Docs, acceptance criteria, close the spec

**Files:** `docs/USER_GUIDE.md`, `docs/TROUBLESHOOTING.md`, this spec's three
files, `docs/specs/README.md`, `CHANGELOG.md`.

- [ ] `USER_GUIDE.md` and `TROUBLESHOOTING.md` per proposal §5.9.
- [ ] Walk every acceptance criterion in proposal §6 and record, per
  criterion, the test or measurement that shows it, in proposal §7 or the
  progress row. Fill in proposal §7's implementation notes.
- [ ] Run the full seven-command gate from `AGENTS.md`, plus
  `npm run specs:check`, each with `CARGO_TARGET_DIR="$PWD/target"` where it
  runs cargo. Record the results.
- [ ] Close per `AGENTS.md` "When a spec becomes Done": the `Status:` line,
  the README row, this file's header and progress table, every `Depends on:`
  naming #23 (#35 waits on #23 and #31; #31 is Done, so #35 is promoted),
  "What to build next", and the `CHANGELOG.md` `Unreleased` entry.
- [ ] Show the change and the gate results and ask before committing.
