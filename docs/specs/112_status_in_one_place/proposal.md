# Feature Spec: Status in One Place

Status: In progress (specified 2026-11-19; started 2026-11-19, Tasks 1–4 done; six tasks in [tasks.md](tasks.md))
Order: 112 of 112
Plan: none. Written from a UX investigation of the desktop shell on
2026-11-19, checked against the running app the same day. The evidence, the
code survey and the decisions are in [context.md](context.md).
Depends on: [#107](../107_visible_waiting_and_working/proposal.md) (pending
decisions above the composer, the status line) and
[#109](../109_fast_verification/proposal.md) Task 6 (one running-action model
with a Stop) — both built. Spec
[#110](../110_responsive_shell_during_work/proposal.md) supplies the
working-sessions route this spec reads; its Task 1 is built. **Nothing here is
unbuilt, so this spec is ready to start.** Build it before spec 110 Task 3:
both edit `app.js` and `style.css` (owner's decision, 2026-11-19).

## 1. Motivation

Specs 107 and 109 made sure the user can always see that Damaian is waiting
or working. They did it by adding signals, and nothing removed the older ones.
In the running app on 2026-11-19 (context §Evidence):

- **One pending question was announced six times**: a pointer in the
  assistant's message ("the card is below"), the waiting row, the question
  card, a second answerable copy of the same card inside a *Still waiting for
  you* recovery card, the composer line with **Answer**, and the composer
  placeholder "Answer the question above". The pointer says *below* and the
  placeholder says *above*; both are true, of different copies.
- **A running turn had two Stop buttons**: one on the turn's status row and
  the composer's Send turned into Stop. On some resumed streams a third sits in
  the composer status line. Answering a question or a fetch approval turns Send
  into a Stop that does nothing.
- **The composer can stack three status boxes** (running, decision, work this
  window never streamed), and their text runs to sentences: "Working: … — not
  visible here, but it blocks your next message".
- **The sidebar shows no state** for an ordinary session: a session waiting
  on the user reads "now", like every idle one. With spec 110 several sessions
  can work at once, so this is where state is missing most.
- Smaller clutter: an empty bordered assistant box while a turn starts;
  "0 files covered · Rewind" on a turn that changed nothing; a memory card
  whose Dismiss button uses a class with no CSS (`btn-secondary`); two
  icon-button classes; an emoji as a control label.

The owner's priority stands (UX first, 2026-11-10): "If a user can't use
Damaian, its features do not matter."

## 2. Requirements

1. **Every state has one home.** A pending decision's home is its card at the
   end of the conversation (spec 107). A running action's home is its status
   row in the turn. Nothing else repeats the state while its home is in view.
2. **One composer status slot.** The three composer boxes become one slot,
   `#composer-status`, showing the first entry that applies, or nothing:
   1. **Needs you** — a pending decision whose card is not in view. Its button
      (**Show**, or **Answer** for a question) scrolls to and focuses the card,
      as today.
   2. **Working** — a running action of this session whose status row is not in
      view.
   3. **Busy elsewhere** — the session works on something this window never
      streamed (spec 110 working-sessions poll).

   Each entry is one line of at most about 60 characters plus a clock.
   Anything longer goes in the entry's `title`. The slot never holds a Stop.
3. **One Stop.** While anything in the current session is running — a streamed
   turn, a resumed stream, a running action, or work another window started —
   the composer's Send button is the Stop, and Escape does the same. It stops
   that work through the path the work needs: the stream's abort for a turn
   this window streams, `action.stop()` for a running action, and
   `/api/session-work-cancel` for work this window does not own. The turn
   status row has no Stop. Work that cannot be stopped (`unstoppable`, or a
   working-sessions row with `cancellable: false`) leaves the button as a
   disabled Stop, labelled with the reason. Steering stays as spec 110 Task 1c
   built it: Enter with text in the prompt during a turn sends a note. A
   card's own **Cancel** row (check batch, reattached command) stays, because
   it is the card's action.
4. **A pending card is drawn once.** After a reload or restart, an open
   question in the transcript and its reattached recovery card are one
   decision: exactly one answerable copy exists, framed as the pending
   decision at the end of the conversation. The transcript keeps the question's
   text, read-only. The same holds for any decision kind the recovery sweep
   reattaches.
5. **No pointers that duplicate the slot.** The in-message pointer
   ("… — the card is below.") is removed; the slot is the pointer, and only when
   the card is out of view. While a question is pending the prompt stays locked
   (spec 107 D2), with the placeholder "Answer the question" (no direction).
6. **No empty or meaningless rows.** An assistant message shows no bordered
   body until it has content: the status row stands alone until the first
   token. A user message offers **Rewind** only when its turn's checkpoint
   covers at least one file, and the live row uses the checkpoint of that turn,
   not the newest unrestored one.
7. **Session state in the sidebar.** Every session row shows the worker dot
   (`.worker-dot`, style guide §4) for three states, and nothing otherwise:
   - **Working** (`running` look): the session is in the working-sessions list.
   - **Needs you** (`needs-you` look): its latest task waits on a decision
     (`WaitingForApproval`).
   - **Failed** (`failed` look): its latest task ended `Failed`.

   The dot replaces the relative time on that row, carries `role="img"`, an
   `aria-label` and a `title` naming the state, and updates within one poll
   (2 s) of the change. Idle, complete, stopped and cancelled sessions draw no
   dot. Worker rows keep their engine-computed dot. The collapsed project row
   names how many of its sessions need you ("2 need you").
8. **One visual system.** The memory proposal's Remember / Dismiss become
   `btn-sm btn-primary` / `btn-sm btn-quiet`; *Compact now* becomes `btn-sm`;
   `.icon-button` merges into `.icon-btn`; the 📌 control uses an `icon()`
   SVG; `--accent-soft`'s off-theme fallback goes; colour literals that repeat
   an existing token's value are replaced by the token, and a literal used in
   three or more rules becomes a new token. Syntax-highlighting colours may
   stay as literals, grouped under one comment.

## 3. Non-goals

- Receipts for decided cards, a turn footer, toast tones, a copy pass across
  every string, the settings reorganisation and the ⌘K palette (proposals C–H
  of the investigation). They are candidates for later specs.
- Any change to approval policy, to what needs approval, or to which routes
  resolve a decision. Requirement 4 changes which copy is answerable, not how
  an answer is delivered.
- A dark theme.
- Notifications outside the window (dock badge, system notification).

## 4. Acceptance criteria

- In the pending-question fixture session after a server restart, the DOM
  holds exactly one answerable `.question-card`, framed as the pending
  decision; the transcript keeps the question text; no `.pending-decision-
  pointer` exists; the slot is hidden while the card is in view and shows
  **Needs your answer · Answer** once the log is scrolled so the card is out of
  view.
- During a live mocked turn exactly one Stop control is visible, the composer
  button; clicking it stops the turn. The same holds while an answered question
  resumes and during a resumed stream that does not set `chatSubmitting`, and
  each of those stops through its own path (asserted by the request made).
- With working-sessions reporting the current session busy and this window not
  streaming, the slot shows **Busy elsewhere**, Send is a Stop, and clicking it
  posts `/api/session-work-cancel` once.
- `#composer-running` and `#composer-invisible-work` no longer exist; the
  three old sync functions are replaced by one that derives the slot.
- A finished mocked turn that changed no files shows no Rewind row; a turn that
  changed a file shows "1 file covered · Rewind" on its own user message.
- No assistant message with an empty body is visible during the "Starting"
  phase.
- `/api/sessions` rows carry `state` (`working`, `needs_you`, `failed`, or
  absent). With one session waiting on a question and another failed, the
  sidebar shows a `needs-you` and a `failed` dot on those rows, a `running` dot
  while a third works, and no dot on idle rows; each dot has an `aria-label`.
- `btn-secondary` and `.icon-button` occur nowhere in `static/`; no emoji is a
  control label; `npm run lint:web` and `node --check` pass.
- `docs/UI_STYLE_GUIDE.md` and `docs/ui-style-guide.html` describe the slot,
  the one Stop, the session dots and the new principles; `USER_GUIDE.md`
  describes what the user now sees.
