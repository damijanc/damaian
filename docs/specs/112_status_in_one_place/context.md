# Spec 112 context

Background for [`proposal.md`](proposal.md): the evidence, what the code does
today, and the decisions taken with the owner on 2026-11-19.

## Evidence

Measured on 2026-11-19 against `main` (spec 109 Task 8 merged) with
`serves_the_ui_for_manual_inspection` on port 4911, its own data dir and
`DAMAIAN_MOCK_MODEL_RESPONSE`, at 1280×820, counts and heights read from the
DOM:

- **Pending-question session** (seeded, loaded after the server started): six
  signals for one decision — in-message pointer, waiting row "Waiting for your
  answer 13s", the transcript `.question-card` (144px), a `.recovery-card`
  "Still waiting for you" (256px) holding a second `.question-card`, the
  composer line "Damaian needs your answer [Answer]", and the placeholder
  "Answer the question above". Both cards are answerable. The composer area
  was 218px of 820 (repo bar 31, decision line 43, prompt 63, controls 27).
- **Live mocked turn**: an empty bordered assistant box, then "Starting 0s
  [Stop]" in the turn, and the composer's Send turned into Stop: two Stops.
  After it finished, the user message read "0 files covered [Rewind]".
- **Sidebar**: the session waiting on the question read "Pick a retry policy ·
  now", the same as idle rows.
- **Declined approval** (Command activity session): the request bubble stays,
  with nothing saying it was declined (out of scope here; receipts are a later
  proposal).
- Earlier problems from the October screenshot (raw command-output bubbles, a
  usage row after every bubble) are gone since specs 67/68; observation 17 (the
  search panel shown on load) did not reproduce.

The investigation and its mockups are local planning material, not linked from
committed docs (AGENTS.md).

## Survey: what exists today

Line numbers are `crates/desktop-shell/static/app.js` at the start of this
spec; they will drift.

**Composer boxes** (`index.html` `#composer-running`, `#composer-decision`,
`#composer-invisible-work`; all `.composer-decision`, `style.css` ~1469–1505).

- `syncComposerRunning` (~16709), called only from `syncRunningState`: shows
  `runningActionText` and a Stop calling `runningActionFor(session).stop()`
  when an action exists, `!chatSubmitting` and not a worker view.
- `syncComposerDecision` (~13883): shows `pendingDecision.decision.label` and
  Show/Answer (`focusPendingDecision`) when a decision is pending and no turn
  or action runs. For a question it locks `#chat-prompt` and `#ask-btn` and
  sets the placeholder "Answer the question above" (~13904), the only place
  that string is set.
- `syncInvisibleWork` (~15924), from the 2 s `pollWorkingSessions`: "Working:
  {label} · {s}s — not visible here, but it blocks your next message" and a
  Stop posting `/api/session-work-cancel`; reloads the session when it hides.
- Running and decision exclude each other; running and invisible-work exclude
  each other; decision and invisible-work can show together.

**Stops.**

- One model: `beginRunningAction` (~16637); `action.stop()` calls the given
  `stop`, else `stopSessionWork` → `/api/session-work-cancel`.
- Turn row Stop (`startTurnIndicator` ~6570, button ~6611): with
  `onStop = stopCurrentTurn` (main turn, ask-again) it aborts the stream;
  with `null` (nine resume flows) it uses the cancel route; `false`
  (`proposePatchFromChat`) means unstoppable.
- Composer Send→Stop (`setComposerBusy` ~16759; click ~17061, Escape ~17069):
  `stopCurrentTurn()` only, i.e. the abort controller.
- **Bug:** the question answer flow (~12739) and the fetch decision (~12929)
  set `chatSubmitting` and `setComposerBusy(true)` without `currentTurn`, so
  the composer Stop and Escape do nothing there.
- Resume streams that do not set `chatSubmitting` show the turn row's Stop and
  `#composer-running`'s Stop together.
- `createRunningRow` (~16731): a card's **Cancel**, calling `action.stop()`
  (check batch card; reattached command card).
- Server: `/api/session-work-cancel` (`session_api.rs` ~789) answers 200, 409
  not working or 409 not cancellable; it never resolves an approval.

**Pending decisions.**

- Live: `appendProposals` builds the card, `setPendingDecision` (~13776) wraps
  it in `section.pending-decision`, moves it to the end of `#chat-log` and adds
  `appendPendingPointer` (~13804, "{label} — the card is below.").
- Reload: `loadSession` runs `renderMessages` (a transcript `question` message
  renders its own live `.question-card`, ~7291–7328, unanswered unless the next
  message is its answer), then `renderRecoveryPrompts` (reattached approvals
  from the once-per-process `/api/recovery` sweep; for a question
  `createReattachedApprovalCard` ~8023 builds a recovery card with a second
  `createQuestionCard`), then `restorePendingDecision` (~13837), which prefers
  the recovery card and leaves the transcript card live where it was asked
  (comment ~13843). Nothing de-duplicates them: this is the double card.
- `restoreWaitingIndicator` (~6542) adds the waiting row under the message.

**Starting bubble.** `appendChatMessage("assistant", "")` (~6453) creates the
article with an empty `.message-body`; `startTurnIndicator` inserts the row
after the body (~6624). The first token calls `updateChatMessage` (~6740).

**Rewind row.** `markMessageRewindable` (~8302) shows "{n} file(s) covered ·
Rewind" for any checkpoint; `take_turn_checkpoint`
(`workspace-engine/src/chat/turn_record.rs`) makes one per turn, possibly with
no files. Reload matches by `userMessageId` (~7333); the live path (~16917)
takes the newest unrestored checkpoint of the session, which may belong to a
different turn.

**Sidebar.** `renderProjectSession` (~11826): title, `span.session-when`, ⋮.
`/api/sessions` → `session_fields_json` (`json.rs` ~638) has no state.
Working sessions come from `/api/working-sessions` (`lanes::working()`,
`session_api.rs` ~765), polled every 2 s but used only for the open session.
Whether a session waits on a decision is known only for the open session
(`/api/session` `tasks[].turnPresentation.pendingDecision`) or from the
recovery sweep; a paused turn holds no lane. Worker rows already show
`createWorkerDot` (~9228) from the engine's `WorkerDot`.

**Visual debt.** `btn-secondary` used once (~7667, memory card Dismiss) with
no CSS rule; `.icon-btn` and `.icon-button` (terminal close) both exist; the
📌 emoji labels the context panel's pin control (~3521); `var(--accent-soft,
#f0f5ff)` (~style.css 917) falls back to an off-theme blue; `style.css` has 14
colour tokens and about 98 distinct hex literals.

## Decisions

- **D1 — scope (owner, 2026-11-19).** The class/token cleanup, the status slot
  with one Stop, and the sidebar session state. The other investigation
  proposals (receipts, turn footer, toast tones, settings, removals) are left
  for later specs.
- **D2 — the composer button is the one Stop (owner, 2026-11-19).** It never
  moves (style guide §4, "the action anchors the right edge"), and it already
  becomes Stop for the main turn. The turn row keeps its phase and clocks and
  loses its button. A card's own Cancel stays.
- **D3 — sidebar state moves here from spec 110 Task 3 (owner, 2026-11-19).**
  Spec 110 keeps the routes (its requirement 5); this spec draws the dots.
  Spec 110's Task 3 bullet carries a dated note.
- **D4 — build order (owner, 2026-11-19).** Before spec 110 Task 3, never at the
  same time as any other spec that edits `app.js` or `style.css`.
- **D5 — the waiting row stays.** Spec 107 requirement 3 (the status line never
  goes blank while a turn is alive) still holds: the waiting row is the turn's
  status line, in the turn; the card is the decision, above the composer. The
  in-message pointer and the directional placeholder were the duplicates.
- **D6 — where the state comes from.** `working` from `lanes::working()` (the
  same source as the poll, so the dot and the slot agree); `needs_you` and
  `failed` from the session's latest task status. If computing the latest task
  status for every listed session means replaying each log, measure it on a
  project with 100 sessions and record the time in the task row; it must stay
  well inside the poll interval. The sidebar refetches the project's sessions
  when the working set the poll reports changes, and after any local turn ends.
- **D7 — which copy survives on reload.** The framed pending copy is the one
  `restorePendingDecision` already chooses (the recovery card when there is
  one, because it carries the reattached route). The transcript copy renders
  as read-only text. A question whose answer follows it is unchanged.
