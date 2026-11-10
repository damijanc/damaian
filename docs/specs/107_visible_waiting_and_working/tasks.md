# Spec 107 tasks

**Started:** not yet — specified 2026-11-10

Implementation plan for [`proposal.md`](proposal.md). The decisions and the code
survey are in [`context.md`](context.md).

## Global constraints

- One task, one session (AGENTS.md).
- No change to what needs approval. A pending decision is still answered only
  through its own route; a question is not an approval.
- Web assets stay vanilla JavaScript; Biome lint and `node --check` must pass.
- Verify UI changes in the running app, as `docs/DEVELOPMENT.md` describes for
  `include_str!`-embedded assets (rebuild, restart, then check), and record what
  was checked in the task row.
- Confirm D3 with the owner before Task 5; D1, D2, D4 and D5 are
  recommendations to confirm when their task starts.
- Scoped checks per task; the workspace gate stays with CI.

| Task | State | Durable result |
|---|---|---|
| 1 · Engine: pending-decision presentation state | Not started | — |
| 2 · Pending cards above the composer, and the composer line | Not started | — |
| 3 · Status line and plan label while waiting | Not started | — |
| 4 · Baseline progress and cancel | Not started | — |
| 5 · Ordinary Code-mode continuation (D3) | Not started | — |
| 6 · Documentation and close-out | Not started | — |

## Task 1 · Engine: pending-decision presentation state

Requirements 1–4; context D1.

- [ ] Add `pendingDecision { kind, label, cardId }` to the turn presentation
      (`task_control.rs`) for every waiting kind, and to `/api/session`'s
      payload for reload (`lib.rs`).
- [ ] `plan_json` carries `waiting: true` for a task waiting on a decision.
- [ ] Tests: each pause kind yields its `pendingDecision`; a finished turn
      yields none; reload yields the same value as the live stream.

Files: `crates/workspace-engine/src/task_control.rs`, `session.rs` (if needed),
`crates/desktop-shell/src/lib.rs`, their tests.
Checks: `cargo clippy -p workspace-engine -p desktop-shell --all-targets --locked -- -D warnings`;
`cargo nextest run -p workspace-engine --test task_control` and the desktop-shell
session-route tests; `cargo fmt --all -- --check`.

## Task 2 · Pending cards above the composer, and the composer line

Requirements 1, 2; context D2, D5.

- [ ] `appendProposals` and `proposePatchFromChat` place the pending card at the
      end of `#chat-log` (spec 93's rule), leaving a one-line pointer in the
      bubble; scroll it into view.
- [ ] A composer line names the pending decision and focuses its card.
- [ ] While a question is pending the main composer is disabled with "Answer the
      question above" (D2).
- [ ] UI tests per kind, and a manual check in the running app.

Files: `crates/desktop-shell/static/app.js`, `index.html`, `style.css`,
desktop-shell UI tests.
Checks: `node --check crates/desktop-shell/static/app.js`; `npm run lint:web`;
`cargo nextest run -p desktop-shell`.

## Task 3 · Status line and plan label while waiting

Requirements 3, 4.

- [ ] `startTurnIndicator`/`finish` keep the row with a waiting label for
      `Pending`; remove it only when the turn ends.
- [ ] While working, the row names the current activity from the latest
      progress event (tool and target, or the running command) and shows that
      step's elapsed time apart from the turn's total; where the provider
      streams reasoning, at most a one-line summary (requirement 3, added
      2026-11-10).
- [ ] `renderPlanPanel` shows **waiting for you** when `plan.waiting`.
- [ ] Tests and a manual check (question, patch, round limit, a read, a
      running command).

Files: `app.js`, `style.css`, desktop-shell tests.
Checks: as Task 2.

## Task 4 · Baseline progress and cancel

Requirement 5; context D4.

- [ ] `run_baseline` takes a caller's cancel token and output callback;
      `handle_resume_baseline` streams phase events and honours cancel.
- [ ] The baseline card shows the running check and elapsed time, and a
      **Cancel** control; a cancelled baseline is recorded as declined.
- [ ] Tests: progress events in order; cancel records a declined baseline and
      leaves the patch undecided; no change for a completed run.

Files: `crates/workspace-engine/src/baseline.rs`, `edit.rs` (resume path),
`crates/desktop-shell/src/lib.rs`, `app.js`, tests.
Checks: `cargo clippy -p workspace-engine -p desktop-shell --all-targets --locked -- -D warnings`;
`cargo nextest run -p workspace-engine --test verification_loop` (or the
baseline test file) and the desktop-shell route tests; web checks as Task 2.

## Task 5 · Ordinary Code-mode continuation (D3)

Requirement 6; context D3. **Confirm D3 with the owner first.**

- [ ] Give an ordinary Code-mode turn a segment tracker with the default task
      budget so `decide_segment_continuation` applies; keep Ask/Plan/Review on
      the 16-round pause.
- [ ] A turn that stops making progress or reaches a ceiling still shows the
      existing card.
- [ ] A continuing ordinary turn without a plan continues after a verified patch,
      and its patches verify per-change (owning root only); the completion floor
      runs once at the answer (context D3, added 2026-11-10).
- [ ] Tests: 40 progressing rounds pause zero times; a no-progress turn pauses;
      Ask mode still pauses at 16; budgets stay restrict-only; a planless
      two-patch turn runs the owning crate's check per patch and the root check
      once at the end, and never ends `incomplete` on a missing root check.

Files: `crates/workspace-engine/src/chat.rs`, `task_segments.rs`, tests.
Checks: `cargo clippy -p workspace-engine --all-targets --locked -- -D warnings`;
`cargo nextest run -p workspace-engine --test task_budget --test plan_turn`;
`cargo run -p eval-harness -- run --tier deterministic`.

## Task 6 · Documentation and close-out

- [ ] `USER_GUIDE.md`: where a decision appears, the composer line, waiting
      labels, baseline progress and cancel, ordinary-session continuation.
- [ ] `TROUBLESHOOTING.md`: "Damaian looks stopped" — reading the status line.
- [ ] Status Done in all four places only after CI is green; README row,
      CHANGELOG Unreleased item, dependency lines; `npm run specs:check -- --strict`.
