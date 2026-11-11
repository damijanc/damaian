# Spec 109 tasks

**Started:** not yet — specified 2026-11-11

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
| 1 · Reuse a result for an unchanged tree (D6) | Not started | — |
| 2 · Plan's scoped check per change (D4) | Not started | — |
| 3 · Lazy baseline on a pre-patch worktree (D1, D2) | Not started | — |
| 4 · Rerun only the failing tests (D3) | Not started | — |
| 5 · One approval per check per task (D5) | Not started | — |
| 6 · Running state and Stop for every running action (D7, D8) | Not started | — |
| 7 · Measurement, documentation and close-out | Not started | — |

## Task 1 · Reuse a result for an unchanged tree (D6)

- [ ] Before proposing a check, reuse a non-stale `CheckRun` of the same
      identity; record the reuse in the verification events and the report.
- [ ] Tests: a second verification of an unchanged tree runs nothing; any file
      change or mutation generation makes it run.

Files: `verification.rs`, `chat.rs`, tests. Checks: scoped clippy;
`cargo nextest run -p workspace-engine --test verification_loop --test plan_turn`.

## Task 2 · Plan's scoped check per change (D4)

- [ ] Per-change selection prefers the plan's required check for the owning (or
      enclosing) root over the detected root check; the floor still runs once
      at completion.
- [ ] Tests: the spec 33 shape (plan with `cargo nextest … --test a --test b`
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
