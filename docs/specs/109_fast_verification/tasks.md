# Spec 109 tasks

**Started:** 2026-11-11

Implementation plan for [`proposal.md`](proposal.md). The decisions and the code
survey are in [`context.md`](context.md).

## Global constraints

- One task, one session (AGENTS.md).
- Provenance meanings and the completion gate's requirements do not change;
  every task proves it against the existing provenance and gate tests.
- No command runs without the policy's approval where the policy asks (D5 is the
  only change: a session-scoped approval the user chooses, restrict-only).
- Never touch the user's checkout to build a pre-patch tree (D2).
- Confirm D2 at the start of Task 3 and D5's definition with the owner before
  Task 5 (both confirmed 2026-11-12). Task 5 builds on spec 111 Tasks 1–2.
- Scoped checks per task; workspace gate on CI. Compare timings within one
  checkout only (OBSERVATIONS 86).

| Task | State | Durable result |
|---|---|---|
| 1 · Reuse a result for an unchanged tree (D6) | Done 2026-11-11 | `CheckRun::reusable_for` (stricter than `!is_stale`: same generation **and** the same changed-file map with the same hashes, so a file changed after the run cannot hide; only an exited `Ran` outcome — passed or failed — is reusable; signalled, timed-out, cancelled, environment, skipped and browser runs rerun). `CheckReuse` is built in `verify_due` from the task chain's folded state (minus checks the plan defers to an open step) and passed to `propose_checks` / `propose_completion_checks` / `propose_checks_exactly` / `with_plan_checks`, which drop reused commands from the restricted map before `propose_detected_validations`, so nothing is stored, audited as proposed or run; all-reused is `Selection::Proposed([])`, not "no checks available". New event `verification_reused` (`VerificationEvent::Reused`, also accepted by `read_verification_events`) and audit `verification_check_reused`; it writes no `Check`, so the completion gate and provenance read the original `CheckRun`. `VerificationState::reused` (cleared by `Started` and by a later run) feeds `VerificationReport::reused`; the text report adds a "Checks reused" line (and drops "all ran") only when something was reused; the panel shows the same row. The spec 105 once-per-batch filter in `run_verification_checks` is unchanged — Task 2 owns selection. Tests (`verification_loop`): reusability rules, a reused check is never proposed, fold/report/serialisation, second verification of an unchanged tree runs nothing and the gate evidence is identical, a changed file reruns, a failed run is reused as failed. Checks passed locally: scoped clippy (`-p workspace-engine`); `cargo nextest run -p workspace-engine --test verification_loop --test plan_turn --test closed_task_completion` (239 passed, 19s wall in this worktree); `cargo check -p desktop-shell -p eval-harness --tests`; deterministic eval tier (62/62 pass; needs a local port, so it ran outside the agent sandbox); `scripts/quality-gate.sh fmt node lint typos deny` (deny outside the sandbox, which blocks its advisory-DB lock). Workspace clippy and tests left to CI. The full seven-check gate passed in CI on 2026-11-11 (Build macOS DMG workflow run 43083664918, Quality jobs, on the merged main). |
| 2 · Plan's scoped check per change (D4) | Done 2026-11-11 | `verification::scoped_selection` splits a change's owning roots (`select_roots`) into roots a plan check covers (its `CheckId` directory is the owning root or an enclosing map root) and roots that keep their detected checks; `propose_scoped_checks` proposes the uncovered roots' detected checks as `propose_checks` does and adds the covering plan checks through the same path as `with_plan_checks` (now a shared `append_plan_checks`, with reuse), via the new `ValidationOrchestrator::propose_scoped_plan_check` (same policy path, its own stored reason). With no plan checks it is exactly `propose_checks`. `verify_due`'s per-change branch calls it with the **approved** plan's `required` checks (`plan_required_checks` in `chat.rs`, normalized like `plan_defers_check`); an unapproved plan narrows nothing. Completion branch, answer-time exact floor run, `completion.rs` and `baseline.rs` unchanged. **Decision (recorded in context.md D4): deferral wins** — a covering check deferred to its open step still displaces the detected checks but is not proposed, because a loop `CheckRun` would override the step's later `CommandExit` in the gate; such a verification runs nothing for that root (`Proposed([])`). **Review follow-up (same day): an all-deferred verification says so.** Before, a per-change run whose only covering check was deferred ended `nothing_verified` with the generic summary "No verification has run." — not a false pass, but silent about where the check went. New event `verification_deferred` (`VerificationEvent::Deferred { check, step }`, accepted by `read_verification_events`), appended after `Started` for every check the run leaves to its open step — the D4-displaced plan checks and spec 105's existing deferral filter in `run_verification_checks` (`plan_deferring_step` names the step). It writes no `Check` and is not selected, so it is never gate or `complete_step` evidence. `VerificationState::deferred` (cleared by `Started` and by a later run) feeds `VerificationReport::deferred` (`DeferredCheck { check, step }`, omitted on the wire when empty). With nothing run the summary is "No check ran for this change: `<command>` in `<dir>` is left to its plan step \"<title>\"."; the text report and the panel (`app.js`) add a "Checks deferred" row only when present, so other reports render as before. The outcome stays the existing `nothing_verified` (never `passed`); the turn shows no card and continues the plan. `plan_turn::a_scoped_check_deferred_to_its_open_step_displaces_the_detected_check` now asserts the outcome, the summary and rendered text, that no `Check`/`CheckRun` exists for the deferred check, no check card, the carried report, and that the continued turn's gate reason is only `open_plan_steps`. Discoveries for later tasks: the completion floor of a member change in a virtual workspace is the root's `cargo test` only (spec 90 drops the member's); a scoped check on an early step whose command evidence goes stale when the next step begins is rerun by spec 106 Task 6 at completion — the step's `CommandExit` is not a `CheckRun`, so D6 reuse cannot cover it. Tests: `verification_loop::a_plan_check_covers_its_own_and_enclosed_roots_only`, `verification_loop::a_scoped_plan_check_is_reused_for_an_unchanged_tree`, `plan_turn::a_plans_scoped_check_verifies_a_change_and_the_floor_runs_once_at_completion` (spec 33 shape: the per-change run is only `cargo nextest run -p member --test a --test b`; completion runs the root `cargo test` once; the task completes), `plan_turn::a_scoped_check_deferred_to_its_open_step_displaces_the_detected_check`; a plan without a check keeps today's selection (`a_member_change_runs_only_the_members_check_per_change_and_the_root_check_at_completion`, the planless tests, and the no-plan-check case of the unit test). Checks passed locally (all re-run after the follow-up): scoped clippy (`-p workspace-engine`); `cargo check -p desktop-shell -p eval-harness --tests`; `cargo nextest run -p workspace-engine --test verification_loop --test plan_turn --test closed_task_completion` (243 passed, 18s; again 243 after the follow-up, 27s); deterministic eval tier (62/62, again after the follow-up, outside the sandbox for its local port); `scripts/quality-gate.sh fmt node lint typos deny`. Workspace clippy and tests left to CI. The full seven-check gate passed in CI on 2026-11-11 (Build macOS DMG workflow run 43083664918, Quality jobs, on the merged main). **Fix (2026-11-12): a finished step's check is not a per-change check.** As landed, `scoped_selection` took every approved required check covering the root while `plan_defers_check` defers only open steps' checks, so each finished step's check was proposed and run after every later patch (the owner's spec 33 plan: 5 steps, every check at the repository root); spec 106 Task 6's `plan_checks_needing_rerun` was also added to every post-apply verification, a second per-change path for the same checks. Now: `scoped_selection(map, changed, plan_checks, current)` — any approved required check still marks a root covered (no detected check per change), but only `current`, the in-progress step's required checks (`plan_current_step_checks` in `chat.rs`), stands in; a covered root with no current covering check goes to `ScopedSelection::completion_roots`. `propose_scoped_checks` now takes the `ScopedSelection` (clippy's argument limit) and runs nothing for those roots. Their detected checks (`detected_checks_of_roots`, nothing stored) are recorded as the new `verification_deferred_to_completion` event (`VerificationEvent::DeferredToCompletion`, `VerificationState`/`VerificationReport::deferred_to_completion`); the summary says "`<check>` is left to task completion" and the text report and panel share the "Checks deferred" row. `verify_due` adds `plan_checks_needing_rerun` only to a completion run (all primary steps terminal, or the answer-time due). To keep the worker's continuation moving, `CompletionGateReport::evaluate` no longer reports a finished step's stale check as `required_check_stale` while primary plan work is open or blocked — the same treatment as the floor; a failed or missing one still is, and completion requirements are unchanged (an open step already makes the task ineligible; the answer-time run reruns the stale check once, as before). D4, spec 105's Task 6 note and `USER_GUIDE.md` record the rule. Tests: `plan_turn::a_finished_steps_check_is_not_rerun_per_change_only_once_at_completion` (replaces `a_plans_scoped_check_verifies_a_change_and_the_floor_runs_once_at_completion`, which asserted the defect: three steps, every check at the root, driven through the model — a patch in step 2 runs no step-1 check and reports step 2's as left to its step; a patch in step 3, which has no check, runs nothing and reports `cargo test in crates/member` left to completion; both gates read only `open_plan_steps`; completion reruns each finished step's check once and the root floor once, and the task completes); `plan_turn::a_check_made_stale_by_a_later_step_is_rerun_once_at_completion` (renamed: 0 per-change runs, 1 at completion); `verification_loop::a_plan_check_covers_its_own_and_enclosed_roots_only` gains the finished-step and mixed cases; `closed_task_completion::every_blocking_fact_produces_its_stable_reason_code` asserts the stale reason only once no step is open; the four R14 baseline/provenance tests in `plan_turn` close the last step before patch 2's verification (`close_the_last_step`), because the loop's run of the finished "Validate" check now happens in the completion run. Checks passed locally for the fix: scoped clippy for `workspace-engine`, `desktop-shell` and `eval-harness`; `cargo nextest run -p workspace-engine --test verification_loop --test plan_turn --test closed_task_completion` (246 passed, 17s); deterministic eval tier (64/64 pass, outside the sandbox for its local port); `scripts/quality-gate.sh fmt node lint typos` and `cargo deny check` (outside the sandbox for its advisory-DB lock). Workspace clippy and tests left to CI. |
| 3 · Lazy baseline on a pre-patch worktree (D1, D2) | Not started | — |
| 4 · Rerun only the failing tests (D3) | Not started | — |
| 5 · Approve for this session (D5) | Not started | — |
| 6 · Running state and Stop for every running action (D7, D8) | Not started | — |
| 8 · Verification with no detected check (D9) | Not started | — |
| 7 · Measurement, documentation and close-out | Not started | — |

## Task 1 · Reuse a result for an unchanged tree (D6)

- [x] Before proposing a check, reuse a non-stale `CheckRun` of the same
      identity; record the reuse in the verification events and the report.
- [x] Tests: a second verification of an unchanged tree runs nothing; any file
      change or mutation generation makes it run.

Files: `verification.rs`, `chat.rs`, tests. Checks: scoped clippy;
`cargo nextest run -p workspace-engine --test verification_loop --test plan_turn`.

## Task 2 · Plan's scoped check per change (D4)

- [x] Per-change selection prefers the plan's required check for the owning (or
      enclosing) root over the detected root check; the floor still runs once
      at completion.
- [x] Tests: the spec 33 shape (plan with `cargo nextest … --test a --test b`
      in the crate) verifies a patch with the scoped check only; completion runs
      the floor; a plan without a check keeps today's selection.

Files: `chat.rs`, `verification.rs`, `completion.rs` (if the gate's evidence
lookup needs it), tests. Checks: as Task 1 plus `--test closed_task_completion`.

## Task 3 · Lazy baseline on a pre-patch worktree (D1, D2)

- [ ] Confirm D2 with the owner.
- [ ] Remove the pre-apply baseline card and R18's pre-command baseline; take a
      baseline only when a check fails and the task has none for it.
- [ ] Build the pre-patch tree in a temporary worktree (task-start uncommitted
      state + the task's earlier patches, hash-verified); `Unavailable` when it
      cannot be reconstructed; always cleaned up.
- [ ] Tests: the spec 102 provenance fixtures produce the same classifications;
      a passing check never baselines; a reconstruction mismatch yields
      `UnknownProvenance`; the user's checkout is untouched.

Files: `baseline.rs`, `edit.rs`, `verification.rs`, `chat.rs`, tests.
Checks: scoped clippy; `--test repair_baseline --test verification_loop
--test plan_turn`; `cargo run -p eval-harness -- run --tier deterministic`.

## Task 4 · Rerun only the failing tests (D3)

- [ ] For Rust test / nextest findings, the failure-time baseline runs only the
      failing tests by name; other checks rerun whole.
- [ ] Tests: two failing tests rerun alone; a mixed failure (one pre-existing,
      one introduced) classifies as worsened with the right introduced finding.

Files: `baseline.rs`, `finding/`, tests. Checks: as Task 3.

## Task 5 · Approve for this session (D5)

- [x] **Confirm with the owner** D5's definition of a command that does not
      change the repository: decided by the policy's approval causes and each
      run's observed effect, with no ecosystem list, plus a user config entry
      (confirmed 2026-11-12).
- [ ] Policy: `session_approvable(command, dir)` from the command's
      `ApprovalCause`s and `is_source_writing_command` (D5), with no ecosystem
      list; tests for each list
      entry and each refusal reason.
- [ ] Built on spec 111 Tasks 1–2: **Approve for this session** writes a
      session-scoped `allow` rule of the exact parsed command and directory;
      user additions are ordinary spec 111 user rules.
- [ ] Session events and the approval path: choosing it records the approval;
      later proposals of the same command and directory in the session run
      without a card, audited; repository config can only disable it.
- [ ] Backstop: before/after repository state around a covered run; a change
      revokes the approval and is reported.
- [ ] UI: the Approve chevron with **Approve for this session** on command,
      check and baseline cards; the run row says so; **Revoke session
      approvals** in the session ⋮.
- [ ] Tests: approved once per session; another command, directory or session
      still asks; a `&&` chain, a `--fix` and an unknown command are never
      offered; unknown programs from several ecosystems (`stack test`, `lein
      test`, `go test`, `phpunit`, `ctest`) are offered and a `--fix` variant
      is not; a covered run that changes a file revokes it; a restart keeps
      it; archive ends it; a repository cannot enable or widen it.

Files: `command_policy.rs`, `validation.rs`, `session.rs`, `chat.rs`,
`app.js`, desktop-shell `routes.rs` (classify any new route), tests.
Checks: scoped clippy; the policy tests and `--test plan_turn`;
`cargo nextest run -p desktop-shell`; web checks.

## Task 6 · Running state and Stop for every running action (D7, D8)

- [ ] One running-state component for checks and baselines: header indicator,
      Cancel in place of the run button, status line and composer line from the
      same state.
- [ ] D8: a visible **Stop** for every running action, including the
      post-apply verification started from the patch card, commands, hooks, MCP
      calls and browser diagnostics; all runners honour the turn's cancel token
      by ending their process group; a stopped side-effecting action is recorded
      with an unknown outcome and named in the report; nothing retries
      automatically.
- [ ] Tests per runner: Stop ends the process group, records the outcome, and
      leaves no running child (process registry empty).
- [ ] Manual check in the running app; UI tests.

Files: `app.js`, `style.css`, `lib.rs` (if the stream needs a field), tests.
Checks: web checks; `cargo nextest run -p desktop-shell`.

## Task 8 · Verification with no detected check (D9)

Build before Task 7.

- [ ] Completion floor: a touched root with no detected check takes the
      approved plan's required checks covering it.
- [ ] User-scope `project_checks` (exhaustive overlay classification; a
      repository may only narrow), added to the floor; editable in Settings.
- [ ] Planning prompt: name the repository's declared checks (CI files,
      `Makefile`/`justfile`, README) as single commands and say where they came
      from.
- [ ] Tests: a fixture repository with only a `Makefile` (and one with a
      `stack.yaml`) is verified by its plan's checks, not "no checks
      available"; a `project_checks` entry joins the floor; a repository value
      cannot add one; the deterministic eval gains one such scenario (update
      both scenario counts in `eval-harness/tests/harness.rs`).

Files: `completion.rs`, `verification.rs`, `config.rs`, the planning prompt,
`app.js`, tests, eval scenario. Checks: scoped clippy; `--test
closed_task_completion --test verification_loop --test plan_turn`;
`cargo nextest run -p desktop-shell`; deterministic eval.

## Task 7 · Measurement, documentation and close-out

- [ ] Re-run the spec 33 Task 1 shape on Damaian's own repository and record
      wall time and the checks run per patch against the 2026-11-11 run.
- [ ] `USER_GUIDE.md` (verification and baselines), `TROUBLESHOOTING.md`.
- [ ] Status Done in all four places only after CI is green; README row,
      CHANGELOG Unreleased item, dependency lines; `npm run specs:check -- --strict`.
