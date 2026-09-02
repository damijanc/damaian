# Feature Spec: Verification Loop

Status: Not started
Order: 23 of 23
Roadmap: `docs/ROADMAP/02_phase_2_complete_task_workflow.md`, Phase 2, Work
Package 3 (Must). That directory is local-only and not committed, so the
reference is a name rather than a link; this spec is self-contained.
Related spec sections: `ai_coding_assistant_specification.md` section 7.4
(command approval), section 7.6 (tool and action orchestrator), section 11 (error
handling). Related implementation specs:
[`04_hunk_level_patch_apply.md`](04_hunk_level_patch_apply.md),
[`10_persistent_command_approval.md`](10_persistent_command_approval.md),
[`12_web_app_troubleshooting.md`](12_web_app_troubleshooting.md),
[`21_task_plan_progress_and_budget.md`](21_task_plan_progress_and_budget.md) (the
plan and evidence this loop populates),
[`22_findings_model_and_panel.md`](22_findings_model_and_panel.md) (the finding
model this loop produces and repairs against).

## 1. Motivation

Damaian applies a change and stops.

Every piece needed to check the change already exists — `patch_engine` applies
it, `ValidationOrchestrator::propose_detected_validations`
(`crates/workspace-engine/src/validation.rs:167`) knows what the project's checks
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

- **The pieces exist; the sequence does not.** `chat.rs` runs a tool-call loop,
  `patch_engine.rs` proposes and applies diffs, `validation.rs` proposes and runs
  commands, `web_diagnostics.rs` inspects a running app. Nothing chains them into
  a task that knows whether it succeeded.
- **Validation discovery already exists and should be reused.**
  `ValidationOrchestrator::propose_detected_validations` (`validation.rs:167`)
  maps `CommandPolicy::detect_project_commands` output into `CommandProposal`s,
  one per detected check, each carrying the standard approval fields.
- **Running a check is approval-gated.** `ValidationOrchestrator::run_proposal`
  (`validation.rs:186`) refuses a blocked proposal outright and returns
  `ApprovalRequired` when `requires_approval` is true and `approved` is false.
- **Check results are unstructured** — `CommandExecution` with `exit_code:
  Option<i32>`, `stdout`, `stderr` (`command_runner.rs:11-22`). Full output is
  persisted by `CommandStore::save_execution` (`validation.rs:63`) as
  `stdout.log`, `stderr.log`, and a summary.
- **Repair bounds already exist.** `agent_tool_retry_limit`
  (`crates/workspace-engine/src/config.rs:73`), `agent_max_tool_rounds`
  (`config.rs:67`), and `agent_web_debug_max_tool_rounds` (`config.rs:70`), all
  overlayable from repository config (`config.rs:308-315`). The roadmap names
  these as the right hooks rather than new ones.
- **The budget-stop shape exists**: `tool_budget_exhausted_response`
  (`chat.rs:841`) and `TaskStatus::ToolBudgetExhausted` (`chat.rs:1205`).
- **Persistent approval exists.** [Spec 10](10_persistent_command_approval.md)
  added exact-command `Allow Always`, written to repository config — which matters
  here because a verification loop that prompts for `cargo test` on every task is
  a loop users disable.
- **Hunk-level acceptance exists end to end**
  ([spec 04](04_hunk_level_patch_apply.md)), including
  `/api/reject-patch-files`. This is what makes §5.6's stale-check problem real.

## 3. Requirements

After agent-generated edits are applied, the loop:

1. Re-reads or validates affected files.
2. Selects targeted tests, lint, type checks, and build checks via
   `propose_detected_validations`.
3. Runs approved checks.
4. Converts failures into structured findings
   ([spec 22](22_findings_model_and_panel.md)).
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
  not replaced or supplemented.
- Running checks without approval. The loop proposes; the existing approval
  boundary decides. Nothing here widens it.
- Deciding a change is good. The loop verifies that the project's own checks pass,
  which is not the same as the change being correct, and the completion report is
  written to avoid implying otherwise.
- Repairing findings the user did not ask about. The loop repairs failures of
  checks it ran; the user-selected subset repair is
  [spec 22](22_findings_model_and_panel.md) §5.5.
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

The loop is entered only in Code mode ([spec 20](20_working_modes.md)); in Ask,
Plan, and Review there is nothing applied to verify.

It appends steps to the task plan from
[spec 21](21_task_plan_progress_and_budget.md) rather than tracking its own
state: a `validating` phase step per check, whose evidence is the
`CommandExit` for that check. That reuse is what makes verification survive a
restart and what makes the plan's completion status honest.

### 5.2 Selecting checks

`propose_detected_validations(working_directory)` returns every detected project
check. The loop runs the ones relevant to what changed, where "relevant" is
deliberately coarse:

| Changed paths | Checks run |
|---|---|
| Any Rust source | Detected build, clippy, and test commands |
| Any web asset under `static/` | Detected lint command |
| Both | Both sets |
| Only documentation or config with no detected check | None. Reported as skipped with the reason |

Targeting is by file category, not by dependency analysis. A precise
"which tests cover this change" mapping needs the symbol and relationship index
from Phase 3 WP3, and approximating it here with heuristics would produce a loop
that silently skips the test that mattered. Coarse and complete beats precise and
wrong; the cost is running more checks than strictly needed, which is the correct
trade at this stage.

Where a project has no detected checks at all, the loop says so in the report.
"No checks were available" is a materially different statement from "checks
passed", and conflating them is the failure mode requirement 7 guards against.

### 5.3 Approval, and not defeating it

Each check is a `CommandProposal` and goes through the existing gate
(`validation.rs:186`). Three cases:

- **Auto-runnable** — sandbox-safe read-only, or an exact match in
  `command_allowlist` from [spec 10](10_persistent_command_approval.md). Runs.
- **Requires approval** — one approval card listing every check the loop wants to
  run, not one per check. A loop that produces five sequential prompts is a loop
  users click through blindly, which is the problem
  [spec 10](10_persistent_command_approval.md) was written to address.
- **Blocked** — by `command_blocklist` or a hard block. Recorded as skipped with
  the policy reason. Never retried, never worked around.

A declined check is `skipped` with reason `declined_by_user`. The task continues —
declining a check is not an error — and the completion report states that the
check did not run. `Allow Always` remains available and exact-command only.

`require_approval_for_all_commands` is honoured. A user who set it gets a card
even for read-only checks, which is what they asked for.

### 5.4 Behaviour verification, and its honest limit

Requirement 7 asks the report to list behaviour verified through browser
diagnostics. The loop invokes `web_diagnostics` when the task already established
a target page — [spec 12](12_web_app_troubleshooting.md)'s session-scoped
diagnostic approval applies unchanged.

It does not start a dev server. Background processes are Phase 2 WP5, outside this
phase's minimum slice, so a web change whose verification needs a running server
that is not running is reported as **skipped, reason `no_running_target`** — not
as verified, and not as failed. Saying "I could not check this because nothing was
serving the page" is a useful report line; inferring success from an unreachable
page is not.

### 5.5 Repair, bounded

On a failing check, its findings ([spec 22](22_findings_model_and_panel.md)) are
handed to the model with a repair request, bounded by `agent_tool_retry_limit`
(`config.rs:73`):

1. Findings for the failing check are given to the model — findings, not raw log
   output.
2. The model proposes a patch. It goes through the normal preview and approval
   path; repair does not bypass `require_approval_for_file_edits`, and the
   `base_hash` conflict check at `patch_engine.rs:291` still applies.
3. The check reruns.
4. On pass, the step's evidence is the passing `CommandExit`. On fail, attempt
   count increments and the loop returns to 1 until the limit.

Two rules keep the loop from being worse than no loop:

- **No repair attempt without a changed file.** If an attempt produces no applied
  patch, the loop stops rather than spending the remaining attempts asking the
  same question. A model that cannot fix something on attempt one usually cannot
  fix it on attempt three, and the attempts cost real money
  ([spec 19](19_token_and_cost_accounting.md)).
- **Regression guard.** After a repair, checks that previously passed rerun. A
  repair that fixes one check by breaking another is a net loss, and the loop that
  only reruns the failing check would report success. A repair that newly breaks a
  previously passing check is reported as such, and its attempt counts.

On exhausting the limit, the task does **not** succeed. Status is `failed` with
the still-failing findings attached, and the report leads with what still fails.

The token ceiling from [spec 21](21_task_plan_progress_and_budget.md) applies to
repair as to any other work: reaching it stops the loop at a step boundary with
the plan intact.

### 5.6 Partial acceptance invalidates checks

Hunk-level acceptance exists ([spec 04](04_hunk_level_patch_apply.md)), so the
user may accept three of five files after the loop verified all five. The verified
state then no longer exists.

The loop records, per check run, the set of file hashes present when it ran. A
check whose recorded hashes no longer match the working tree is marked
`stale_after_partial_acceptance`, and the report says which checks are no longer
valid for the accepted state. It does not silently rerun them — rerunning without
being asked would spend tokens and time on a state the user may still be editing —
and it does not report them as passed.

This is the same staleness idea as `FindingStatus::Stale` in
[spec 22](22_findings_model_and_panel.md) and the same hash comparison as
`patch_engine.rs:291`, applied to check runs. Phase 2 WP7 builds the review UI
around this; the data it needs is produced here.

### 5.7 The completion report

```text
Files changed        src/upload.rs, tests/upload.rs
Checks passed        cargo build, cargo clippy, cargo test   (all ran, exit 0)
Checks failed        none
Checks skipped       npm run lint:web — no web assets changed
                     browser check — no running target
Behaviour verified   none
Unverified steps     "understand existing retry logic" — no observable evidence
Remaining            Review the retry backoff constant; it was chosen arbitrarily
User action needed   none
```

Construction rules, which are the requirement rather than the formatting:

- **`Checks passed` is derived from `CommandExit` evidence with
  `exit_code == Some(0)`.** Not from the absence of a failure, not from
  `!is_error`, and never from `exit_code.unwrap_or(0)` — `exit_code: None`
  (`command_runner.rs:19`) means a signalled or killed command, and it belongs in
  `Checks failed` with the reason.
- **A check is listed exactly once**, in exactly one of passed, failed, or
  skipped. A check appearing nowhere is a bug the report's own test asserts
  against by comparing the union against the set of proposed checks.
- **`Unverified steps`** comes from [spec 21](21_task_plan_progress_and_budget.md)'s
  steps with empty evidence.
- **The summary line never says "complete"** when anything is in `Checks failed`
  or when the repair limit was exhausted.
- Every path is rendered as a clickable reference
  ([spec 05](05_clickable_file_references.md)).

### 5.8 Eval scenario

The roadmap requires the end-to-end fixture to become an eval scenario so later
phases cannot silently regress it. Added to
[spec 18](18_local_evaluation_harness.md): request a change, plan, edit, fail a
test, repair, pass, review, complete — with the deterministic tier scripting the
model turns via `MockModelAdapter` and asserting the report's passed list contains
only checks that ran and exited zero.

A second scenario asserts the negative: a task whose repair limit is exhausted
produces a report that does not claim success. That one matters more, because it
is the assertion that fails if someone later makes the loop optimistic.

### 5.9 Documentation

`docs/USER_GUIDE.md`: what the loop does after a change, why some checks are
skipped, what the report's sections mean, and that "no checks available" is not
"checks passed". `docs/TROUBLESHOOTING.md`: how to see which checks ran, where
full output lives (`origin_ref` → `stdout.log`), what
`stale_after_partial_acceptance` means, and how to allowlist a check so the loop
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
- Every proposed check appears exactly once across passed, failed, and skipped —
  asserted by comparing the union against the proposed set.
- A project with no detected checks produces a report saying so, distinct from
  checks passing.
- A web verification with no running target is skipped with reason
  `no_running_target`, never inferred as verified.
- Accepting a subset of files marks the affected check runs
  `stale_after_partial_acceptance` and the report says which checks are no longer
  valid.
- Multiple checks requiring approval produce one approval card, not one per check.
- `require_approval_for_all_commands` is honoured for read-only checks.
- The loop does not run in Ask, Plan, or Review mode.
- Verification state survives a restart, since it lives in the task plan.
- The summary line never says "complete" when a check failed or the repair limit
  was exhausted.
- Both eval scenarios from §5.8 exist in
  [spec 18](18_local_evaluation_harness.md) and pass.
- The five quality-gate commands from `AGENTS.md` pass, and the
  [spec 18](18_local_evaluation_harness.md) baseline shows no regression.

## 7. Implementation Notes

To be completed during implementation. Record:

- The file-category-to-check mapping actually shipped, and any project type where
  targeting selected no checks for a change that clearly needed one.
- How often repair succeeded within the limit during real use, and at which
  attempt. If nearly all successes are on attempt one, the limit is doing less
  work than it appears and the "no changed file" stop is doing most of it.
- Whether the regression rerun materially slowed the loop, and the measured cost.
