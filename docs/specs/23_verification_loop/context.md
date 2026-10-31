# Verification Loop: Context

Where the code contradicted [`proposal.md`](proposal.md), and the decisions it
left open. Checked against the code on 2026-10-09, after spec 24 closed, and
re-checked on 2026-10-24 after specs 32, 50, 59, 61, 62 and 63 landed. The
proposal was corrected in place where a sentence was simply wrong. This file
records why, and every decision [`tasks.md`](tasks.md) builds on.

Line numbers are as of 2026-10-24. Other specs edit these files, so re-check a
line before editing near it.

## 0. Plan re-checked on 2026-10-24

The plan was written on 2026-10-09 and never started. Six specs landed since and
each changed code this plan builds on. Every reference in this file and in
`tasks.md` was re-verified on 2026-10-24; §13 records the one spec still in
flight. What changed:

- **Line numbers drifted everywhere.** `chat.rs` is 10,562 lines now. Every
  `(file.rs:NNN)` here and in `tasks.md`'s Interface reference was re-checked
  and updated.
- **§1: a patch's review state is not stored.** `ProposedFilePatch.status` is
  the content's status, not the user's decision, and neither the apply nor the
  reject path rewrites the stored patch. "Settled" has to come from a recorded
  per-file decision.
- **§3: the pause model is `PausedFor`, which is not the type the plan
  expected.** The plan said spec 50's `PendingDecision` would hold the
  `"checks"` kind and its exhaustive match would do the refusing. It does not:
  `PendingDecision` types the recovery *link* (`Approval` or `Question`), and
  the per-turn refusal is `paused_for` into `PausedFor` plus
  `refuse_unless_paused_for`. Spec 61 added a fifth kind. The `"checks"` kind
  plugs into those, and every other resume refuses it without further work, so
  Task 4's refusal work shrank.
- **§3: spec 62 gives each check card entry its cause for free**, through
  `CommandProposal.causes` and `agent_command_proposal`.
- **§5: the round budget split and changed meaning.** Spec 59 split action and
  exploration rounds; spec 61 turned exhaustion into a pause. A repair turn
  counts action rounds, has a separate exploration allowance, and pauses
  rather than stopping at the round budget.
- **§5: `status_from_evidence` now reads the command's purpose** (spec 63). A
  check must be recorded as `CommandPurpose::Check`, or a failing check would
  leave its plan step `Completed`.
- **§8: `PlanReport::is_complete` is a field**, not a method.
- **§9: recovery knows five pause kinds** and has a catch-all arm that fails an
  unknown one. It needs a `"checks"` arm before that arm, and `PausedTurns`
  needs an accessor for the batch.
- **§10: the `chat.rs` spec list was stale.** #32 and #50 are Done, as are #56
  and #57. The pairing rule is now #26's.
- **§12: several "checked and hold" references moved or changed shape**,
  including `allow_always_eligible`, `detect_in`'s visibility, and the
  `exit_code: None` rule.
- **§13 is new**: spec 26 rewrites the context-assembly and prompt-building
  path this loop's repair turn needs, so the `chat.rs` tasks wait for it.

## 1. A patch is applied after its turn has ended, so the apply path starts the loop

The proposal said the loop "runs after a patch is applied within a task" and
placed it in the turn. No turn is running when a patch is applied:

- `propose_patch` and `edit_file` save the patch, finish the action marker as
  `awaiting_review`, and `break` out of the loop with the patch as the turn's
  proposal (`chat.rs:3858-3906`, again at `:4054-4109`). Unlike a command, no
  paused turn is saved. Nothing resumes the model after the patch.
- The user applies it later. `/api/apply-patch` (`desktop-shell/src/lib.rs:358`,
  handler `:1422`) calls `EditOrchestrator::apply_stored_patch` (`edit.rs:666`),
  which appends `Evidence::PatchApplied` to the open plan step through the log
  (`edit.rs:801-812`, `SessionStore::append_step_evidence`, `session.rs:1492`).
- The UI applies a patch per file. It may apply some files, reject others, and
  apply the rest later (`app.js`'s `runApply` at `:10091`, `reportResolvedIfSettled`
  at `:9948`). That state is the UI's, not the engine's: `apply_stored_patch`
  takes `approved_paths` for one call and `reject_stored_patch_files` writes a
  separate rejected copy (`edit.rs:930`, `edit.rs:197-216`). Neither rewrites
  the stored patch, and `ProposedFilePatch.status` (`patch_engine.rs:24`) is
  `added`, `modified` or `deleted` — the content's status, never the review's.

**Decision.** Verification is a continuation the apply path triggers. It has
two halves, so that the model cannot skip it and a missing UI call cannot lose
it:

1. **The engine records each patch-file decision, then that verification is
   due.** When `apply_stored_patch` or `reject_stored_patch_files` runs, it
   appends a `patch_file_decided` session event naming the task, the patch and
   the paths it decided, with which way. When those decisions across the log
   cover every file of the patch, at least one file was applied, and the
   patch's session is in Code mode, the loop appends a `verification_due`
   session event naming the task and the patch. This is in `edit.rs`, beside
   the `PatchApplied` evidence it follows, not in `chat.rs`.

   The one exception is a patch proposed by a repair turn (§5): if the task
   already has an unfinished verification attempt, a settle with every file
   rejected still appends `verification_due`, because that is how the attempt
   is closed with `no_repair_applied`. Without it nothing would ever ask
   again. A task's first patch keeps the stricter rule: nothing applied,
   nothing due.

   The decision record is what makes "settled" knowable at all. The stored
   patch keeps no review state (above), so deriving it from
   `ProposedFilePatch.status` would report every patch unsettled forever. It
   also gives "once per patch" for free: a second settle finds no undecided
   file. Reading the audit log instead was rejected — it is not part of the
   session a reader replays.
2. **A continuation turn runs it.** `ChatOrchestrator::verify_due(session_id, ..)`
   starts a turn Damaian drives. It is a new task, because a task is one turn
   (spec 21 `context.md` §3.1). The plan is carried into it with
   `SessionStore::resume_plan` (`session.rs:1463`), as a resume does. The shell
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

- `detect_in` (`command_policy.rs:708-751`, private, `(repository_root,
  directory)`) gives a Cargo root exactly one command, `cargo test`, named
  `Cargo.toml`. No build, no clippy.
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
  `rst` or `adoc`. It selects nothing. `detect_language` (`language.rs:616`)
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
`propose_detected_validations` (`validation.rs:349`) proposes every root's
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
(`chat.rs:318`) is one command, and `ChatTurnResult` (`chat.rs:462`) carries one
(`command_proposal` at `:468`). The paused turn (`PendingChatTurn`,
`chat.rs:5696`) holds one dispatched call.

**The pause model is typed and exhaustive, since spec 50.** `PendingChatTurn`
carries at most one optional marker per kind — `plan_review`,
`provider_fallback`, `round_budget` (spec 61), `question` (spec 50) — and
`paused_for` (`chat.rs:5676-5690`) turns those into a `PausedFor`
(`chat.rs:5628-5639`). Every resume names the kind it accepts and calls
`refuse_unless_paused_for` (`chat.rs:2356-2377`) before doing anything else;
that call puts the pending-turn file back so the resume that owns it still
finds it. The five kinds are `Action`, `PlanReview`, `ProviderFallback`,
`RoundBudget` and `Question`.

The plan expected a typed `PendingDecision` to be where the kind goes. That
type exists (`clarification.rs:148`) but does something else: it is what
recovery reads off a task's status event, and its two variants are `Approval`
and `Question`. The per-turn refusal is `PausedFor`'s job. A check batch is an
`Approval` with kind `"checks"`, so both layers gain a case, in different
files: a `PausedFor::CheckBatch` in `chat.rs` for the resume, and a
`"checks"` arm in `recovery.rs` for the restart.

**Decision.**

- `ChatTurnResult` gains `check_batch: Option<AgentCheckBatch>`, where
  `AgentCheckBatch { id, checks: Vec<AgentCommandProposal> }`. Each check
  keeps its own `CommandProposal`, so `Allow Always` still works per check
  through the existing `allow_command_always`, and is still exact-command
  and root-qualified (spec 24 `context.md` §2).
- Spec 62 needs no new field here: `CommandProposal` carries
  `causes: Vec<ApprovalCause>` (`validation.rs:35`), and
  `agent_command_proposal` (`chat.rs:7395`) renders the first into the
  `AgentCommandProposal.cause` the card shows. Building a batch entry that way
  keeps the prompt's stated reason for every check.
- `TurnProposals` (`chat.rs:512`) gains a `checks` field, so every `break`
  site names it.
- The paused turn gains `check_batch: Option<PendingCheckBatch>` with
  `#[serde(default)]`, listing the proposal ids. `PendingApprovalRef.kind` is
  `"checks"`.
- `PausedFor` gains `CheckBatch`, with one arm in `paused_for`'s tuple match, a
  `description` ("a check approval") and a `resume_method` naming
  `resume_after_check_decision`. The tuple match makes an unclassified marker
  a compile error, which is the whole of the work of teaching the other
  resumes to refuse it: each already calls `refuse_unless_paused_for` with its
  own kind, so none of them is edited.
- `ChatOrchestrator::resume_after_check_decision(batch_id, decisions: &[(proposal_id, approved)], ..)`
  takes the pending turn, calls `refuse_unless_paused_for(.., PausedFor::CheckBatch)`,
  runs the approved checks, records the declined ones as skipped with
  `declined_by_user`, and continues the loop. A proposal id the batch does
  not hold is refused.
- **A batch id reaching the command route is refused, not run.** The shell
  routes `/api/run-command` to the command resume whenever
  `has_pending_chat_command` (`chat.rs:2345-2347`) is true, and that is true
  for any non-question pause. The command resume's `refuse_unless_paused_for`
  at `:1109` answers instead, so `last_content` is never run as a shell
  command. `has_pending_chat_command` needs no change; its question special
  case exists for a different reason (it falls through to the standalone
  proposal path), and a batch needs no such path.
- Auto-runnable checks run before the card is shown, without a pause. A
  blocked check never reaches the card.
- **Running a check fires `after_check`.** Spec 32 `proposal.md` §7.3 narrows
  that hook to `record_command_findings`, for a validation command only, and
  its §4 non-goals say a hook "observes check results; it does not schedule
  checks". So Task 4 records each check's findings through
  `record_command_findings` (`chat.rs:2099`) and the hook fires from there,
  exactly as it does for a check the model ran. It must not write findings
  through `record_findings` directly to avoid the hook: that would make a
  verification check the one check no hook sees.

## 4. Where verification state lives

Proposal §5.1 puts verification in the spec 21 plan. The plan holds steps and
evidence (`plan.rs`). It cannot hold an attempt count, a skip's reason or the
hashes a run saw. `StepStatus::Skipped` exists, but it carries no reason.

**Decision: steps in the plan, the rest in session events, one pure fold.**

- Each selected check is a plan step titled with its command and root, for
  example "Run `cargo test` in `crates/engine`". Its evidence is the
  `CommandExit`, plus `Evidence::Findings` when the run produced any. Its
  status comes from `status_from_evidence` (`plan.rs:167`), unchanged: a
  non-zero or absent exit code blocks the step **when the run's purpose is
  `Check` or `Other`** — and a detected check is `Check`, because
  `propose_detected_validations` passes `detected: true` to
  `command_purpose` (`command_purpose.rs:53`). An exit recorded as
  `CommandPurpose::Exploration` decides nothing (spec 63 `proposal.md` §5.3),
  so the loop must never record a check that way. A skipped check is a
  `Skipped` step.
- Session events, appended through `append_session_event`
  (`session.rs:2264`): `patch_file_decided` (§1), `verification_due` (§1),
  `verification_started`
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
is something Damaian observed (`plan.rs:72-105`), and a skip is the absence of
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
  `agent_tool_retry_limit` (default 16 since
  [spec 96](../96_continue_repair_at_the_limit.md)). When the verification
  after the last repair still fails, the task's status is `TaskStatus::Failed`
  (`session.rs:71`), `verification_finished` records
  `outcome: "repair_limit_exhausted"`, and the report leads with what still
  fails. Spec 96's explicit **Continue repair** action can start exactly one
  more attempt; it does not raise the limit or run on its own.
- **No changed file, no further attempt.** If a repair turn ends without
  proposing a patch, that turn records `outcome: "no_repair_applied"` and
  stops with status `Failed`. It does not ask again. If it proposes a patch
  and the user rejects every file of it, §1's exception appends `Due`, and
  the next verification turn reads that patch's decisions, finds nothing
  applied, records the same outcome and stops. Neither case spends the
  remaining attempts.
- **The regression rerun is the union.** Each verification after a repair
  reruns every check selected so far in this task, plus whatever the
  repair's changed files select. A previously passing check that now fails is
  reported as a regression, and its attempt counts.
- **The ceilings apply, with one correction since spec 61.** A repair turn is
  a turn, so it spends *action* rounds against `agent_max_tool_rounds` and
  read-only rounds against its own allowance, `agent_max_exploration_rounds`
  (spec 59). The task's `agent_max_task_tokens` still applies to it. Reaching
  the token ceiling stops at a step boundary with the plan intact, as spec 21
  §5.4 does. Reaching the round budget does **not** stop the turn: with no
  token ceiling set, the loop pauses it before the forced final call and
  offers *Continue* or *Answer now* (`pause_for_round_budget`,
  `chat.rs:4860`; `AgentRoundBudgetProposal`, `chat.rs:421`). With a ceiling
  set, spec 47's continuation keeps going under it, and the pause comes only
  at `ABSOLUTE_TOOL_ROUND_CAP` (16, `chat.rs:2884-2887`). A continuation
  resumes the same turn, so it neither ends the repair attempt nor changes the
  attempt count. Once the turn has used `ABSOLUTE_TURN_ROUND_CAP` (64) rounds
  across continuations, *Continue* is refused and only *Answer now* remains,
  which is the old stop and leaves the plan intact.
- `agent_tool_retry_limit` is a Preference (`config.rs:2838`, field at `:329`,
  default 16 since spec 96), so a repository may raise it. The token ceiling is a
  lower-wins Capability (`config.rs:2842`), as is the exploration allowance
  (`config.rs:2846`), so a repository cannot raise what the repairs may spend.
  `agent_max_tool_rounds` is a Preference (`config.rs:2836`). Nothing here
  changes any classification.
- `agent_tool_retry_limit` has one existing reader — the browser diagnostic's
  repeated-failure note (`chat.rs:4202`). Reusing it as the repair bound adds a
  second meaning, so a user who lowers it to stop repeated browser calls also
  lowers repair attempts. That is the reuse proposal §2 asks for; it is
  recorded here because the doc comment on the field still describes only the
  first meaning.

## 6. Staleness, without partial acceptance

The proposal's §5.6 said "the user may accept three of five files after the
loop verified all five". With the loop running after the patch settles (§1),
it verifies what was accepted, so that case does not arise.

What does arise:

- `/api/rollback-patch` (`desktop-shell/src/lib.rs:359`, handler `:1494`)
  restores a file the loop verified.
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
record (`SessionStore::read_session_web_diagnostics`, `session.rs:1670`) from
this task or a task it continues, with a URL. When a changed path is a web
asset (`detect_language` is `html`, `css`, `javascript` or `typescript`):

- with a target, the loop reruns that diagnostic at that URL through
  `run_and_record_web_diagnostic` (`chat.rs:746`). Spec 12's session approval
  applies unchanged, and without it the diagnostic needs approval like any
  other. A diagnostic whose page cannot be reached is skipped with
  `no_running_target`;
- with no target, the browser check is skipped with `no_running_target`.

Findings from the diagnostic go through spec 22's `record_web_findings`
(`chat.rs:2241`) as they already do. A browser check is listed in the report
under "Behaviour verified" only when its diagnostic ran and recorded no
`Error` finding.

## 8. The report is pure and lives beside the plan

`PlanReport` (`plan.rs:382`) already decides what a plan "came to" and owns the
word "complete" (`PlanReport::summary`, `plan.rs:404`, gated by its
`is_complete` field at `:384`). The verification report extends that rule,
it does not compete with it.

**Decision.** `VerificationReport::build(&VerificationState, &TaskPlan, changed_files, current_hashes)`
in `verification.rs`, pure. It returns the sections of proposal §5.7 as data
and one `summary()`. Rules are enforced in the builder, each pinned by a test:

- passed means a `CommandExit` with `Some(0)`, and nothing else;
- every selected check is in exactly one section, asserted against the
  selected set;
- "no checks available" is its own state, not an empty passed list;
- `summary()` says "complete" only when the `PlanReport::is_complete` field
  holds, no check failed, the limit was not exhausted, and nothing is due.

The verification turn's final response is the rendered report, with every
path written as a repository-relative reference so spec 05's link rendering
makes it clickable. The UI also gets the report as data, on the turn result,
for the report card.

`Remaining` and `User action needed` come from the model's closing answer and
from the report's own state, for example "approve `npm run lint` in
`packages/web` to check it". The model never fills `Checks passed`.

## 9. Recovery

Spec 17's recovery classifies a task left `waiting_for_approval` by the
`PendingApprovalRef.kind` on its newest status event. Five kinds are known now
— `command`, `patch`, `plan`, `provider_fallback` (spec 56) and `round_budget`
(spec 61) — in `reattach_pending_approvals` (`recovery.rs:339`), and
`PendingDecision::Approval` carries the link (`clarification.rs:148`,
`session.rs:244`). Any other `Approval` kind falls into the catch-all at
`recovery.rs:421-423` and fails the task with "unknown pending approval kind".

**Decision.** Recovery learns `"checks"`, with one arm before that catch-all.
The batch that was waiting is reattached and offered again, like a command; a
paused turn whose file is gone fails the task, like the other pauses whose
resume needs it. `ReattachedApproval` (`recovery.rs:266`) gains a
`CheckBatch` variant, and `PausedTurns` (`chat.rs:5827`) gains an accessor for
the batch's proposal ids, shaped like `round_budget_pause` (`chat.rs:5876`).
That accessor is the only reason Task 7 might edit `chat.rs`.
`PendingApprovalRef.kind`'s doc list (`session.rs:245-246`) gains `"checks"`
in the same change. A `verification_due` with no `verification_finished`,
whose task is not running, is reported to the shell as verification to offer.
Neither is run automatically: running a check after a restart without asking is
the same unrequested side effect spec 17 refuses for commands.

## 10. Who touches `chat.rs`, and pairing

**Tasks 4, 5 and 6 edit `chat.rs`.** Task 7 does only if recovery needs a
`PausedTurns` accessor for the new kind, which §9 says it does. Tasks 1, 2, 3,
8 and 9 do not.

`docs/specs/README.md` "What to build next" and "Parallel work" state the rule:
two specs may run together only if **at most one of them edits `chat.rs`**, and
#26, #55 and #49's reuse slice never run together at all. The specs that edit
`chat.rs` and are unbuilt are #23, #26, #65 and #66, plus #49's blocked reuse
slice; #32, #50, #56 and #57 are Done, so the old "#32/#50/#56/#57" list is
stale.

That leaves #23 with no partner among the ready specs: #26 is the keystone the
directory wants next (#23's repair turn needs its prompt path, §13), and #65
and #66 both edit `chat.rs`. #27 and #51 become genuinely independent of #23
only once #26 lands. **#23's `chat.rs` tasks must not run beside #26**, and
§13 makes that a sequencing rule rather than a pairing preference.

Shared files that are rebases rather than blockers:

- `lib.rs`: Task 1 registers `verification.rs` in the module list and the
  re-export block (`lib.rs:1-51`, `:88`); #26 edits the `ContextItem` /
  `ContextPlan` re-export at `:88`. Different lines, same file.
- `edit.rs`: Task 3 edits the apply and reject paths; #26 edits
  `build_edit_prompt` (`edit.rs:1086`) and its `build_context_with_hooks` call
  (`:509`) because they read `ContextItem.kind`. Different regions of the same
  file, so Task 3 may run beside #26 and expect one rebase; it does not use the
  context API. §13 has the rule.
- `session.rs` (Task 3's events) is touched by no spec in flight. Neither are
  `repository_map.rs` (Task 2), `verification.rs` (Tasks 1–2) or the eval
  harness (Task 9).

## 11. The eval scenarios need a check that can fail and then pass

Proposal §5.8 asks for two deterministic scenarios. The runner already
applies a proposed patch (`eval-harness/src/runner.rs:517-536`) and resumes a
command decision (`Scenario.approval_decision`,
`eval-harness/src/scenario.rs:144`). It has no
verification continuation and no scripted model turns after an apply.

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
  classifies each command again where it runs (`validation.rs:349-376`).
- `run_proposal` refuses blocked proposals and returns `ApprovalRequired`
  when approval is missing (`validation.rs:389-399`).
- `exit_code: None` means killed or signalled (`command_runner.rs:26`), and
  `status_from_evidence` treats it as failure for a `Check` or `Other` purpose
  (`plan.rs:167-199`). Spec 63 added the one exception: an `Exploration` exit
  decides nothing, including `None`. A detected check is `Check`, so the rule
  this loop relies on is unchanged — but only because the purpose is right
  (§4).
- Findings carry ids, an `origin_ref` to the stored output and bounded
  excerpts, and `RepairRequest::render` names them by id (spec 22).
- `require_approval_for_all_commands` makes every proposal require approval,
  read-only ones included, and disables `Allow Always`
  (`allow_always_eligible`, `command_policy.rs:768`).
- Ask, Plan and Review cannot apply a patch: `apply_stored_patch` refuses
  unless the session's mode and profile permit patches (`edit.rs:685-686`).
  The loop checks the mode again anyway, because the mode can change between
  apply and verify.
- **Spec 63 does not change what running a check through `run_proposal` does.**
  Its shell-loop and `set`/`sed` work is a lexical classifier over the
  *agent's own* commands (`command_purpose.rs:99`), applied after the policy
  has judged them, and only when the proposal is not detected. A check's
  command comes from `propose_detected_validations`, which passes
  `detected: true` (`validation.rs:371`), so its purpose is always `Check`
  whatever the command text looks like. Approval, risk and blocking are the
  same for every purpose (spec 63 §6).

## 13. Collision with spec 26 (context assembly)

Spec 26 ([`26_context_assembly/`](../26_context_assembly/proposal.md), Status: In
progress) rewrites context assembly and the prompt that carries it. It is being
implemented in parallel with this plan, and both touch `chat.rs`. It must land
first, for a reason, not only because of file contention.

**What both specs touch.** #26 replaces `ContextItem`'s `kind: String` with a
`ContextCategory` enum and adds a range, provenance and owning root
(`context_manager.rs:25-41`); it replaces the single `token_budget` argument of
`ContextManager::build_context` (`context_manager.rs:96`) and
`build_context_with_hooks` (`context_manager.rs:124`) with per-category
budgets; it adds a manifest. Anything that reads an item's `kind` or renders
items into a prompt changes with it:

- `build_model_prompt` (`chat.rs:6101-6150`), which renders each item's `kind`
  and `path`.
- Its call site and the assembly call beside it (`chat.rs:1030-1044`).
- `bounded_messages` (`chat.rs:6161`) is not #26's; #65 and #55 rewrite it. #26
  changes what goes into the window, not the window itself. The repair request
  has to survive both.
- `edit.rs:509` and `build_edit_prompt` (`edit.rs:1086`) do the same for the
  one-shot edit flow. Task 3 does not touch them.
- `lib.rs:88` re-exports `ContextItem` and `ContextPlan`. Task 1 adds a module
  and its own re-exports in the same block.

**Does the verification turn use the rewritten path?** Yes, twice.

1. `verify_due` starts a task and, on a failing check, runs an agentic turn.
   Its first model request is built the ordinary way — assembled context plus
   the prompt — so it goes through exactly the function #26 rewrites. The
   prompt is Damaian's own ("verify the patch that was just applied"), not a
   user request, which is a case #26 does not have and must not lose.
2. The repair request is `RepairRequest::render` (`finding/repair.rs:99`)
   followed by the failing checks' findings. Task 5 puts it in front of the
   model as what the turn is being asked to do. Whether that means a second
   user message in the in-turn `messages` array (#61's replay path, which
   `build_model_prompt` does not touch) or a `Findings` context item
   (`ContextCategory::Findings`, which #26 budgets at a fraction of the window)
   cannot be settled before #26 exists. #26 already defines a `Findings`
   category from spec 22's findings, so the second is now the natural home —
   and it would make the repair request budgeted, deduplicated and visible in
   the manifest, instead of an unbudgeted extra message.

**Rule.** #23's `chat.rs` tasks — **4, 5 and 6** — start only after #26 is
merged into `main`, and rebase on #26's prompt and context API. Do not merge
them in parallel and reconcile afterwards: the prompt path is the one region
where both are changing the same lines with different models of what context
is.

**Re-check against #26's result, before Task 4 or 5 is written:**

- how a turn's first model request is built, and where a Damaian-authored
  prompt (not a user request) enters it;
- how the repair request is injected into the prompt — in-turn message or
  `Findings` item — and whether it is budgeted as its own context category;
- whether `build_model_prompt` still exists, and what renders items now
  (category, range, provenance);
- whether the verification turn assembles repository context at all, or only
  the plan state, the findings and the changed files: a repair turn needs the
  failing files far more than it needs search results;
- the manifest: whether a verification turn records one, and where a reader
  finds it (spec 27 renders it);
- `bounded_messages` and `agent_max_turn_messages`: whether an injected repair
  request is at risk of being dropped at the oldest end when the turn runs
  long.

**What can run beside #26.** Tasks 1, 2, 3 and Task 9's measurement do not
touch `chat.rs`. These are the files #26 must change, checked on 2026-10-24: `context_manager.rs`
(`ContextItem`, `ContextPlan`, `build_context`, `build_context_with_hooks`),
`chat.rs` (`build_model_prompt` and its call site), `edit.rs`
(`build_edit_prompt` and its context call — both read `ContextItem.kind`) and
`lib.rs` (the re-export block). So:

- Task 1 (`verification.rs`, `tests/verification_loop.rs`, a `lib.rs` module
  line), Task 2 (`verification.rs`, `repository_map.rs`, `lib.rs`),
  `session.rs` in Task 3, and Task 9's fixture measurement are outside #26's
  set and may run beside it. `lib.rs` is shared with #26's re-export block, so
  expect one rebase there.
- Task 3 is the exception to "Tasks 1–3 are independent": `edit.rs` **is** in
  #26's set, through `build_edit_prompt` and the edit flow's context call. Task
  3 changes the apply and reject paths, a different region, so it may still run
  beside #26 — but it must be rebased on #26 before it is merged, and it must
  not "fix" `build_edit_prompt` on the way past.
- Task 8 touches `desktop-shell` and `app.js`, which #26 does not. It can run
  beside #26, but it cannot be tested end to end until Task 4 exists.

