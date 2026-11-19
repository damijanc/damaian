# Spec 112 tasks

**Started:** 2026-11-19

Implementation plan for [`proposal.md`](proposal.md). The evidence, the code
survey and the decisions are in [`context.md`](context.md).

## Global constraints

- One task, one session (AGENTS.md).
- Build before spec 110 Task 3 and never at the same time as another spec that
  edits `app.js` or `style.css` (context D4).
- No change to approval policy or to the routes that resolve a decision.
- Web assets stay vanilla JavaScript; `node --check` and Biome lint pass.
  Every UI change is checked in the running app
  (`serves_the_ui_for_manual_inspection` on its own port and data dir,
  stopped by PID) and the result is recorded in the task row.
- `docs/UI_STYLE_GUIDE.md` and `docs/ui-style-guide.html` change in the same
  task as the surface they describe.
- Scoped checks per task; the workspace gate stays with CI.

| Task | State | Durable result |
|---|---|---|
| 1 · Classes and tokens (req 8) | Done 2026-11-19 | No behaviour change. `app.js`: memory card Remember `btn-sm btn-primary`, Dismiss `btn-sm btn-quiet` (was `btn-secondary`, which had no rule); *Compact now* `btn-sm btn-quiet`; the two context-panel pin controls use a new `icon("pin")` glyph (`ICON_PATHS.pin`) instead of 📌, keeping their `aria-label`. `index.html`: the terminal close is an `.icon-btn`. `style.css`: `.icon-button` and its dead `.panel-icon` / `.is-collapsed` rules removed (nothing else used them); the terminal bar rule now sizes `.icon-btn` to 26px; `--accent-soft`'s `#f0f5ff` fallback removed. Colours: literals equal to a token replaced (`#ffffff`/`#fff` backgrounds → `--surface`, `#f0f0ed`, `#fbfbfa`, `#f7f7f5`, `#e7f1ee`); twelve new tokens for literals in three or more rules (`--on-accent`, `--ink-strong`, `--faint`, `--hover`, `--line-soft`, `--code-soft`, `--ok-line`, `--warn-soft`, `--warn-line`, `--warn-ink`, `--danger-soft`, `--danger-line`), listed in style guide §2. **Deviations:** `#eeeeea` (3 rules) folded into `--line-soft` (`#eeeeeb`), `#fbfbfb` into `--surface-soft` (`#fbfbfa`), both 1/255 apart; `#626568` (3 settings rules) became `--muted` (`#646b77`), a hair lighter and still above the contrast floor, rather than a near-duplicate token. Syntax colours were already grouped under their own comment and stay literals; one- and two-use literals stay (req 8 only asks for three or more). Distinct literals outside `:root`: about 85 → 67. Test: desktop-shell `static_assets_use_one_button_scale_and_drawn_icons` (no `btn-secondary`, `icon-button` or 📌 in the three assets; no `--accent-soft` fallback). Checked in the running app (`serves_the_ui_for_manual_inspection`, port 4911, its own data dir, stopped by PID): the terminal close renders as a quiet 26×26 `icon-btn`; the context panel's pin controls show the drawn pin (34×28, no text); `.btn-primary` text resolves to white; tokens resolve (`--warn-ink` = `#7a4b12`). Not exercised: the memory proposal card and the compaction boundary (no fixture shows them; the change there is class names only). Checks (local, 2026-11-19): `scripts/quality-gate.sh fmt node lint typos deny` ok (deny outside the agent sandbox for its advisory-db lock); `cargo clippy -p desktop-shell --all-targets --locked -- -D warnings` clean; `cargo nextest run -p desktop-shell --locked` 335 passed, 20 skipped (outside the sandbox for loopback binds; 57 s wall). Workspace clippy and tests left to CI. |
| 2 · One Stop (req 3) | Done 2026-11-19 | `app.js`: `composerStopTarget()` decides what the composer button stops in the current session — `currentTurn` (its indicator's action, whose stop is `stopCurrentTurn`, i.e. the abort), else `runningActionFor(currentSessionId).stop()` (default: `stopSessionWork` → `/api/session-work-cancel`), else a `workingSessionsById` row (`stopWorkFromComposer`: the same cancel, then a poll, with an in-flight guard so click + Escape post once), else Send. Click and Escape both go through `stopFromComposer()`. `syncComposerStop()` draws the button (is-stopping, `aria-label` "Stop", `title` "Stop (Esc)" or the reason) and is called from `syncRunningState`, `syncInvisibleWork` (poll and session switch) and `setComposerBusy` (now parameterless), so a resumed stream without `chatSubmitting`, a recovered command and working-sessions work all turn Send into Stop. It goes back to Send once per transition, so the question lock is not undone each tick; the lock in `syncComposerDecision` leaves a showing Stop's `disabled` alone. The answered-question and fetch-decision flows were already running actions with the cancel-route stop; they are now reached because the composer no longer only aborts `currentTurn`. `startTurnIndicator` builds no button; `onStop` still sets the action's stop, `false` marks it `unstoppable` with `unstoppableReason`; the row shows "Stopping" through a `runningListeners` entry, and returns `action` so `currentTurn.action` is set in `sendChatPrompt` and `askAgainFromExhausted`. Removed `#composer-running-stop-btn`, `.turn-indicator-stop` (CSS and the waiting-row cleanup). Steering unchanged (Enter → `sendChatPrompt` → `sendSteeringNote`). **Deviations:** `#composer-invisible-work`'s Stop is removed too (the task named only `#composer-running`'s; leaving it would keep two Stops for working-sessions work, and Task 3 replaces the box anyway). A sent turn whose stream has not begun shows a disabled Stop ("Starting — nothing to stop yet") rather than an enabled no-op. The card's own Cancel/Stop row (`createRunningRow`) stays, per req 3. Tests: new desktop-shell `app_js_has_one_stop_in_the_composer` (priority order, one Stop site, no direct `stopCurrentTurn();` call, the turn row builds no button, the removed ids gone everywhere, steering kept); `app_js_gives_every_running_action_one_state_and_a_stop`, `app_js_shows_work_this_window_never_streamed` and `app_js_status_line_waits_…` updated for the removed buttons. Checked in the running app (`serves_the_ui_for_manual_inspection`, port 4911, its own data dir, `DAMAIAN_MOCK_MODEL_RESPONSE`, stopped by PID; a console `fetch` wrapper delayed the stream requests by 5–6 s so each run could be stopped): live mocked turn — the only visible Stop was `#ask-btn`, the row had no button; click and, separately, Escape ended it "Stopped by you" with no cancel request (the `/api/ask-stream` abort) and the prompt restored. Answered question (pending-question fixture, `/api/answer-question-stream`, `chatSubmitting` true, no `currentTurn`): one Stop; click posted `/api/session-work-cancel` once, the row read "Stopping" and the button "Stopping…" disabled. Recovered command Approve (`/api/run-command`, `chatSubmitting` false): `#ask-btn` became Stop beside the card's own Stop row; click posted `/api/session-work-cancel` once (200). Injected working-sessions row: `cancellable: false` → disabled Stop titled "This work cannot be stopped; it ends on its own"; `cancellable: true` → enabled; click then Escape posted the cancel once (409 not working, swallowed). No JS errors (the fixture's `/api/commit-status` 500s predate this). Not exercised: the fetch decision (no fixture; same code path as the question) and the one-shot patch's unstoppable action (off by default). Checks (local, 2026-11-19): `scripts/quality-gate.sh fmt node lint typos deny` ok (lint after `npm run lint:web:fix` reflowed one line; the `.findings-panel` specificity warning predates this); `cargo clippy -p desktop-shell --all-targets --locked -- -D warnings` clean; `cargo nextest run -p desktop-shell --locked` 336 passed, 20 skipped (outside the sandbox for loopback binds; 16 s wall). Workspace clippy and tests left to CI. |
| 3 · One composer status slot (req 2, 5) | Done 2026-11-19 | `index.html`: `#composer-running`, `#composer-decision` and `#composer-invisible-work` replaced by `#composer-status` (`-label`, `-clock`, one optional `#composer-status-btn`; no Stop). `app.js`: one `syncComposerStatus()` renders the first entry of `composerStatusEntry()` — **Needs you** (`pendingDecisionApplies()`: pending, not sending, not in a worker view, nothing running; a question reads "Needs your answer", others the engine label; **Answer**/**Show** → `focusPendingDecision`; clock from the decision's `since`), **Working** (`runningActionFor(currentSessionId)`: its label, or "Stopping: …"; clock from `stepStartedAt`), **Busy elsewhere** (`busyElsewhereWork()`: a working-sessions row with no stream or running action of this window; "Busy elsewhere: {label}"; clock = `elapsedMs` + time since the poll, `workingSessionsPolledAt`). Text is clipped to 60 characters (`clipStatusText`); the full sentence is the slot's `title`. Needs you is skipped while the pending frame is in view, Working while any of the action's `homes` (the turn's status row from `startTurnIndicator`, a card's `createRunningRow`) is — an `IntersectionObserver` (`observeStatusHome` / `unobserveStatusHome`, seeded from geometry by `homeIsInView` so the slot does not flash). It calls `syncComposerStop()` and the question lock (`syncComposerQuestionLock()`, split out of `syncComposerDecision`, placeholder now "Answer the question", and independent of the card's visibility), and keeps the reload-on-hide (`busyElsewhereApplied`, reset by `loadSession`; it tracks whether Busy elsewhere *applies*, so a Needs-you entry shadowing it does not suppress the reload). `syncRunningState`, `pollWorkingSessions`, `setComposerBusy`, `loadSession` and the pending-decision setters call it; `syncComposerStop`'s return to Send calls the lock. Removed: `syncComposerRunning`, `syncComposerDecision`, `syncInvisibleWork`, `appendPendingPointer`, `.pending-decision-pointer`, `.composer-decision`/`-running`/`-invisible-work` CSS (new `.composer-status`, accent edge only for `data-kind="needs-you"`). The waiting row stays (D5). This also ends Task 2's noted quirk (the running line flashing after an answered question starts): Working now follows the row's visibility, not `chatSubmitting`. **Deviations:** the observer's root is the viewport, not `#chat-log`: below 1100px wide the page scrolls and `#chat-log` grows to its content, where a `#chat-log` root reports every home in view and the slot never shows; the viewport root is still clipped by `#chat-log`'s own scrolling in the wide layout. Every entry carries a clock (req 2 says "plus a clock"), so Needs you and Busy elsewhere tick on their own 1 s timer while shown. Not changed: in a brand-new session the first turn has no session id until its `session` event, so Working cannot show for that turn until then (the old line had the same limit). Tests: new desktop-shell `app_js_has_one_composer_status_slot` (slot markup with one button and no Stop, priority order, the observer and both homes, 60-character clip and `title`, the removed ids/functions/pointer gone from all three assets); `app_js_composer_names_the_pending_decision_and_yields_to_a_question` (spec 107) rewritten for the slot and the lock; `app_js_shows_work_this_window_never_streamed`, `app_js_gives_every_running_action_one_state_and_a_stop`, `app_js_has_one_stop_in_the_composer` and `app_js_places_every_pending_decision_kind_above_the_composer` updated. Checked in the running app (`serves_the_ui_for_manual_inspection`, port 4911, its own data dir, `DAMAIAN_MOCK_MODEL_RESPONSE`, stopped by PID; the pane was hidden, so a screenshot was taken to give the observer a frame before each read; a 2500px spacer at the top of the log made it scroll): pending-question fixture, card in view — slot hidden, prompt disabled with "Answer the question", no pointer; scrolled away — **Needs your answer · 18s · Answer** in both layouts (1024px page scroll, 1400px log scroll); **Answer** scrolled to the card, focused its answer field, and the slot hid. Live mocked turn (stream delayed 8 s by a console `fetch` wrapper) scrolled away — **Starting · 0s**, the only visible Stop `#ask-btn`; scrolled back with the turn still running — hidden. Injected working-sessions row for the open session (stubbed `/api/working-sessions`) — **Busy elsewhere: Running cargo nextest run --workspace --loc…** (60 characters) · 12s, full text in `title`, `#ask-btn` an enabled Stop; removing the row reloaded the session exactly once and the button went back to Send. No new console errors (the fixture's `/api/commit-status` 500s predate this). Not exercised: a card's running row as Working's home (checks fixture; same `homes` path as the turn row). Checks (local, 2026-11-19): `scripts/quality-gate.sh fmt node lint typos deny` ok (deny outside the sandbox; the `.findings-panel` specificity warning predates this); `cargo clippy -p desktop-shell --all-targets --locked -- -D warnings` clean; `cargo nextest run -p desktop-shell --locked` 337 passed, 20 skipped (outside the sandbox for loopback binds; 16 s wall). Workspace clippy and tests left to CI. Docs: `USER_GUIDE.md` (pending decisions) and `TROUBLESHOOTING.md` ("Waiting for your …") no longer quote the removed pointer and "Answer the question above", and say the line shows only while the card is out of view; the full description of the slot stays with Task 6. Observation for later (not this task): below 1100px the composer sits at the bottom of the scrolled page, so the slot itself is off screen while the user reads earlier messages. |
| 4 · A pending card drawn once; no empty rows (req 4, 6) | Done 2026-11-19 | `app.js`: `renderRecoveryPrompts` makes the transcript copy of every question the sweep reattached for the open session read-only with a new `makeQuestionCardReadOnly` (removes `.question-card-options`, `-free-text` and `-status`, sets `data-readonly`; an answered card is left as it is); the recovery card stays the answerable copy that `restorePendingDecision` frames (D7, its comment updated). The other reattached kinds have no transcript card on reload (the transcript draws only questions as cards), so nothing else needed de-duplicating. `style.css`: `.question-card[data-readonly="true"]` takes the settled card's background; `.message.assistant .message-body:empty { display: none; }`, so a starting turn is its status row alone until the first token fills the body (no JS change: every live turn already opens with `appendChatMessage("assistant", "")`). `markMessageRewindable` returns early when `fileCount` is 0 (reload and live). **Deviations:** (1) The live bubble has no user message id (neither the done payload nor the stream carries it), so the live path matches the checkpoint by the turn's task id (`assistantMessage.message.dataset.taskId`, from the done payload) — the checkpoint `take_turn_checkpoint` takes once per task with that turn's user message, so it is the same checkpoint. `checkpoint_list_json` (`json.rs`) now also returns `taskId` (null when absent); the reload path still matches `userMessageId`. (2) Not in the task text: overlapping `loadSession` calls for one session (`setRepository` plus another load, a poll's reload, a rewind) each appended the sweep's recovery cards after the same await, giving two answerable recovery cards (the "three copies" Task 3 saw). `loadSession` now bumps `sessionLoadGeneration` and a load that a later one has overtaken stops drawing after each await; `renderRecoveryPrompts` takes the generation and checks it after its own await. (3) A worker session's transcript question is not made read-only: it answers through the worker's socket (spec 104) and `restorePendingDecision` frames nothing for a worker, so changing it would change the route. A zero-file checkpoint is still listed in Settings › Checkpoints, where a conversation-only rewind to it remains possible; `USER_GUIDE.md` (Rewind) says so. Tests: desktop-shell `app_js_draws_a_reattached_question_answerable_once` (transcript copies only, read-only helper, recovery card framed first, sweep before restore, the load generation), `style_css_hides_an_empty_assistant_body`, `app_js_offers_rewind_only_for_the_turns_own_files` (guard before any DOM, task-id match, no newest-unrestored lookup, reload by `userMessageId`); `checkpoint_list_json_reports_what_the_list_view_shows` asserts `taskId`. Checked in the running app (`serves_the_ui_for_manual_inspection`, port 4911, its own data dir, `DAMAIAN_MOCK_MODEL_RESPONSE`, stopped by PID): pending-question fixture after a server start — exactly one answerable `.question-card`, inside the recovery card and the `.pending-decision` frame; the transcript copy shows "Exponential or fixed backoff?" with no controls; no pointer; the same after two overlapping `loadSession` calls and after a third load (before the generation guard the same fixture showed two answerable recovery cards). Live mocked turn with the stream delayed 5 s — during "Starting" the assistant body is empty and `display: none` (0 px), the row stands alone; finished, its checkpoint reports `fileCount` 0 and the user message has no Rewind row; no "0 files covered" anywhere. Fixture rewind session reloaded — "2 files covered · Rewind" on its user message; a further mocked turn there left that row and added none to the new message (the old lookup would have taken the new zero-file checkpoint). No new console errors (the fixture's `/api/commit-status` 500s predate this). Not exercised: a live turn that changes a file (the mock model proposes no patch); the match is the same `taskId` path checked by the zero-file turn. Checks (local, 2026-11-19): `scripts/quality-gate.sh fmt node lint typos deny` ok (outside the sandbox for deny); `cargo clippy -p desktop-shell --all-targets --locked -- -D warnings` clean; `cargo nextest run -p desktop-shell --locked` 340 passed, 20 skipped (outside the sandbox for loopback binds; 13 s wall). Workspace clippy and tests left to CI. |
| 5 · Session state in the sidebar (req 7) | Not started | |
| 6 · Documentation and close-out | Not started | |

## Task 1 · Classes and tokens (req 8)

No behaviour change.

- [x] Memory proposal: Remember `btn-sm btn-primary`, Dismiss `btn-sm
      btn-quiet`; *Compact now* `btn-sm`.
- [x] Merge `.icon-button` into `.icon-btn` (terminal close and any other
      user); delete the old rule.
- [x] Replace the 📌 control label with an `icon()` SVG and an `aria-label`.
- [x] Drop `--accent-soft`'s fallback; replace colour literals that equal an
      existing token; make a token of any literal used in three or more rules;
      group syntax-highlighting literals under one comment.
- [x] Desktop-shell test: `static/` contains no `btn-secondary` and no
      `icon-button` class.
- [x] Style guide §2 lists any new tokens; `ui-style-guide.html` still renders.

Files: `static/app.js`, `static/style.css`, `static/index.html`,
`docs/UI_STYLE_GUIDE.md`, `docs/ui-style-guide.html`, desktop-shell tests.
Checks: `scripts/quality-gate.sh fmt node lint typos deny`; `cargo nextest run
-p desktop-shell` for the new test; a visual pass in the running app (settings,
a card, the context panel, the terminal).

## Task 2 · One Stop (req 3)

- [x] One function decides what the composer button stops for the current
      session: this window's stream (abort), else its running action
      (`action.stop()`), else work from working-sessions (cancel route), else
      nothing (Send). Click and Escape both use it.
- [x] The question answer and fetch decision flows become stoppable through
      it (today a no-op).
- [x] `startTurnIndicator` no longer renders a Stop; `onStop` is kept as the
      stop path the composer uses. `#composer-running`'s Stop is removed.
- [x] Unstoppable work: the button shows a disabled Stop with the reason in
      its `title`.
- [x] Steering notes unchanged (Enter with text during a turn).
- [x] Desktop-shell `app_js_…` test: one Stop site; the turn row builds no
      button.

Files: `static/app.js`, `static/style.css`, desktop-shell tests.
Checks: web checks; `cargo nextest run -p desktop-shell`; in the running app:
a live mocked turn, an answered question resuming, and a resumed stream each
show one Stop that stops it (record the request each made).

## Task 3 · One composer status slot (req 2, 5)

- [x] `index.html`: replace the three boxes with `#composer-status` (label,
      clock, one optional button).
- [x] One `syncComposerStatus()` derives the entry by priority (needs you,
      working, busy elsewhere) and replaces `syncComposerRunning`,
      `syncComposerDecision`'s box and `syncInvisibleWork`'s box; the question
      lock and the reload-on-hide behaviour are kept.
- [x] An `IntersectionObserver` on the pending frame and on the running
      action's status row (rooted at `#chat-log`) hides the matching entry
      while it is in view. *(Landed with the viewport as root, clipped by
      `#chat-log`; see the progress row.)*
- [x] Remove `appendPendingPointer` and its CSS; the placeholder reads
      "Answer the question".
- [x] Entry text ≤ ~60 characters; the long explanations go in `title`.
- [x] Desktop-shell test updated (spec 107's composer test and any that name
      the removed ids).

Files: `static/index.html`, `static/app.js`, `static/style.css`, desktop-shell
tests. Checks: web checks; `cargo nextest run -p desktop-shell`; in the running
app: the question fixture with the card in and out of view; a live turn
scrolled away; an injected working-sessions row for the open session.

## Task 4 · A pending card drawn once; no empty rows (req 4, 6)

- [x] On reload, a transcript decision that the recovery sweep also reattached
      renders read-only; exactly one answerable copy is framed (context D7).
- [x] An assistant `.message-body` with no content is not drawn; the status
      row stands alone until the first token.
- [x] `markMessageRewindable` is skipped when `fileCount` is 0; the live path
      finds the checkpoint whose `userMessageId` is that turn's user message.
      *(Landed as the checkpoint of the turn's task id; see the progress
      row.)*
- [x] Desktop-shell tests for the three.

Files: `static/app.js`, `static/style.css`, desktop-shell tests.
Checks: web checks; `cargo nextest run -p desktop-shell`; in the running app:
the question fixture after a server restart (one answerable card); a live
mocked turn (no empty box, no "0 files covered").

## Task 5 · Session state in the sidebar (req 7)

- [ ] `/api/sessions` rows carry `state`: `working` (from `lanes::working()`),
      `needs_you` (latest task `WaitingForApproval`), `failed` (latest task
      `Failed`), else absent. Measure the listing on 100 sessions (context D6).
- [ ] `renderProjectSession` draws `createWorkerDot`-style dots for those three
      states in place of the time, with `role="img"`, `aria-label` and `title`.
- [ ] The collapsed project row names its "N need you".
- [ ] The poll refetches the project's sessions when the working set changes,
      and a local turn's end refetches them.
- [ ] Spec 110 Task 3's sidebar bullet points here (done in the spec change;
      check it still reads true).
- [ ] Rust test for the `state` field (working, needs_you, failed, absent);
      desktop-shell `app_js_…` test for the dots and labels.

Files: `crates/desktop-shell/src/json.rs`, `session_api.rs`, possibly
`workspace-engine/src/session.rs` (latest task status), `static/app.js`,
`static/style.css`, tests. Checks: scoped clippy for each crate touched;
`cargo nextest run -p desktop-shell`; scoped workspace-engine tests if it
changes; in the running app: the fixture's question session shows Needs you,
a failed session Failed, a live mocked turn Working, idle rows nothing.

## Task 6 · Documentation and close-out

- [ ] `UI_STYLE_GUIDE.md`: principles "every state has one home" and "one
      vocabulary of states"; §4 the composer status slot and the one Stop;
      session dots beside worker dots; §8 anti-patterns "the same state in
      several places" and "a Stop on every surface".
- [ ] `ui-style-guide.html`: specimens for the slot and a session row with
      each dot.
- [ ] Spec 107: a dated note on requirement 1's pointer and requirement 2's
      composer line (shown only while the card is out of view).
- [ ] `USER_GUIDE.md` / `TROUBLESHOOTING.md`: what the slot, the Stop and the
      sidebar dots mean; why a session shows Needs you after a restart.
- [ ] Close-out per AGENTS.md: Status in `proposal.md`, this header, the
      README row and *What to build next*; the CHANGELOG *Unreleased* line;
      every `Depends on:` naming #112.

Checks: `scripts/quality-gate.sh fmt node lint typos deny`; `npm run
specs:check`. Workspace clippy and tests left to CI.
