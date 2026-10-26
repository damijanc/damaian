# Feature Spec: Conversation Activity and Plan Hierarchy

Status: Done 2026-10-26 (started and finished the same day; verified in the
running app against a seeded session, see §7). A real investigation session made
the conversation hard to follow: every action was a full-size bubble, the plan
panel was always expanded under the last message, and the findings repair action
sent a prompt immediately.
Order: 67 of 67
Reference: [`../../UI_STYLE_GUIDE.md`](../../UI_STYLE_GUIDE.md) — this spec uses
the disclosure pattern §7 established and adds one component to its specimen
page.
Depends on: [#41](../41_ui_density_and_action_hierarchy/proposal.md) (the
disclosure pattern and the style guide), [#42](../42_conversation_column_density/proposal.md)
(the conversation column this changes), [#66](../66_plan_left_open_at_turn_end/proposal.md)
(the plan panel whose steps this collapses) — all built. Everything else named
below is a cross-reference, not a prerequisite.

## 1. Motivation

A three-minute reporting turn (`session_18e220b0a2aaa500_1236_16`,
2026-10-26) wrote 100 messages for 72 actions. Every action was an assistant
summary followed by a full `tool` result, and both rendered as ordinary bubbles.
The actual report sat above the bottom, while the bottom was a meta-summary plus
the five-step plan panel, fully expanded. The user could not tell intermediate
work from the answer, and the plan took most of the remaining space.

The same session showed the second problem: the failing `node …` command became
an `error` finding, the repo bar offered **Fix 1 error**, and clicking it sent a
full agentic repair prompt with no preview. The prompt asked the agent to change
code, when the real problem was the app's `PATH`.

This is presentation only. It changes no model context, stored message, finding,
approval or command policy.

## 2. Current State

- `renderMessages` (`app.js`) maps every session message to
  `appendChatMessage`, so an action summary and its result are two bubbles with
  the same visual weight as an answer.
- `renderPlanPanel` (`app.js`) appends the header and every step; the steps are
  visible whenever the panel is.
- `repairFindings` (`app.js`) fetches `/api/findings-repair` and calls
  `sendChatPrompt` immediately. The route already returns the rendered prompt,
  so a preview needs no server change.
- `createDisclosure` is the one disclosure implementation and starts collapsed
  by design (`UI_STYLE_GUIDE.md` §7).
- The session log already has the structure this spec needs: an action summary
  is the assistant message immediately before its `tool` result. No new
  `ChatMessage.kind` value is required, and old sessions group identically.

## 3. Requirements

1. Consecutive action/result pairs in one task render as a single collapsed
   **Activity** disclosure, not as individual bubbles.
2. The trigger names what is inside and how much: `Activity (9 steps)`.
3. Each expanded row shows the assistant action summary and its tool result.
   The expanded content is capped and scrolls; the stored text is unchanged.
4. The last assistant message of a task is never grouped, so the answer, the
   plan panel, usage and stop marks stay on their existing message.
5. Assistant messages containing a `worker-prompt` block are never grouped, so
   the parent session's *Started* links keep their anchor.
6. The plan panel's step list is behind a collapsed disclosure; the summary
   header (`5 steps complete`, `Turn ended with 1 step not marked done`) stays
   visible.
7. **Fix** and **Fix selected** show the exact repair prompt in a confirmation
   before anything is sent. Cancel sends nothing and leaves the selection.
8. If the findings being repaired are generic `command` findings with no
   location, the confirmation says this is likely an environment or `PATH`
   failure, not a code defect.
9. Old sessions render with the same grouping; no session log is rewritten.
10. `npm run lint:web` passes clean, the style guide gains an Activity
    specimen, and the changed surfaces are checked in the running app.

## 4. Non-goals

- **Persisting or showing model reasoning.** That needs a storage and redaction
  decision; this spec only separates the work that is already stored from the
  answer.
- **Classifying environment findings in the engine.** The confirmation warns;
  a distinct `environment` source or a `repairable` flag is a separate change.
- **Moving the plan into permanent chrome.** `UI_STYLE_GUIDE.md` §8 rejects
  per-turn information docked in chrome; the plan stays in the turn.
- **Changing what the model sees.** Grouping is a rendering decision.
- **A JavaScript test harness.** The shell has none; verification is lint, the
  style-guide specimen, and a live walk.

## 5. Design

### 5.1 Activity grouping

`renderMessages` already runs once per session load with the full ordered
message list. Before drawing, it builds groups:

- Start at an assistant message that is not the task's last assistant, is not a
  `plan_reminder`, has no `worker-prompt` block, and whose immediately following
  message is a `tool` message of the same task.
- Consume pairs while each pair is an assistant summary followed by its task's
  tool result. Stop at the last assistant, a user/question/system message, or an
  unpaired assistant.
- Each group is drawn at the first pair's position; the consumed messages are
  skipped.

`renderActivityGroup` builds one `.message.activity` with a visually hidden
label, one `createDisclosure("Activity (N steps)", detail)`, and a
`.activity-detail` containing one `.activity-step` per pair. The action summary
uses the client Markdown renderer and `wireFileReferences`; the result is a
`<pre>` with the exact stored text. The detail is capped (`max-height`) and
scrolls, per the disclosure rule.

The structural rule is deliberate: it needs no new session field, works on every
log already written, and cannot hide a message that is not mechanically an
action/result pair. A future engine-side `kind` would be a cleaner input but
would not change the rendering boundary this spec defines.

### 5.2 Plan disclosure

`renderPlanPanel` keeps the header, the single-in-progress warning and the
summary where they are, and wraps the step list in
`createDisclosure("Steps (5)", list)`. The trigger is one quiet row; the panel
collapses to its header and that row. `.plan-step-list` sets `display: grid`, so
a `[hidden] { display: none }` rule is required or the list stays visible when
the disclosure is closed (`UI_STYLE_GUIDE.md` §7's explicit warning).

### 5.3 Repair confirmation

`showAppDialog` gains an optional `detail` string. When present it renders a
scrollable `<pre>` above the actions and widens the dialog. `repairFindings`
calls it with the prompt from `/api/findings-repair`:

- **Cancel** leaves the selection and the findings panel untouched.
- **Send** clears the selection, closes the panel and runs the existing
  `sendChatPrompt({ agentic: true })` path.
- The message above the prompt names an environment failure when every kept
  finding is a `command` finding without a range.

The server still builds and excludes findings; the UI only previews the result.

### 5.4 Styling

New classes in `style.css`: `.message.activity`, `.activity-body`,
`.activity-detail` (capped, scrolls), `.activity-step`, `.activity-step-title`,
`.activity-step-result`, and `.app-dialog-preview` / `.app-dialog-detail`. The
plan needs only `.plan-step-list[hidden]`. The Activity specimen is added to
`docs/ui-style-guide.html`.

## 6. Acceptance Criteria

- An action/result run renders as one collapsed Activity, and the final answer
  renders as its own bubble.
- The Activity trigger reads `Activity (N steps)` and its expanded rows show the
  action and result, with the detail scrolling at its cap.
- The last assistant of a task is never inside an Activity, so the plan, usage
  and stop marks still render on it.
- A `worker-prompt` assistant message is not grouped and its Started links remain.
- A plan panel shows its summary and a collapsed `Steps (N)` row; expanding shows
  the steps and evidence.
- Fix and Fix selected show the prompt before sending; Cancel sends nothing.
- A `command` finding with no range produces the environment/PATH warning.
- An old session with no new fields groups the same way.
- `npm run lint:web` is clean and the style-guide specimen renders.
- The running app shows the collapsed plan and activity and the repair preview.

## 7. Implementation Notes

### What landed

- `app.js` groups consecutive assistant-summary/tool-result pairs into one
  `.message.activity` with a collapsed `Activity (N steps)` disclosure. The
  turn's last assistant is excluded, as are messages containing a
  `worker-prompt` block. Old sessions group identically because the rule reads
  the existing message order.
- `renderPlanPanel` keeps its header, warning and summary and puts the step
  list behind `Steps (N)`.
- `showAppDialog` accepts an optional `detail`; `repairFindings` uses it to show
  the exact `/api/findings-repair` prompt before sending and warns when every
  kept finding is a `command` finding with no range.
- `style.css` adds the activity and prompt-preview classes and the
  `.plan-step-list[hidden]` guard the disclosure needs.
- `docs/ui-style-guide.html` gains the Activity, collapsed-plan and
  repair-preview specimens.
- `serves_the_ui_for_manual_inspection` seeds a finished turn with a plan and
  three action/result pairs, so both new surfaces are inspectable by hand.

### Discoveries

- **The pair rule is already in the log.** An assistant action summary is
  always immediately followed by its task's `tool` result, and an answer is
  not. A new `ChatMessage.kind` would be a cleaner input but would not change
  the rendering boundary, and it would leave every existing session ungrouped.
- **`createDisclosure` is collapsed by design.** `UI_STYLE_GUIDE.md` §7 says
  collapsed by default and never remembered, so the plan's steps are collapsed
  while a turn runs as well as after it ends. The summary header is the live
  signal; the earlier idea of opening the disclosure while `active` was
  dropped to keep one disclosure rule.
- **The result is text, not Markdown.** Tool output is a diff, a test log or a
  command result. It is rendered with `textContent` in a `<pre>` so the bytes
  the assistant saw are the bytes shown; the action title keeps the client
  Markdown renderer for its inline code and file references.
- **The environment warning is presentation only.** The finding remains
  `command` / `error`; the server's prompt is unchanged. A later engine change
  can add a distinct source, but the preview already stops an immediate send.

### Verified in the running app

`serves_the_ui_for_manual_inspection` was extended to seed a finished turn with
three action/result pairs and a three-step plan (step 1 confirmed, step 2
blocked, step 3 pending), then run on port 4899 with its known token and a
throwaway data directory. The browser was driven through its console:

- The seeded session rendered **one** `Activity (3 steps)`; expanding it showed
  three rows, the first result byte-for-byte `fn upload() {\n    todo!()\n}`,
  and no `.message-body` inside the Activity (the answer stayed its own card).
- The plan panel showed `Turn ended with 1 step not marked done, 1 step blocked,
  1 finished.` with a collapsed `Steps (3)`. Expanding it showed three steps and
  the one evidence disclosure. The step list's `hidden` property was true while
  collapsed and false after the click.
- On the seeded findings session, **Fix 2 errors** opened `Fix 2 findings?`
  with the generic review message and the prompt beginning `Fix the 2 findings
  below…`. Selecting only the `npm test: 1 failing` command finding and pressing
  **Fix selected** opened `Fix 1 finding?` with the environment/PATH warning and
  the prompt beginning `Fix the 1 finding below…`. **Cancel** hid the dialog,
  left the user-message count unchanged and kept the selection.
- The style-guide page rendered two Activity specimens (one collapsed, one
  expanded), the collapsed `Steps (3)` plan and the prompt-preview dialog, with
  the Activity detail capped at its 420px CSS height.

The manual harness and the style-guide HTTP server were stopped by PID; the
harness's `cargo test` then exits 101 because its serving test was killed, not
because an assertion failed.

**Checks (2026-10-26, local):** `cargo fmt --all -- --check`, `node --check
crates/desktop-shell/static/app.js`, `npm run lint:web` (9 files, clean),
`typos`, `cargo deny check` and the rest of `scripts/quality-gate.sh fmt node
lint typos deny` all passed. `cargo clippy -p desktop-shell --all-targets
--locked -- -D warnings` is clean. `cargo nextest run -p desktop-shell --locked`
ran **205 passed, 17 skipped**. `npm run specs:check` reports **67 specs** and
agreeing dependency lines. The workspace clippy and test suite are left to CI.

