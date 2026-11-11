# Spec 109 tasks

**Started:** 2026-11-11

Implementation plan for [`proposal.md`](proposal.md). The decisions and the code
survey are in [`context.md`](context.md).

## Global constraints

- One task, one session (AGENTS.md).
- Provenance meanings and the completion gate's requirements do not change;
  every task proves it against the existing provenance and gate tests.
- No command runs without the policy's approval where the policy asks (D5 is the
  only change, and it is task-scoped and restrict-only).
- Never touch the user's checkout to build a pre-patch tree (D2).
- Confirm D2 at the start of Task 3 and D5 with the owner before Task 5.
- Scoped checks per task; workspace gate on CI. Compare timings within one
  checkout only (OBSERVATIONS 86).

| Task | State | Durable result |
|---|---|---|
| 1 · Reuse a result for an unchanged tree (D6) | Done 2026-11-11 | `CheckRun::reusable_for` (stricter than `!is_stale`: same generation **and** the same changed-file map with the same hashes, so a file changed after the run cannot hide; only an exited `Ran` outcome — passed or failed — is reusable; signalled, timed-out, cancelled, environment, skipped and browser runs rerun). `CheckReuse` is built in `verify_due` from the task chain's folded state (minus checks the plan defers to an open step) and passed to `propose_checks` / `propose_completion_checks` / `propose_checks_exactly` / `with_plan_checks`, which drop reused commands from the restricted map before `propose_detected_validations`, so nothing is stored, audited as proposed or run; all-reused is `Selection::Proposed([])`, not "no checks available". New event `verification_reused` (`VerificationEvent::Reused`, also accepted by `read_verification_events`) and audit `verification_check_reused`; it writes no `Check`, so the completion gate and provenance read the original `CheckRun`. `VerificationState::reused` (cleared by `Started` and by a later run) feeds `VerificationReport::reused`; the text report adds a "Checks reused" line (and drops "all ran") only when something was reused; the panel shows the same row. The spec 105 once-per-batch filter in `run_verification_checks` is unchanged — Task 2 owns selection. Tests (`verification_loop`): reusability rules, a reused check is never proposed, fold/report/serialisation, second verification of an unchanged tree runs nothing and the gate evidence is identical, a changed file reruns, a failed run is reused as failed. Checks passed locally: scoped clippy (`-p workspace-engine`); `cargo nextest run -p workspace-engine --test verification_loop --test plan_turn --test closed_task_completion` (239 passed, 19s wall in this worktree); `cargo check -p desktop-shell -p eval-harness --tests`; deterministic eval tier (62/62 pass; needs a local port, so it ran outside the agent sandbox); `scripts/quality-gate.sh fmt node lint typos deny` (deny outside the sandbox, which blocks its advisory-DB lock). Workspace clippy and tests left to CI. The full seven-check gate passed in CI on 2026-11-11 (Build macOS DMG workflow run 43083664918, Quality jobs, on the merged main). |
| 2 · Plan's scoped check per change (D4) | Done 2026-11-11 | `verification::scoped_selection` splits a change's owning roots (`select_roots`) into roots a plan check covers (its `CheckId` directory is the owning root or an enclosing map root) and roots that keep their detected checks; `propose_scoped_checks` proposes the uncovered roots' detected checks as `propose_checks` does and adds the covering plan checks through the same path as `with_plan_checks` (now a shared `append_plan_checks`, with reuse), via the new `ValidationOrchestrator::propose_scoped_plan_check` (same policy path, its own stored reason). With no plan checks it is exactly `propose_checks`. `verify_due`'s per-change branch calls it with the **approved** plan's `required` checks (`plan_required_checks` in `chat.rs`, normalized like `plan_defers_check`); an unapproved plan narrows nothing. Completion branch, answer-time exact floor run, `completion.rs` and `baseline.rs` unchanged. **Decision (recorded in context.md D4): deferral wins** — a covering check deferred to its open step still displaces the detected checks but is not proposed, because a loop `CheckRun` would override the step's later `CommandExit` in the gate; such a verification runs nothing for that root (`Proposed([])`). **Review follow-up (same day): an all-deferred verification says so.** Before, a per-change run whose only covering check was deferred ended `nothing_verified` with the generic summary "No verification has run." — not a false pass, but silent about where the check went. New event `verification_deferred` (`VerificationEvent::Deferred { check, step }`, accepted by `read_verification_events`), appended after `Started` for every check the run leaves to its open step — the D4-displaced plan checks and spec 105's existing deferral filter in `run_verification_checks` (`plan_deferring_step` names the step). It writes no `Check` and is not selected, so it is never gate or `complete_step` evidence. `VerificationState::deferred` (cleared by `Started` and by a later run) feeds `VerificationReport::deferred` (`DeferredCheck { check, step }`, omitted on the wire when empty). With nothing run the summary is "No check ran for this change: `<command>` in `<dir>` is left to its plan step \"<title>\"."; the text report and the panel (`app.js`) add a "Checks deferred" row only when present, so other reports render as before. The outcome stays the existing `nothing_verified` (never `passed`); the turn shows no card and continues the plan. `plan_turn::a_scoped_check_deferred_to_its_open_step_displaces_the_detected_check` now asserts the outcome, the summary and rendered text, that no `Check`/`CheckRun` exists for the deferred check, no check card, the carried report, and that the continued turn's gate reason is only `open_plan_steps`. Discoveries for later tasks: the completion floor of a member change in a virtual workspace is the root's `cargo test` only (spec 90 drops the member's); a scoped check on an early step whose command evidence goes stale when the next step begins is rerun by spec 106 Task 6 at completion — the step's `CommandExit` is not a `CheckRun`, so D6 reuse cannot cover it. Tests: `verification_loop::a_plan_check_covers_its_own_and_enclosed_roots_only`, `verification_loop::a_scoped_plan_check_is_reused_for_an_unchanged_tree`, `plan_turn::a_plans_scoped_check_verifies_a_change_and_the_floor_runs_once_at_completion` (spec 33 shape: the per-change run is only `cargo nextest run -p member --test a --test b`; completion runs the root `cargo test` once; the task completes), `plan_turn::a_scoped_check_deferred_to_its_open_step_displaces_the_detected_check`; a plan without a check keeps today's selection (`a_member_change_runs_only_the_members_check_per_change_and_the_root_check_at_completion`, the planless tests, and the no-plan-check case of the unit test). Checks passed locally (all re-run after the follow-up): scoped clippy (`-p workspace-engine`); `cargo check -p desktop-shell -p eval-harness --tests`; `cargo nextest run -p workspace-engine --test verification_loop --test plan_turn --test closed_task_completion` (243 passed, 18s; again 243 after the follow-up, 27s); deterministic eval tier (62/62, again after the follow-up, outside the sandbox for its local port); `scripts/quality-gate.sh fmt node lint typos deny`. Workspace clippy and tests left to CI. The full seven-check gate passed in CI on 2026-11-11 (Build macOS DMG workflow run 43083664918, Quality jobs, on the merged main). |
| 3 · Lazy baseline on a pre-patch worktree (D1, D2) | Not started | — |
| 4 · Rerun only the failing tests (D3) | Not started | — |
| 5 · One approval per check per task (D5) | Not started | — |
| 6 · Running state and Stop for every running action (D7, D8) | Not started | — |
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

## Task 5 · One approval per check per task (D5)

- [ ] **Confirm D5 with the owner first.**
- [ ] Durable task-chain approval of a check identity; later proposals of the
      same identity in the chain run without a card and are audited
      `approved_for_task`; the card's action reads "Run for this task".
- [ ] Tests: same identity approved once per task chain; a different command or
      directory still asks; a new task asks; a repository cannot widen it; a
      declined check is not remembered.

Files: `validation.rs`, `session.rs`, `chat.rs`, `app.js`, tests.
Checks: scoped clippy; `--test command_policy` (or the policy tests) and
`--test plan_turn`; desktop-shell tests; web checks.

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

## Task 7 · Measurement, documentation and close-out

- [ ] Re-run the spec 33 Task 1 shape on Damaian's own repository and record
      wall time and the checks run per patch against the 2026-11-11 run.
- [ ] `USER_GUIDE.md` (verification and baselines), `TROUBLESHOOTING.md`.
- [ ] Status Done in all four places only after CI is green; README row,
      CHANGELOG Unreleased item, dependency lines; `npm run specs:check -- --strict`.
