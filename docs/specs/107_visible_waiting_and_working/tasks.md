# Spec 107 tasks

**Started:** 2026-11-10

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
| 1 · Engine: pending-decision presentation state | Done 2026-11-10 | `TurnPresentation.pendingDecision { kind, label, statusLabel, cardId }` (`task_control.rs`: `PendingDecisionKind`, `TurnPendingDecision`, pure `pending_decision_presentation`, durable reader `TaskControlService::pending_decision`). One reader serves both paths: every live `ChatTurnResult` goes through `ChatOrchestrator::turn_presentation_for` (`chat.rs`), and `/api/session` fills each task's `turnPresentation.pendingDecision` from the same call (`lib.rs` `handle_session`), so reload equals live by construction. The reader takes the newest status event's pause link (`SessionStore::pending_decision_for`) and only for `waiting_for_approval`. `plan_json_with_waiting` adds `plan.waiting`, true exactly when the task has a `pendingDecision`. **Deviations from D1:** (1) an added `statusLabel` ("Waiting for your answer/review/approval/decision") so Task 3 renders the status line from engine text; (2) two more kinds than D1 lists, `context_review` and `fetch`, because the engine has those pauses too (the durable `provider_fallback` link presents as `fallback`); (3) `baseline` has no link of its own: it is a waiting `patch` whose apply is held on a pending baseline card (new `baseline::pending_baseline_for_patch`), and its `cardId` is the baseline batch id; `POST /api/apply-patch` and `/api/resume-baseline` add `pendingDecision` (`baseline_pending_decision`) whenever they return a card, because that card arrives outside a turn; (4) a patch whose files were all applied or rejected one by one (spec 93's `patch_already_decided`, now `pub(crate)`) and an unknown link kind yield no decision rather than a guess. **For Tasks 2–3:** a live `plan` SSE event always carries `waiting: false` (the plan is streamed before the pause is written), so the live UI takes the waiting state from the `done` event's `turnPresentation.pendingDecision`; check counts in the label come from the paused-turn file (`PausedTurns::check_batch_proposals`); a command's spec 102 R18 baseline card is a `checks` decision, not `baseline`. **Tests:** `workspace-engine --test task_control` 23/23 (new: plan review → command approval → settled turn has none, question, patch, round limit, plain answer, and the checks/fallback/context-review/fetch links, each asserting live equals reload); two new `task_control.rs` unit tests (every link's kind, labels, card id and wire spelling; no field when nothing is pending); desktop-shell: the baseline route test now asserts the patch → baseline → patch decision through `/api/session` and the apply response, and a new reload test covers `plan.waiting` and the field. **Checks run locally, all passing:** `cargo clippy -p workspace-engine -p desktop-shell --all-targets --locked -- -D warnings`; `scripts/quality-gate.sh fmt node lint typos deny`; `cargo nextest run -p workspace-engine` `--test task_control`, `--test round_budget --test clarification` (58), `--test repair_baseline --test verification_loop --test deferred_check_waiver --test plan_turn` (217), `--lib` filtered to task_control/baseline/recovery (17); `cargo nextest run -p desktop-shell` (289 passed, 20 ignored side-effect tests skipped). Desktop-shell tests bind local ports, which this agent's sandbox refuses, so they ran outside it. Workspace clippy and the full test suite are left to CI. |
| 2 · Pending cards above the composer, and the composer line | Not started | — |
| 3 · Status line and plan label while waiting | Not started | — |
| 4 · Baseline progress and cancel | Not started | — |
| 5 · Ordinary Code-mode continuation (D3) | Not started | — |
| 6 · Documentation and close-out | Not started | — |

## Task 1 · Engine: pending-decision presentation state

Requirements 1–4; context D1.

- [x] Add `pendingDecision { kind, label, cardId }` to the turn presentation
      (`task_control.rs`) for every waiting kind, and to `/api/session`'s
      payload for reload (`lib.rs`).
- [x] `plan_json` carries `waiting: true` for a task waiting on a decision.
- [x] Tests: each pause kind yields its `pendingDecision`; a finished turn
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
