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
- D3 was decided by the owner on 2026-11-10 (A and C). D1, D2, D4 and D5 are
  recommendations to confirm when their task starts.
- Scoped checks per task; the workspace gate stays with CI.

| Task | State | Durable result |
|---|---|---|
| 1 · Engine: pending-decision presentation state | Done 2026-11-10 | `TurnPresentation.pendingDecision { kind, label, statusLabel, cardId }` (`task_control.rs`: `PendingDecisionKind`, `TurnPendingDecision`, pure `pending_decision_presentation`, durable reader `TaskControlService::pending_decision`). One reader serves both paths: every live `ChatTurnResult` goes through `ChatOrchestrator::turn_presentation_for` (`chat.rs`), and `/api/session` fills each task's `turnPresentation.pendingDecision` from the same call (`lib.rs` `handle_session`), so reload equals live by construction. The reader takes the newest status event's pause link (`SessionStore::pending_decision_for`) and only for `waiting_for_approval`. `plan_json_with_waiting` adds `plan.waiting`, true exactly when the task has a `pendingDecision`. **Deviations from D1:** (1) an added `statusLabel` ("Waiting for your answer/review/approval/decision") so Task 3 renders the status line from engine text; (2) two more kinds than D1 lists, `context_review` and `fetch`, because the engine has those pauses too (the durable `provider_fallback` link presents as `fallback`); (3) `baseline` has no link of its own: it is a waiting `patch` whose apply is held on a pending baseline card (new `baseline::pending_baseline_for_patch`), and its `cardId` is the baseline batch id; `POST /api/apply-patch` and `/api/resume-baseline` add `pendingDecision` (`baseline_pending_decision`) whenever they return a card, because that card arrives outside a turn; (4) a patch whose files were all applied or rejected one by one (spec 93's `patch_already_decided`, now `pub(crate)`) and an unknown link kind yield no decision rather than a guess. **For Tasks 2–3:** a live `plan` SSE event always carries `waiting: false` (the plan is streamed before the pause is written), so the live UI takes the waiting state from the `done` event's `turnPresentation.pendingDecision`; check counts in the label come from the paused-turn file (`PausedTurns::check_batch_proposals`); a command's spec 102 R18 baseline card is a `checks` decision, not `baseline`. **Tests:** `workspace-engine --test task_control` 23/23 (new: plan review → command approval → settled turn has none, question, patch, round limit, plain answer, and the checks/fallback/context-review/fetch links, each asserting live equals reload); two new `task_control.rs` unit tests (every link's kind, labels, card id and wire spelling; no field when nothing is pending); desktop-shell: the baseline route test now asserts the patch → baseline → patch decision through `/api/session` and the apply response, and a new reload test covers `plan.waiting` and the field. **Checks run locally, all passing:** `cargo clippy -p workspace-engine -p desktop-shell --all-targets --locked -- -D warnings`; `scripts/quality-gate.sh fmt node lint typos deny`; `cargo nextest run -p workspace-engine` `--test task_control`, `--test round_budget --test clarification` (58), `--test repair_baseline --test verification_loop --test deferred_check_waiver --test plan_turn` (217), `--lib` filtered to task_control/baseline/recovery (17); `cargo nextest run -p desktop-shell` (289 passed, 20 ignored side-effect tests skipped). Desktop-shell tests bind local ports, which this agent's sandbox refuses, so they ran outside it. Workspace clippy and the full test suite are left to CI. |
| 2 · Pending cards above the composer, and the composer line | Done 2026-11-10 | `app.js`: `appendProposals` routes each proposal through `place(field, card)`; the one named by `turnPresentation.pendingDecision` (map `PENDING_DECISION_FIELDS`, kind → payload field; `baseline` → `patchProposal`, since the baseline card lives in the patch card) goes to `setPendingDecision`, which wraps it in `section.pending-decision` (`data-kind`, `data-card-id`, `data-waiting`), appends it as the last child of `#chat-log`, scrolls it into view (`block: "nearest"`) and leaves a one-line pointer button in the bubble ("<label> — the card is below."). Every `done` passes through `appendProposals`, so a turn with no decision clears the last one; a decided card stays in the history with `data-waiting="false"`. A patch settles its own decision in `onResolved`; an apply or baseline response's `pendingDecision` renames it (`notePendingDecisionChange`, from the patch card's `render`). Composer line `#composer-decision` (`index.html`, above the prompt box) shows `pendingDecision.label` with **Show** / **Answer** (`focusPendingDecision`), hidden while a turn runs (`syncComposerDecision`, called from `setComposerBusy`). D2 as recommended: a pending question disables `#chat-prompt` and Send with the placeholder "Answer the question above"; **Answer** focuses the free-text field, else the first option. Reload: `loadSession` calls `restorePendingDecision(payload.tasks)` last (after recovery cards and the memory stack); `renderMemoryProposals` inserts above a pending card; `renderMessages`/`clearChat` reset the state. **Deviations:** (1) `proposePatchFromChat` (`/api/propose-edit`, not a turn, no `turnPresentation`) uses the response's `pendingDecision` if present, else a UI-built `{ kind: "patch", label: "Review the patch" }` — the route only ever returns one patch; (2) after a restart the recovery sweep reattaches a waiting decision as a spec 45 card for the same task; that card is the pending one and the transcript question stays where it was asked, so a restarted question still shows two answer cards (pre-existing duplication, not fixed here — candidate observation); (3) within one shell run, a reloaded patch/checks/command decision has no card to move (`/api/session` carries no proposal payloads; only questions and swept approvals re-render), so the composer line names it with no **Show** button; (4) the composer line and the lock are off in a worker's view, whose composer the worker owns (spec 57). **Tests:** desktop-shell `app_js_places_every_pending_decision_kind_above_the_composer` (exhaustive match over `PendingDecisionKind`: map entry, `place(...)`, no direct bubble append, per kind), `app_js_restores_the_pending_decision_last_on_reload`, `app_js_composer_names_the_pending_decision_and_yields_to_a_question`; `serves_the_ui_for_manual_inspection` now seeds a pending-question session. **Checked in the running UI** (the inspection server on port 4899 with its own temp data dir, rebuilt after each change, driven from the browser): live question, round budget, command and plan payloads each render as the last `#chat-log` child with the composer line naming the engine label; a question disables the composer and Send, and a following decision or a turn with none releases it; a turn in flight hides the line and leaves Stop enabled; the memory stack lands above the pending card; reload of the seeded session moves the question below everything else, and **Answer** focuses "Your answer" (a first check found focus on the option button; fixed). **Checks run locally, all passing:** `node --check`; `npm run lint:web` (one existing `noDescendingSpecificity` warning in `style.css`, present before this change); `cargo clippy -p desktop-shell --all-targets --locked -- -D warnings`; `cargo nextest run -p desktop-shell` (292 passed, 20 ignored skipped; outside the sandbox, which refuses local ports); `scripts/quality-gate.sh fmt node lint typos deny` (`deny` outside the sandbox, which refuses its advisory-db lock). Workspace clippy and the full suite are left to CI. **For Task 3:** the live `done` payload's `pendingDecision` is now stored in `pendingDecision` (module state in `app.js`) — read its `decision.statusLabel` there rather than re-deriving. |
| 3 · Status line and plan label while waiting | Done 2026-11-10 | `app.js`: `startTurnIndicator` now shows the current activity with its own clock (`.turn-indicator-elapsed`, restarted by every non-`thinking` phase and by streaming) and the turn's total apart from it (`.turn-indicator-total`, "turn 3m 4s", hidden until the first activity changes); clocks use `formatClock` ("40s", "1m 12s", "1h 3m"); the model phase reads "Waiting for the model". `finish("pending")` with a `pendingDecision` keeps the row as `data-state="waiting"` (`showWaitingIndicator`: engine `statusLabel`, a waiting clock, Stop and command output removed). One waiting row at a time (module `waitingIndicator`); it goes on `settlePendingDecision`/`resetPendingDecision` and when the next turn's `startTurnIndicator` runs; `notePendingDecisionChange` renames it (patch ↔ baseline); `restorePendingDecision` recreates it under the bubble on reload (`restoreWaitingIndicator`). `renderPlanPanel` labels the open step via `planStepLiveLabel` (**waiting for you** / running / not finished, `.plan-step-live[data-waiting]`); live, `setPendingDecision` marks the raising bubble's panel waiting in place (`markPlanWaiting`, so an opened Steps list stays open), since live `plan` events always carry `waiting: false`. `style.css`: waiting row (still dot, warn colour) and plan label. **Deviations / engine fields added:** (1) `pending` alone does not make a waiting row: it is also every route's default when a payload has no presentation, so without a `pendingDecision` the row is removed as before; (2) engine labels (`chat.rs`): a ranged read is "Reading <path> <start>–<end>", and a command that runs emits a new `tool` phase "Running `<cmd>`" (`running_command_label`) before its output, on the auto-run path, the approved-command resume path and verification checks — before this the label said "Proposing" throughout the run (a worker's progress log now also gets this `tool` line); (3) reasoning: the one-line summary is spec 81's existing "Thinking · 2.1k chars"; the phase carries only a count, so no reasoning text is shown or added; (4) the old bubble's plan label returns to its previous value when its decision settles ("running" for a still-active plan payload) — a history panel showing a stale "running" predates this task (candidate observation). **Tests:** workspace-engine lib `activity_labels_name_a_read_range_and_a_running_command`; desktop-shell `app_js_status_line_waits_with_the_engine_label_until_the_turn_ends`, `app_js_status_line_times_the_current_activity_apart_from_the_turn`, `app_js_plan_step_reads_waiting_for_you_while_a_decision_is_pending`; the worker `progress_lines_mirror_the_turn` now asserts "Proposing `…`" precedes "Running `…`" for an auto-run command. **Checked in the running UI** (inspection server on port 4899, its own temp data dir, driven from the browser): a scripted turn showed "Assembling context 2s", "Thinking · 2.1k chars 2s · turn 4s", "Reading src/config.rs 1200–1600 · round 2/16 · 1 read 1s · turn 5s", "Running `cargo test --test permission_profiles` 2s · turn 7s" with its output line; pausing on a question, a patch, a round limit and a baseline each left exactly one waiting row with the engine label and a clock, no Stop button, and the composer line naming the decision; the question's plan step read **waiting for you** (Steps list stayed open); each next turn's start removed the previous row; a turn completing with nothing pending left none; `pending` with no decision left none; reload of the seeded pending-question session restored the row under its bubble with the card last. The real engine labels (ranged read, running command) were checked by the tests above, not by a live model turn. **Checks run locally, all passing:** `cargo clippy -p workspace-engine -p desktop-shell --all-targets --locked -- -D warnings`; `cargo nextest run -p desktop-shell` (295 passed, 20 ignored skipped; outside the sandbox, which refuses local ports); `cargo nextest run -p workspace-engine` `--test foundation --test steering --test task_outcomes --test task_segments --test task_control --test round_budget --test verification_loop --test agent_tools` (399; the FSEvents watcher test fails only inside the sandbox and passed outside it) and the new lib test; `scripts/quality-gate.sh fmt node lint typos deny` (`npm run lint:web` keeps the one existing `style.css` warning). Workspace clippy and the full suite are left to CI. |
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

- [x] `appendProposals` and `proposePatchFromChat` place the pending card at the
      end of `#chat-log` (spec 93's rule), leaving a one-line pointer in the
      bubble; scroll it into view.
- [x] A composer line names the pending decision and focuses its card.
- [x] While a question is pending the main composer is disabled with "Answer the
      question above" (D2).
- [x] UI tests per kind, and a manual check in the running app.

Files: `crates/desktop-shell/static/app.js`, `index.html`, `style.css`,
desktop-shell UI tests.
Checks: `node --check crates/desktop-shell/static/app.js`; `npm run lint:web`;
`cargo nextest run -p desktop-shell`.

## Task 3 · Status line and plan label while waiting

Requirements 3, 4.

- [x] `startTurnIndicator`/`finish` keep the row with a waiting label for
      `Pending`; remove it only when the turn ends.
- [x] While working, the row names the current activity from the latest
      progress event (tool and target, or the running command) and shows that
      step's elapsed time apart from the turn's total; where the provider
      streams reasoning, at most a one-line summary (requirement 3, added
      2026-11-10).
- [x] `renderPlanPanel` shows **waiting for you** when `plan.waiting`.
- [x] Tests and a manual check (question, patch, round limit, a read, a
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

Requirement 6; context D3 (decided by the owner 2026-11-10: A and C).

- [ ] With Worker sessions on, suggest a worker once per code-changing task at
      its first change (a code-changing plan's review, or a planless task's
      first patch): **Run in a worker** / **Continue here**. Not for a planning-
      or documentation-only task, not in a worker; the choice is remembered for
      the task. **Run in a worker** reuses spec 94's handoff (New worker dialog
      prefilled).
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
      once at the end, and never ends `incomplete` on a missing root check;
      the worker suggestion appears once for a code-changing task and never for
      a docs-only plan or in a worker.

Files: `crates/workspace-engine/src/chat.rs`, `task_segments.rs`, tests; for
the suggestion, `crates/desktop-shell/static/app.js` and the shell payload.
Checks: `cargo clippy -p workspace-engine --all-targets --locked -- -D warnings`;
`cargo nextest run -p workspace-engine --test task_budget --test plan_turn`;
`cargo run -p eval-harness -- run --tier deterministic`.

## Task 6 · Documentation and close-out

- [ ] `USER_GUIDE.md`: where a decision appears, the composer line, waiting
      labels, baseline progress and cancel, ordinary-session continuation.
- [ ] `TROUBLESHOOTING.md`: "Damaian looks stopped" — reading the status line.
- [ ] Status Done in all four places only after CI is green; README row,
      CHANGELOG Unreleased item, dependency lines; `npm run specs:check -- --strict`.
