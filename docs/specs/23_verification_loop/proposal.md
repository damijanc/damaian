# Feature Spec: Verification Loop

Status: Done 2026-10-25. Started and finished the same day, after
[#26](../26_context_assembly/proposal.md) merged. Split into a folder and
planned on 2026-10-09, after
[#24](../24_repository_map_and_monorepo_boundaries/proposal.md) made check
discovery per root. Where the code contradicted the design, this proposal was
corrected in place. The decisions it left open are in
[`context.md`](context.md). The most important ones:
- a patch is applied after the turn that proposed it has ended, so the loop
  is a continuation that the apply path triggers, not a step inside a turn
  (`context.md` §1);
- checks are selected by the root that owns each changed file, not by a
  file-category table. Detection yields `cargo test` for a Cargo root and no
  separate build or clippy command (`context.md` §2);
- the loop verifies what was applied, so "accepted a subset after the loop
  verified all of it" cannot happen. Staleness is still real, for a rollback
  or an edit made after a check ran (`context.md` §6).
Order: 23 of 23
Plan: `docs/PLAN/02_phase_2_complete_task_workflow.md`, Phase 2, Work
Package 3 (Must). That directory is local-only and not committed, so the
reference is a name rather than a link; this spec is self-contained.
Depends on: [#18](../18_local_evaluation_harness/proposal.md) (the harness) —
built; [#21](../21_task_plan_progress_and_budget/proposal.md) (plan state) —
built; [#22](../22_findings_model_and_panel/proposal.md) (the finding type it repairs) —
built; [#24](../24_repository_map_and_monorepo_boundaries/proposal.md) (per-root
check discovery) — built. Everything else named below is a cross-reference,
not a prerequisite.
Related spec sections: `ai_coding_assistant_specification.md` section 7.4
(command approval), section 7.6 (tool and action orchestrator), section 11 (error
handling). Related implementation specs:
[`04_hunk_level_patch_apply.md`](../04_hunk_level_patch_apply.md),
[`10_persistent_command_approval.md`](../10_persistent_command_approval.md),
[`12_web_app_troubleshooting`](../12_web_app_troubleshooting/proposal.md),
[`21_task_plan_progress_and_budget`](../21_task_plan_progress_and_budget/proposal.md) (the
plan and evidence this loop populates),
[`22_findings_model_and_panel`](../22_findings_model_and_panel/proposal.md) (the finding
model this loop produces and repairs against),
[`24_repository_map_and_monorepo_boundaries`](../24_repository_map_and_monorepo_boundaries/proposal.md)
(the roots checks are discovered and run in).

## 1. Motivation

Damaian applies a change and stops.

Every piece needed to check the change already exists — `patch_engine` applies
it, `ValidationOrchestrator::propose_detected_validations`
(`crates/workspace-engine/src/validation.rs:349`) knows what the project's checks
are, `CommandRunner` runs them, `web_diagnostics` can inspect a running page.
Nothing sequences them. The agent finishes a patch, says what it did, and the
question of whether it works is left to the user.

That produces the specific failure this work package exists to end: a confident
completion message for a change that does not compile. The user's trust in the
assistant is calibrated by exactly this — not by whether it is right often, but
by whether it knows when it is wrong. An assistant that says "I added retry
handling and the tests pass" without running them is worse than one that says "I
added retry handling; I did not run the tests", because the first teaches the user
to skip checking.

The loop closes it: after edits, discover the relevant checks, run the approved
ones, turn failures into findings, repair within a bounded limit, rerun, and
report — distinguishing what was verified from what was assumed.

## 2. Current State

Checked against the code on 2026-10-09; references re-checked on 2026-10-24.

- **The pieces exist; the sequence does not.** `chat.rs` runs a tool-call loop,
  `patch_engine.rs` proposes and applies diffs, `validation.rs` proposes and runs
  commands, `web_diagnostics.rs` inspects a running app. Nothing chains them into
  a task that knows whether it succeeded.
- **A patch is applied after its turn has ended.** `propose_patch` and
  `edit_file` end the turn with a patch awaiting review (`chat.rs:3858-3906`,
  `chat.rs:4054-4109`).
  The user applies it later through `/api/apply-patch`
  (`desktop-shell/src/lib.rs:358`) and `EditOrchestrator::apply_stored_patch`
  (`edit.rs:666`), which appends `Evidence::PatchApplied` to the plan's open
  step through the log (`edit.rs:801-812`). No turn is running at that point
  (`context.md` §1).
- **Validation discovery already exists and should be reused.**
  `ValidationOrchestrator::propose_detected_validations(repository_root, &RepositoryMap)`
  (`validation.rs:349`) maps each repository-map root's detected commands into
  `CommandProposal`s, one per detected check, each carrying the standard
  approval fields and its root's working directory
  ([spec 24](../24_repository_map_and_monorepo_boundaries/context.md) §9). It
  skips removed roots and classifies each command again where it will run.
- **What detection finds is narrow.** `detect_in` (`command_policy.rs:708`,
  private) yields `npm run <test|lint|typecheck|build|format>` for the scripts a
  `package.json` names, plus `npm test` again as `test-shortcut`, and one
  command per other manifest: `pytest`, `mvn test`, `gradle test`,
  `go test ./...`, `cargo test`. A Cargo root has no build or clippy command,
  and a script named `lint:web` is not found (`context.md` §2).
- **Running a check is approval-gated.** `ValidationOrchestrator::run_proposal`
  (`validation.rs:379`) refuses a blocked proposal outright and returns
  `ApprovalRequired` when `requires_approval` is true and `approved` is false.
- **Check results are unstructured** — `CommandExecution` with `exit_code:
  Option<i32>` (`command_runner.rs:26`), `stdout`, `stderr`. Full output is
  persisted by `CommandStore::save_execution` (`validation.rs:113`) as
  `stdout.log`, `stderr.log`, and a summary.
- **Repair bounds already exist.** `agent_tool_retry_limit` (default 2,
  `crates/workspace-engine/src/config.rs:329`), `agent_max_tool_rounds`
  (`config.rs:317`), and `agent_web_debug_max_tool_rounds` (`config.rs:326`).
  All three are Preference keys a repository may set (`config.rs:2836-2838`).
  The roadmap names these as the right hooks rather than new ones.
- **The budget-stop shape exists**: `tool_budget_exhausted_response`
  (`chat.rs:7549`) and `TaskStatus::ToolBudgetExhausted` (`session.rs:73`).
  `TaskStatus::Failed` and `TaskStatus::Validating` exist too (`session.rs:58`).
  Spec 61 added a pause at the round budget beside that stop
  (`context.md` §5).
- **Persistent approval exists.** [Spec 10](../10_persistent_command_approval.md)
  added exact-command `Allow Always`, written to repository config — which matters
  here because a verification loop that prompts for `cargo test` on every task is
  a loop users disable. Since spec 24, a grant made for a check in a sub-root
  is qualified with that root (`cd packages/api && npm test`).
- **Hunk-level acceptance exists end to end**
  ([spec 04](../04_hunk_level_patch_apply.md)), including
  `/api/reject-patch-files` and `/api/rollback-patch`. Each file of a proposed
  patch carries a `status`, but it is the content's (`added`, `modified`,
  `deleted`), not the user's decision; the apply path records what it wrote and
  the reject path writes a separate rejected copy (`context.md` §1).

## 3. Requirements

After agent-generated edits are applied, the loop:

1. Re-reads or validates affected files.
2. Selects targeted tests, lint, type checks, and build checks via
   `propose_detected_validations`.
3. Runs approved checks.
4. Converts failures into structured findings
   ([spec 22](../22_findings_model_and_panel/proposal.md)).
5. Lets the agent repair failures within `agent_tool_retry_limit`.
6. Reruns the relevant checks.
7. Produces a completion report listing files changed, checks passed, checks
   failed, checks skipped and why, behaviour verified through browser
   diagnostics, remaining uncertainty, and user actions still required.

And throughout: **an unrun check is never represented as passed.**

## 4. Non-goals

- LSP diagnostics. The loop consumes parsed check output only. LSP becomes an
  additional source feeding the same `Finding` model when Phase 3 WP3 lands, and
  no acceptance criterion here mentions it. (The roadmap records that requiring
  LSP made this work package uncompletable as originally written.)
- Discovering checks by a new mechanism. `propose_detected_validations` is reused,
  not replaced or supplemented. That includes not adding `cargo clippy` or
  `cargo build` here: a project's checks are what detection finds, and widening
  detection is a change to `detect_in`, outside this spec.
- Running checks without approval. The loop proposes; the existing approval
  boundary decides. Nothing here widens it.
- Deciding a change is good. The loop verifies that the project's own checks pass,
  which is not the same as the change being correct, and the completion report is
  written to avoid implying otherwise.
- Repairing findings the user did not ask about. The loop repairs failures of
  checks it ran; the user-selected subset repair is
  [spec 22](../22_findings_model_and_panel/proposal.md) §5.5.
- Committing, pushing, or opening a pull request on success — Phase 5.
- Background processes for checks that need a running server — Phase 2 WP5,
  outside this phase's minimum slice. §5.4 handles its absence.
- Selecting checks by static analysis of the diff. §5.2 explains why targeting is
  deliberately coarse in this spec.

## 5. Design

### 5.1 Where the loop lives

The loop runs after a patch is applied within a task, driven by the orchestrator
rather than by the model. This is the central structural choice: **the model does
not decide whether to verify.** It may request checks, and it participates in
repair, but the sequence — verify, find, repair, rerun, report — is Damaian's, so
a model that would rather declare success cannot skip it.

A patch is applied after the turn that proposed it has ended (§2). So the
apply path, not the turn, starts the loop. When a patch from a Code-mode task
is settled — every file of it applied or rejected, at least one applied — the
apply path records that verification is due. A continuation of
the task then runs it. That continuation is a turn Damaian starts, not one the
user types, and the plan carries into it the way spec 21 carries a plan across
a resume. A task with verification due cannot be reported complete until the
verification has run (`context.md` §1).

The loop is entered only in Code mode ([spec 20](../20_working_modes/proposal.md)); in Ask,
Plan, and Review there is nothing applied to verify.

It appends steps to the task plan from
[spec 21](../21_task_plan_progress_and_budget/proposal.md) rather than tracking its own
state: a `validating` phase step per check, whose evidence is the
`CommandExit` for that check. That reuse is what makes verification survive a
restart and what makes the plan's completion status honest. What the plan
cannot hold (the attempt count, a skip's reason, the file hashes a run saw) is
kept in session-log events beside it (`context.md` §4).

### 5.2 Selecting checks

`propose_detected_validations(repository_root, &RepositoryMap)` returns every
detected project check, one set per root of the
[spec 24](../24_repository_map_and_monorepo_boundaries/proposal.md) map, each at
its root's working directory. The loop runs the ones relevant to what changed,
where "relevant" is deliberately coarse and decided by **root**, not by
language:

| Changed path | Roots whose checks run |
|---|---|
| Documentation (`.md`, `.txt`, `.rst`, `.adoc`) | None from this path |
| Anything else | The root that owns it (`RepositoryMap::root_for_path`), and every root that encloses that one |
| Only documentation changed | None. Reported as skipped with the reason |

The enclosing roots are included because a change to a member is a change to
its workspace. A Cargo workspace's `cargo test` at the repository root is the
check that would catch a member change breaking a sibling. Roots overlap as a
result: a Cargo workspace root and each member all propose `cargo test`. Both
run. One de-duplication is safe: `npm test` (`test-shortcut`) is dropped when
the same root also proposes `npm run test`, because the two run the same
script.

The loop does not call `propose_detected_validations` for every root and
discard most of the results. Each proposal is persisted and audited, so the
map is restricted to the selected roots first (`context.md` §2).

Targeting is by owning root, not by dependency analysis. A precise
"which tests cover this change" mapping needs the symbol and relationship index
from Phase 3 WP3, and approximating it here with heuristics would produce a loop
that silently skips the test that mattered. Coarse and complete beats precise and
wrong; the cost is running more checks than strictly needed, which is the correct
trade at this stage.

Where the selected roots have no detected checks at all, the loop says so in the
report. "No checks were available" is a materially different statement from
"checks passed", and conflating them is the failure mode requirement 7 guards
against.

### 5.3 Approval, and not defeating it

Each check is a `CommandProposal` and goes through the existing gate
(`validation.rs:379`). Three cases:

- **Auto-runnable** — sandbox-safe read-only, or an exact match in
  `command_allowlist` from [spec 10](../10_persistent_command_approval.md)
  (root-qualified in a sub-root, per spec 24). Runs.
- **Requires approval** — one approval card listing every check the loop wants to
  run, not one per check. A loop that produces five sequential prompts is a loop
  users click through blindly, which is the problem
  [spec 10](../10_persistent_command_approval.md) was written to address. The
  user approves or declines each check on that one card.
- **Blocked** — by `command_blocklist` or a hard block. Recorded as skipped with
  the policy reason. Never retried, never worked around.

A declined check is `skipped` with reason `declined_by_user`. The task continues —
declining a check is not an error — and the completion report states that the
check did not run. `Allow Always` remains available per check and exact-command
only.

`require_approval_for_all_commands` is honoured. A user who set it gets a card
even for read-only checks, which is what they asked for.

### 5.4 Behaviour verification, and its honest limit

Requirement 7 asks the report to list behaviour verified through browser
diagnostics. The loop invokes `web_diagnostics` when a changed path is a web
asset and the task already established a target page — [spec 12](../12_web_app_troubleshooting/proposal.md)'s session-scoped
diagnostic approval applies unchanged.

It does not start a dev server. Background processes are Phase 2 WP5, outside this
phase's minimum slice, so a web change whose verification needs a running server
that is not running is reported as **skipped, reason `no_running_target`** — not
as verified, and not as failed. Saying "I could not check this because nothing was
serving the page" is a useful report line; inferring success from an unreachable
page is not.

### 5.5 Repair, bounded

On a failing check, its findings ([spec 22](../22_findings_model_and_panel/proposal.md)) are
handed to the model with a repair request, bounded by `agent_tool_retry_limit`
(`config.rs:329`):

1. Findings for the failing check are given to the model — findings, not raw log
   output.
2. The model proposes a patch. It goes through the normal preview and approval
   path; repair does not bypass `require_approval_for_file_edits`, and the
   `base_hash` conflict check at `patch_engine.rs:334` still applies.
3. The check reruns once that patch is applied and settled (§5.1).
4. On pass, the step's evidence is the passing `CommandExit`. On fail, attempt
   count increments and the loop returns to 1 until the limit.

Two rules keep the loop from being worse than no loop:

- **No repair attempt without a changed file.** If an attempt produces no applied
  patch, the loop stops rather than spending the remaining attempts asking the
  same question. A model that cannot fix something on attempt one usually cannot
  fix it on attempt three, and the attempts cost real money
  ([spec 19](../19_token_and_cost_accounting/proposal.md)). A repair patch the
  user rejects in full is the same case.
- **Regression guard.** After a repair, checks that previously passed rerun. A
  repair that fixes one check by breaking another is a net loss, and the loop that
  only reruns the failing check would report success. A repair that newly breaks a
  previously passing check is reported as such, and its attempt counts. The
  rerun set is every check the loop has selected so far in the task, plus any
  the repair's own changed files select.

On exhausting the limit, the task does **not** succeed. Status is `failed` with
the still-failing findings attached, and the report leads with what still fails.

The token ceiling from [spec 21](../21_task_plan_progress_and_budget/proposal.md) applies to
repair as to any other work: reaching it stops the loop at a step boundary with
the plan intact.

### 5.6 Checks go stale when the files they saw change

The loop records, per check run, the hash of every file the task has changed
as it was when the check ran. A check whose recorded hashes no longer match
the working tree is marked `stale`, and the report says which checks are no
longer valid for the current state. It does not silently rerun them —
rerunning without being asked would spend tokens and time on a state the user
may still be editing — and it does not report them as passed.

The loop runs on what was applied, after the patch is settled (§5.1). So the
case this section was first written for, accepting a subset of files after the
loop had verified all of them, does not arise. The cases that do are rolling a
verified file back (`/api/rollback-patch`), applying a later patch, and an edit
made outside Damaian (`context.md` §6).

This is the same staleness idea as `FindingStatus::Stale` in
[spec 22](../22_findings_model_and_panel/proposal.md) and the same hash comparison as
`patch_engine.rs:334`, applied to check runs. Phase 2 WP7 builds the review UI
around this; the data it needs is produced here.

### 5.7 The completion report

```text
Files changed        src/upload.rs, tests/upload.rs
Checks passed        cargo test (crates/engine), cargo test (repository root)   (all ran, exit 0)
Checks failed        none
Checks skipped       npm run lint (packages/web) — declined by you
                     browser check — no running target
Checks stale         none
Behaviour verified   none
Unverified steps     "understand existing retry logic" — no observable evidence
Remaining            Review the retry backoff constant; it was chosen arbitrarily
User action needed   none
```

Construction rules, which are the requirement rather than the formatting:

- **`Checks passed` is derived from `CommandExit` evidence with
  `exit_code == Some(0)`.** Not from the absence of a failure, not from
  `!is_error`, and never from `exit_code.unwrap_or(0)` — `exit_code: None`
  (`command_runner.rs:26`) means a signalled or killed command, and it belongs in
  `Checks failed` with the reason.
- **A check is identified by its command and its root**, so `cargo test` at the
  repository root and `cargo test` in `crates/engine` are two checks.
- **A check is listed exactly once**, in exactly one of passed, failed,
  skipped, or stale. A check appearing nowhere is a bug the report's own test
  asserts against by comparing the union against the set of proposed checks.
- **`Unverified steps`** comes from [spec 21](../21_task_plan_progress_and_budget/proposal.md)'s
  steps with empty evidence.
- **The summary line never says "complete"** when anything is in `Checks failed`,
  when the repair limit was exhausted, or when verification is due and has not
  run.
- Every path is rendered as a clickable reference
  ([spec 05](../05_clickable_file_references.md)).

### 5.8 Eval scenario

The roadmap requires the end-to-end fixture to become an eval scenario so later
phases cannot silently regress it. Added to
[spec 18](../18_local_evaluation_harness/proposal.md): request a change, plan,
edit, fail a test, repair, pass, review, complete — with the deterministic tier
scripting the model turns via `MockModelAdapter` and asserting the report's
passed list contains only checks that ran and exited zero.

A second scenario asserts the negative: a task whose repair limit is exhausted
produces a report that does not claim success. That one matters more, because it
is the assertion that fails if someone later makes the loop optimistic.

### 5.9 Documentation

`docs/USER_GUIDE.md`: what the loop does after a change, why some checks are
skipped, which roots' checks run for a change, what the report's sections mean,
and that "no checks available" is not "checks passed". `docs/TROUBLESHOOTING.md`:
how to see which checks ran, where full output lives (`origin_ref` →
`stdout.log`), what `stale` means, and how to allowlist a check so the loop
stops asking.

## 6. Acceptance Criteria

- A task that breaks a test reports the failure, attempts repair within
  `agent_tool_retry_limit`, reruns the check, and reports the outcome.
- Exhausting the repair limit produces a report stating what still fails, with
  task status `failed` — not a success.
- A repair attempt that produces no applied patch stops the loop rather than
  consuming the remaining attempts.
- A repair that breaks a previously passing check is detected by the regression
  rerun and reported.
- A check the user declined appears as skipped with reason `declined_by_user`, and
  the task continues.
- A blocked check appears as skipped with the policy reason and is never retried.
- The report's `Checks passed` list contains only checks that ran and exited zero
  — asserted by test, including a case where `exit_code` is `None`.
- Every proposed check appears exactly once across passed, failed, skipped, and
  stale — asserted by comparing the union against the proposed set.
- A project with no detected checks produces a report saying so, distinct from
  checks passing.
- A change in a sub-root runs that root's checks and its enclosing roots'
  checks, each in its own working directory, and no other root's.
- A change to documentation only runs no checks and says so.
- A web verification with no running target is skipped with reason
  `no_running_target`, never inferred as verified.
- Rolling back a verified file, or changing it after the check ran, marks the
  affected check runs `stale`, and the report says which checks are no longer
  valid.
- Multiple checks requiring approval produce one approval card, not one per check.
- `require_approval_for_all_commands` is honoured for read-only checks.
- The loop does not run in Ask, Plan, or Review mode.
- Verification state survives a restart, since it lives in the task plan and
  the session log. A verification that was due, or a check card that was
  waiting, when Damaian stopped is offered again.
- The summary line never says "complete" when a check failed, the repair limit
  was exhausted, or verification is due and has not run.
- Both eval scenarios from §5.8 exist in
  [spec 18](../18_local_evaluation_harness/proposal.md) and pass.
- Every quality-gate command from `AGENTS.md` passes, and the
  [spec 18](../18_local_evaluation_harness/proposal.md) baseline shows no
  regression.

## 7. Implementation Notes

To be completed during implementation. Record:

- The root-selection rule actually shipped, and any project type where
  targeting selected no checks for a change that clearly needed one.
- How often repair succeeded within the limit during real use, and at which
  attempt. If nearly all successes are on attempt one, the limit is doing less
  work than it appears and the "no changed file" stop is doing most of it.
- Whether the regression rerun and the overlapping roots materially slowed the
  loop, and the measured cost.

### What shipped

**Root selection (Tasks 1–2, 2026-10-25).** `select_roots` takes
`RepositoryMap::root_for_path` and every enclosing non-removed root, exactly as
§5.2 proposes; documentation is the extension list `md`, `txt`, `rst`, `adoc`.
The map is restricted before `propose_detected_validations`, so only selected
roots' proposals are stored and audited. The one observed case where selection
found nothing for a change that needed a check is a root with no manifest at
all: the report says `NoChecksAvailable` rather than pretending it passed. The
Cargo detection gap (no clippy or build command, `lint:web` missed) is
`OBSERVATIONS.md` #48; the loop lists no such check rather than claiming one
passed.

**Repair within the limit (Task 5).** The deterministic `verified_repair`
scenario repairs a compile error on the first attempt; that is one run, not a
success-rate measurement. No real-use sessions were recorded for this spec, so
the question "how often does attempt two succeed" remains open rather than
answered with an invented number. The "no changed file" stop is what ends the
`repair_limit_exhausted` scenario after one repair.

**Cost of overlapping roots and the regression rerun (Tasks 5, 9).** The
`rust-workspace` fixture's `cargo test` measured **1.24s cold and 0.05s warm**
with a shared `CARGO_TARGET_DIR`, so a verification of that fixture is cheap;
that is the only measurement taken. No live repository was run with the loop
enabled, so the proposal §7 question about material slowdown in real projects
is not answered here.

### Acceptance criteria, and what shows each

| Criterion (§6) | Evidence |
|---|---|
| A broken test is reported, repaired within the limit, rerun and reported | `a_failed_check_is_repaired_and_the_rerun_passes`; eval `verified_repair` |
| Exhausting the limit reports what still fails, status `failed` | `exhausting_the_repair_limit_reports_what_still_fails`; eval `repair_limit_exhausted` |
| A repair with no applied patch stops the loop | `a_repair_turn_that_answers_without_a_patch_stops`; `a_fully_rejected_repair_patch_closes_the_attempt_without_another_round` |
| A repair that breaks a passing check is a regression | `a_repair_that_breaks_a_previously_passing_check_is_a_regression` |
| Declined check is skipped `declined_by_user`, task continues | `two_checks_needing_approval_produce_one_batch_and_one_resume`; shell `resume_checks_endpoint_records_declined_checks` |
| Blocked check is skipped with the policy reason and never retried | `a_blocked_check_is_skipped_and_never_runs` |
| `Checks passed` only exit-zero, including `exit_code: None` | `a_check_passes_only_on_exit_code_zero` |
| Every proposed check appears once across passed/failed/skipped/stale | `every_selected_check_is_in_exactly_one_section` |
| No detected checks is distinct from passing | `no_checks_available_is_not_checks_passed`; `verify_due_with_no_detected_checks_says_so`; shell `verify_endpoint_reports_no_checks_available` |
| A sub-root change runs its root's and enclosing roots' checks only | `a_member_change_selects_its_own_root_and_every_enclosing_root` |
| A docs-only change runs no checks and says so | `only_documentation_changes_select_nothing`; `a_documentation_only_patch_verifies_nothing_and_says_so` |
| A web check with no running target is skipped, never verified | `an_unreachable_page_is_skipped_with_no_running_target`; `a_web_change_with_no_stored_target_is_skipped_with_no_running_target` |
| A rollback or later edit marks affected checks `stale` | `a_check_whose_files_changed_is_stale_not_passed` |
| Multiple approvals produce one card | `two_checks_needing_approval_produce_one_batch_and_one_resume` |
| `require_approval_for_all_commands` is honoured for read-only checks | `require_approval_for_all_commands_puts_an_allowlisted_check_on_the_card` |
| The loop does not run in Ask, Plan or Review | `verify_due_refuses_a_session_that_is_no_longer_code`; `an_ask_mode_session_never_records_a_due` |
| Verification survives a restart; a due run or waiting card is offered again | `a_due_verification_is_offered_after_a_reopen`; `a_check_batch_is_reattached_and_resumes_after_a_reopen`; `verification_events_round_trip_through_a_fresh_store` |
| The summary never says "complete" when a check failed, the limit was hit, or verification is due | `the_summary_never_says_complete_when_anything_is_wrong`; eval `repair_limit_exhausted` |
| Both eval scenarios exist and pass | `verified_repair` and `repair_limit_exhausted`, 25/25 deterministic scenarios |
| The quality gate passes and spec 18 shows no regression | Scoped checks recorded in `tasks.md`; the workspace gate and the CI Quality run are recorded there when green |
