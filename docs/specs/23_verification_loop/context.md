# Verification Loop: Context

Where the code contradicted [`proposal.md`](proposal.md), and the decisions it
left open. Checked against the code on 2026-10-09, after spec 24 closed. The
proposal was corrected in place where a sentence was simply wrong. This file
records why, and every decision [`tasks.md`](tasks.md) builds on.

Line numbers are as of 2026-10-09. Other specs edit these files, so re-check a
line before editing near it.

## 1. A patch is applied after its turn has ended, so the apply path starts the loop

The proposal said the loop "runs after a patch is applied within a task" and
placed it in the turn. No turn is running when a patch is applied:

- `propose_patch` and `edit_file` save the patch, finish the action marker as
  `awaiting_review`, and `break` out of the loop with the patch as the turn's
  proposal (`chat.rs:2508-2531`, again at `:2690-2716`). Unlike a command, no
  paused turn is saved. Nothing resumes the model after the patch.
- The user applies it later. `/api/apply-patch` (`desktop-shell/src/lib.rs:1172`)
  calls `EditOrchestrator::apply_stored_patch` (`edit.rs:560`), which appends
  `Evidence::PatchApplied` to the open plan step through the log
  (`edit.rs:657-676`, `SessionStore::append_step_evidence`, `session.rs:1303`).
- The UI applies a patch per file. It may apply some files, reject others, and
  apply the rest later (`app.js`'s `runApply`, `reportResolvedIfSettled`).

**Decision.** Verification is a continuation the apply path triggers. It has
two halves, so that the model cannot skip it and a missing UI call cannot lose
it:

1. **The engine records that verification is due.** When `apply_stored_patch`
   or `reject_stored_patch_files` leaves a patch **settled** (no file still
   pending) with at least one file applied, and the patch's session is in Code
   mode, it appends a `verification_due` session event naming the task and
   the patch. This is in `edit.rs`, beside the `PatchApplied` evidence it
   follows, not in `chat.rs`.
2. **A continuation turn runs it.** `ChatOrchestrator::verify_due(session_id, ..)`
   starts a turn Damaian drives. It is a new task, because a task is one turn
   (spec 21 `context.md` §3.1). The plan is carried into it with
   `SessionStore::resume_plan` (`session.rs:1274`), as a resume does. The shell
   calls it when the patch card reports the patch settled. The CLI gets a
   `verify` subcommand so the loop is reachable without the UI.

Until a `verification_finished` event answers a `verification_due`, the task's
report says verification is due and never "complete" (proposal §5.7). A
verification that is due when Damaian restarts is offered again (§9).

**Why "settled" rather than every apply:** verifying after the first of three
files would run checks against a state the user is still building, and every
later apply would make that run stale. Waiting for the user to finish deciding
the patch costs nothing and verifies the state they chose.

**Why not run the loop inside `apply_stored_patch`:** the loop calls the model
to repair. `EditOrchestrator` has no model adapter, no turn sink and no
streaming, and the apply endpoint is a plain request/response. The
continuation turn has all three.

## 2. Checks are selected by root, and detection is narrower than the proposal assumed

The proposal's table mapped "any Rust source" to "detected build, clippy, and
test commands" and "any web asset under `static/`" to "detected lint
command". Neither holds:

- `detect_in` (`command_policy.rs:308-351`) gives a Cargo root exactly one
  command, `cargo test`, named `Cargo.toml`. No build, no clippy.
- For a `package.json` it finds `npm run <name>` only for the five names
  `test`, `lint`, `typecheck`, `build`, `format`, by substring match on
  `"<name>"`. This repository's root `package.json` names its lint script
  `lint:web`, so it is not found. Spec 24 Task 5 recorded the same thing.
- `static/` is one repository's layout, not a category.

Adding `cargo clippy` or `cargo build` would be a new discovery mechanism, and
the proposal's non-goal forbids that. `cargo test` compiles the crate, so a
compile error is still caught. Clippy lints are not, and the report cannot
list a check that was never proposed. That is a detection change for another
spec. Record it in `OBSERVATIONS.md` when Task 2 lands.

**Decision: select by owning root and its enclosing roots.**

- A changed path is **documentation** when its extension is `md`, `txt`,
  `rst` or `adoc`. It selects nothing. `detect_language` (`language.rs:3`)
  is not used for this: it maps every unknown extension to `text`, which would
  make `Makefile` and `build.sh` documentation.
- Any other changed path selects `RepositoryMap::root_for_path(path)`
  (`repository_map.rs:525`) and every non-removed root whose path encloses
  it. The repository root `""` encloses everything, so it is always selected
  for a non-documentation change.
- Enclosing roots are included because a member change is a workspace change.
  `cargo test` at a Cargo workspace root is what catches a member change that
  breaks a sibling. The cost is that overlapping roots run overlapping checks.
  On this repository a change to `crates/workspace-engine` selects
  `cargo test` there and at the root. Proposal §7 asks for the cost to be
  measured.
- **One de-duplication, within a root only:** `test-shortcut` (`npm test`) is
  dropped when the same root also proposes `npm run test`. Both run the same
  script. Nothing is de-duplicated *across* roots. Whether `cargo test` at a
  root covers a member depends on whether its `Cargo.toml` is a virtual
  workspace or a package with `[workspace]`, and guessing wrong would skip the
  test that mattered.

**Decision: restrict the map, then call `propose_detected_validations`.**
`propose_detected_validations` (`validation.rs:273`) proposes every root's
commands, and each `propose_command_at` saves a proposal file and audits
`command_proposal_stored`. On this repository that is seven `cargo test`
proposals for any change. So the loop builds a copy of the map holding only
the selected roots (`RepositoryMap::restricted_to(&[&str])`) and passes that.
`propose_detected_validations` stays the only discovery path, unchanged, and
nothing is proposed that will not run. The `test-shortcut` drop happens on
the restricted map's commands, before proposing.

The pure selection lives in a new `verification.rs`, so it is tested without a
filesystem, a model or a store.

## 3. One card for many checks: a new pause kind

Proposal §5.3 requires one approval card for every check that needs approval.
Today a turn pauses for exactly one command: `AgentCommandProposal`
(`chat.rs:277`) is one command, and `ChatTurnResult` carries one
(`command_proposal`). The paused turn (`PendingChatTurn`, `chat.rs:3540`)
holds one dispatched call.

**Decision.**

- `ChatTurnResult` gains `check_batch: Option<AgentCheckBatch>`, where
  `AgentCheckBatch { id, checks: Vec<AgentCommandProposal> }`. Each check
  keeps its own `CommandProposal`, so `Allow Always` still works per check
  through the existing `allow_command_always`, and is still exact-command
  and root-qualified (spec 24 `context.md` §2).
- `TurnProposals` (`chat.rs:379`) gains a `checks` field, so every `break`
  site names it.
- The paused turn gains `check_batch: Option<PendingCheckBatch>`, listing the
  proposal ids. `PendingApprovalRef.kind` is `"checks"`.
- `ChatOrchestrator::resume_after_check_decision(batch_id, decisions: &[(proposal_id, approved)], ..)`
  runs the approved checks, records the declined ones as skipped with
  `declined_by_user`, and continues the loop. A proposal id the batch does
  not hold is refused.
- **Every resume entry point refuses another kind's pause.** The command
  resume already refuses a plan review (`chat.rs:837-848`). It must refuse a
  check batch too, and the check resume must refuse a command or a plan.
  Spec 50 `context.md` found the same hole for questions. A batch resumed as a
  command would run `last_content` as a shell command.
- Auto-runnable checks run before the card is shown, without a pause. A
  blocked check never reaches the card.

If spec 50 has landed first, its typed `PendingDecision` is where the
`"checks"` kind goes, and the exhaustive match it adds does the refusing.
Whichever spec lands second adapts.

## 4. Where verification state lives

Proposal §5.1 puts verification in the spec 21 plan. The plan holds steps and
evidence (`plan.rs`). It cannot hold an attempt count, a skip's reason or the
hashes a run saw. `StepStatus::Skipped` exists, but it carries no reason.

**Decision: steps in the plan, the rest in session events, one pure fold.**

- Each selected check is a plan step titled with its command and root, for
  example "Run `cargo test` in `crates/engine`". Its evidence is the
  `CommandExit`, plus `Evidence::Findings` when the run produced any. Its
  status comes from `status_from_evidence` (`plan.rs:158`), unchanged: a
  non-zero or absent exit code blocks the step. A skipped check is a
  `Skipped` step.
- Session events, appended through `append_session_event`
  (`session.rs:2023`): `verification_due` (§1), `verification_started`
  (attempt number, selected checks), `verification_check` (check id,
  outcome, skip reason, the hash of every file the task changed as of the
  run), and `verification_finished` (outcome).
- `verification.rs` holds the types and one pure function,
  `fold_verification(&[VerificationEvent]) -> VerificationState`, that
  replays them. The report, the staleness check and recovery all read that
  state, so there is one interpretation of the log.
- A check's identity is `(working_directory, command)`, root-relative. The
  report and the regression rerun both key on it.

Why not a new `Evidence` variant for "skipped because declined": `Evidence`
is something Damaian observed (`plan.rs:49-66`), and a skip is the absence of
an observation. The plan already has the right word for it, `Skipped`.

## 5. Repair crosses turns

A repair is a patch, and a patch ends the turn (§1). So one repair attempt is:

1. The verification turn finds a failing check. It renders that check's
   findings with spec 22's `RepairRequest::render` (`finding/repair.rs:99`),
   which names each finding by id and bounds each excerpt, and gives it to
   the model in the same turn as a repair request, with `propose_patch` and
   `edit_file` offered as in any Code turn.
2. The model proposes a patch. The turn ends awaiting review, as any patch
   does.
3. The user applies it. The patch settles, verification is due again (§1),
   and the next verification turn is attempt *n + 1*.

**Decisions.**

- **The attempt count is in the log**, in `verification_started`, not in
  memory. It survives the turn boundary and a restart.
- **Attempt 0 is the first verification.** Repairs are attempts 1 to
  `agent_tool_retry_limit` (default 2). When the verification after the last
  repair still fails, the task's status is `TaskStatus::Failed`
  (`session.rs:68`), `verification_finished` records
  `outcome: "repair_limit_exhausted"`, and the report leads with what still
  fails.
- **No changed file, no further attempt.** If a repair turn ends without
  proposing a patch, or its patch settles with nothing applied, the loop
  records `outcome: "no_repair_applied"` and stops with status `Failed`. It
  does not ask again.
- **The regression rerun is the union.** Each verification after a repair
  reruns every check selected so far in this task, plus whatever the
  repair's changed files select. A previously passing check that now fails is
  reported as a regression, and its attempt counts.
- **The ceilings apply unchanged.** A repair turn is a turn. It counts
  `agent_max_tool_rounds` and the task's `agent_max_task_tokens`, and
  reaching either stops at a step boundary with the plan intact, as spec 21
  §5.4 does.
- `agent_tool_retry_limit` is a Preference (`config.rs:2296`), so a
  repository may raise it. The token ceiling is a lower-wins Capability
  (`config.rs:2297`), so a repository cannot raise what the repairs may
  spend. Nothing here changes either classification.

## 6. Staleness, without partial acceptance

The proposal's §5.6 said "the user may accept three of five files after the
loop verified all five". With the loop running after the patch settles (§1),
it verifies what was accepted, so that case does not arise.

What does arise:

- `/api/rollback-patch` (`desktop-shell/src/lib.rs:338`) restores a file the
  loop verified.
- A later patch in the same session changes a verified file.
- The user edits a verified file in another editor.

**Decision.** Each `verification_check` event records the hash of every file
the task has changed, as it was when the check ran. A check is `stale` when
any of those hashes differs from the working tree now. It is computed when
the report is built and when the task is reattached. It is not stored, so it
cannot itself go stale. The status is named `stale`, not
`stale_after_partial_acceptance`, because partial acceptance is not what
causes it. The acceptance criterion was restated to match.

A file that was deleted is a mismatch. A file outside the repository cannot
be in the set, because the patch engine never writes one.

## 7. Browser verification needs a target the task already used

Proposal §5.4 says the loop invokes `web_diagnostics` "when the task already
established a target page".

**Decision.** A target is established when the session has a web diagnostic
record (`SessionStore::read_session_web_diagnostics`, `session.rs:1481`) from
this task or a task it continues, with a URL. When a changed path is a web
asset (`detect_language` is `html`, `css`, `javascript` or `typescript`):

- with a target, the loop reruns that diagnostic at that URL through
  `run_and_record_web_diagnostic` (`chat.rs:574`). Spec 12's session approval
  applies unchanged, and without it the diagnostic needs approval like any
  other. A diagnostic whose page cannot be reached is skipped with
  `no_running_target`;
- with no target, the browser check is skipped with `no_running_target`.

Findings from the diagnostic go through spec 22's `record_web_findings` as
they already do. A browser check is listed in the report under "Behaviour
verified" only when its diagnostic ran and recorded no `Error` finding.

## 8. The report is pure and lives beside the plan

`PlanReport` (`plan.rs:363`) already decides what a plan "came to" and owns the
word "complete" (`plan.rs:385`). The verification report extends that rule,
it does not compete with it.

**Decision.** `VerificationReport::build(&VerificationState, &TaskPlan, changed_files, current_hashes)`
in `verification.rs`, pure. It returns the sections of proposal §5.7 as data
and one `summary()`. Rules are enforced in the builder, each pinned by a test:

- passed means a `CommandExit` with `Some(0)`, and nothing else;
- every selected check is in exactly one section, asserted against the
  selected set;
- "no checks available" is its own state, not an empty passed list;
- `summary()` says "complete" only when `PlanReport::is_complete` holds, no
  check failed, the limit was not exhausted, and nothing is due.

The verification turn's final response is the rendered report, with every
path written as a repository-relative reference so spec 05's link rendering
makes it clickable. The UI also gets the report as data, on the turn result,
for the report card.

`Remaining` and `User action needed` come from the model's closing answer and
from the report's own state, for example "approve `npm run lint` in
`packages/web` to check it". The model never fills `Checks passed`.

## 9. Recovery

Spec 17's recovery classifies a task left `waiting_for_approval` by its
`PendingApprovalRef.kind`. Only `command`, `patch` and `plan` are known
(`recovery.rs:314-336`), and any other kind fails the task.

**Decision.** Recovery learns `"checks"`. A check batch that was waiting is
reattached and offered again, like a command. A `verification_due` with no
`verification_finished`, whose task is not running, is reported to the shell
as verification to offer. Neither is run automatically: running a check
after a restart without asking is the same unrequested side effect spec 17
refuses for commands.

## 10. Who touches `chat.rs`, and pairing

**Tasks 4, 5 and 6 edit `chat.rs`.** Task 7 does only if recovery needs a
`PausedTurns` accessor for the new kind. Tasks 1, 2, 3, 8 and 9 do not.

`docs/specs/README.md` "What to build next" lists the specs that edit
`chat.rs`: #32, #50, #56 and #57, besides this one. **#23 may pair with #25
and with no other `chat.rs` spec** while any of its `chat.rs` tasks is in
progress. #25 is the symbol and relationship index and does not edit
`chat.rs`.

Two shared files, which are rebases rather than blockers:

- `session.rs` (Task 3's events) and `recovery.rs` (Task 7) are also touched
  by #50's Tasks 1 and 5. #50 cannot run beside #23's `chat.rs` tasks anyway.
- `edit.rs` (Task 3's `verification_due`) is touched by no other spec in
  flight.

## 11. The eval scenarios need a check that can fail and then pass

Proposal §5.8 asks for two deterministic scenarios. The runner already
applies a proposed patch (`eval-harness/src/runner.rs:331-336`) and resumes a
command decision (`Scenario.approval_decision`). It has no verification
continuation and no scripted model turns after an apply.

**Decision, partly deferred to Task 9.** The runner gains a
`verify_after_apply` scenario option, a check decision for the batch card,
and scripted turns that continue across the continuation. The fixture needs
a detected check whose result the scripted patch flips. `rust-workspace` with
`cargo test` is the obvious candidate. It is real compilation, so Task 9
measures it first, with a shared `CARGO_TARGET_DIR`, before choosing it.
Writing a fake executable into the fixture is ruled out: freshly written
executables stall on this Mac (`OBSERVATIONS.md` observation 25).

## 12. Things the proposal says that were checked and hold

- `propose_detected_validations` is per root, skips removed roots, and
  classifies each command again where it runs (`validation.rs:268-299`).
- `run_proposal` refuses blocked proposals and returns `ApprovalRequired`
  when approval is missing (`validation.rs:301`).
- `exit_code: None` means killed or signalled (`command_runner.rs:25-30`), and
  `status_from_evidence` already treats it as failure (`plan.rs:158-180`).
- Findings carry ids, an `origin_ref` to the stored output and bounded
  excerpts, and `RepairRequest::render` names them by id (spec 22).
- `require_approval_for_all_commands` makes every proposal require approval,
  read-only ones included, and disables `Allow Always`
  (`command_policy.rs`'s `allow_always_eligible`).
- Ask, Plan and Review cannot apply a patch: `apply_stored_patch` refuses
  unless the session's mode and profile permit patches (`edit.rs:579-580`).
  The loop checks the mode again anyway, because the mode can change between
  apply and verify.
