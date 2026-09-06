# Conversation Column Density — Task List

**Implements:** [`proposal.md`](proposal.md) · background in [`context.md`](context.md)
**Style reference:** [`../../UI_STYLE_GUIDE.md`](../../UI_STYLE_GUIDE.md) · rendered at `docs/ui-style-guide.html`
**Started:** 2026-09-04

## Progress

| Task | State | Commit | Notes |
|---|---|---|---|
| 1 · Shared disclosure and visually-hidden helpers | Done | — | Approval card still 104/162px, so the refactor is behaviour-neutral. `.visually-hidden` hardened with `clip-path` since it is now a general utility |
| 2 · Context files into the turn | Done | — | Chat log 459 → 557px. Strip gone from the DOM, grid down to four rows, zero-file turn appends nothing. File rows needed `min-height: 24px` — they computed to 20px |
| 3 · Thread header shows repo and session | Done | — | Driven off `setRepoState` and `syncSessionListActive`, which already fire on every project/session change. Empty, selected, unknown-session and cleared states all verified; long title ellipsises with actions intact |
| 4 · Hide role labels | Not started | — | |
| 5 · Composer auto-grow | Not started | — | |
| 6 · Specimen page and outcome | Not started | — | |

## Baseline

Measured 2026-09-04, 1280×800, two-message exchange, seven context files:

| | px |
|---|---|
| Thread header | 51 |
| Context strip | 99 |
| Composer | 191 |
| **Chat log** | **459** |

Target: chat log ≥ 589px (acceptance criterion 1).

## How to verify

Same method as spec 41 — no provider key needed. `app.js` is a plain script, so
its top-level functions are global.

```bash
cargo build -p desktop-shell --bin damaian-desktop-shell
./target/debug/damaian-desktop-shell --port 4899
```

Load `http://127.0.0.1:4899` at a 1280×800 viewport and build a synthetic
exchange:

```js
appendChatMessage("user", "Why does the upload stall on 503?");
const a = appendChatMessage("assistant", "The retry lives in `client.rs`.");
appendContextDisclosure(a.body, [
  "crates/net/src/client.rs", "crates/net/src/retry.rs", "crates/net/src/lib.rs",
  "crates/core/src/upload.rs", "crates/core/src/error.rs", "crates/net/Cargo.toml",
  "docs/specs/12_web_app_troubleshooting.md",
]);
```

**Kill the shell by PID, never by name** — a real Damaian app shares the binary
name and `pkill -f damaian-desktop-shell` would kill the user's app too.

Static assets are `include_str!`-embedded: rebuild and restart for every visual
change, or you are looking at stale UI.

---

## Task 1 · Shared disclosure and visually-hidden helpers

**Files:** `app.js`, `style.css`, `index.html`

Spec 41 named the disclosure after its only caller. Two callers now.

- [ ] Rename `.command-approval-disclosure` to `.disclosure` in `style.css`,
      keeping every declaration including the `min-height: 24px` hit-target
      floor and the `[aria-expanded="true"] .disclosure-caret` rotation. Update
      the selector on the caret rotation rule to match.
- [ ] Rename `.session-select-hidden` (`style.css:156`, used once at
      `index.html:46`) to `.visually-hidden`. Same clip-rect declarations,
      general name, two callers after Task 4.
- [ ] Add the builder to `app.js`, near the other DOM helpers:

```js
// One disclosure implementation for every caller. Returns the trigger; the
// caller places it and the panel. Collapsed on creation, and deliberately not
// remembered — see docs/UI_STYLE_GUIDE.md §7.
function createDisclosure(label, panel) {
  const trigger = document.createElement("button");
  trigger.type = "button";
  trigger.className = "disclosure";
  trigger.setAttribute("aria-expanded", "false");
  const caret = document.createElement("span");
  caret.className = "disclosure-caret";
  caret.setAttribute("aria-hidden", "true");
  trigger.append(caret, document.createTextNode(label));
  panel.hidden = true;
  trigger.addEventListener("click", () => {
    const open = trigger.getAttribute("aria-expanded") === "true";
    trigger.setAttribute("aria-expanded", open ? "false" : "true");
    panel.hidden = open;
  });
  return trigger;
}
```

- [ ] Rewrite the approval card's inline disclosure construction in
      `createCommandApprovalPreview` to call `createDisclosure("Why this command", details)`.
      Its behaviour must not change.
- [ ] Rebuild, restart, and confirm the approval card still measures 104px
      collapsed and 162px expanded, with the caret still rotating. Lint. Commit.

## Task 2 · Context files into the turn

**Files:** `app.js`, `style.css`, `index.html`

- [ ] Replace `renderContextFiles` (`app.js:3435`) with:

```js
// The files a turn read, folded into that turn rather than a docked strip.
// Reference information, so the paths are quiet links rather than buttons.
// Live-turn only: stored messages do not carry contextFiles, so this does not
// survive a session reload — see context.md §3.
function appendContextDisclosure(body, files) {
  if (!files.length) return;
  const panel = document.createElement("div");
  panel.className = "context-file-list";
  files.forEach((path) => {
    const link = document.createElement("button");
    link.type = "button";
    link.className = "context-file";
    link.textContent = path;
    link.title = `${path} — open in Visual Studio Code`;
    link.addEventListener("click", async () => {
      try {
        const payload = await api("/api/open-vscode-file", form({ repo: requireRepo(), path }));
        toast(`Opened ${payload.path}`);
      } catch (error) {
        toast(error.message);
      }
    });
    panel.append(link);
  });
  const label = files.length === 1 ? "Read 1 file" : `Read ${files.length} files`;
  body.append(createDisclosure(label, panel), panel);
}
```

- [ ] Update the three populating call sites to pass the turn's message body:
      `app.js:4327` and `app.js:4756` (both have `assistantMessage` in scope) and
      `app.js:4654` (has `assistantMessage`). Each becomes
      `appendContextDisclosure(assistantMessage.body, payload.contextFiles || [])`.
- [ ] Delete the two clearing calls at `app.js:581` and `app.js:4447`.
- [ ] Delete `#chat-context` from `index.html:118`, and `.chat-context-strip`
      plus the old `.context-list` rule from `style.css`. Change `.conversation`'s
      `grid-template-rows` from five tracks to four.
- [ ] Restyle `.context-file` as a quiet link rather than a button: 12px,
      `--muted`, no border or background, hover to `--ink`, and
      `direction: rtl; text-align: left` with `text-overflow: ellipsis` so a long
      path truncates from the left and keeps its filename visible. Give
      `.context-file-list` `display: grid; gap: 2px` and a top margin.
- [ ] Verify: seven files render one row collapsed; expanding lists all seven;
      the strip is gone from the DOM; a zero-file turn appends nothing; clicking
      a path still calls `/api/open-vscode-file`. Lint. Commit.

## Task 3 · Thread header shows repo and session

**Files:** `index.html`, `app.js`, `style.css`

- [ ] Replace the static caption at `index.html:70-73` with two elements
      carrying ids — a session title and a repository line — keeping the
      existing `.eyebrow` treatment for the smaller of the two.
- [ ] Add a `renderThreadHeader()` to `app.js` that reads the current repo and
      the active session's title, writes the folder basename with the full path
      as `title`, and falls back to "No repository selected" with an empty
      session line when there is no repo.
- [ ] Call it wherever the header's inputs change: alongside the existing
      `$("repo-state").textContent` write (`app.js:261`), after
      `loadSessions`, after a session is selected, and after a session is
      renamed (`app.js:3671`).
- [ ] Verify: switching project updates it, switching session updates it,
      renaming a session updates it, clearing the project shows the empty
      state, and a deep path does not widen the header or push the buttons off.
      Lint. Commit.

## Task 4 · Hide role labels

**Files:** `app.js`, `style.css`

- [ ] In `appendChatMessage` (`app.js:2840`), change the label's class from
      `message-role` to `visually-hidden`. Keep the element and its text.
- [ ] Delete `.message-role` and `.message.user .message-role` from `style.css`.
- [ ] Verify with the accessibility tree, not the screenshot: the message's
      accessible name still begins "Assistant" or "You", and the label
      contributes zero height. Lint. Commit.

## Task 5 · Composer auto-grow

**Files:** `index.html`, `app.js`, `style.css`

- [ ] Change `#chat-prompt` to `rows="2"` and stop the base
      `textarea { min-height: 78px }` from applying to it — scope the base rule
      or override on the id. Do not remove the base minimum; the settings
      textareas rely on it.
- [ ] Add to `app.js`:

```js
// Grows with content up to a cap, so a short prompt costs two rows and a long
// one cannot push the log off screen. Capped against the conversation column
// rather than the window, since the sidebar takes a fixed share.
function autoGrowPrompt() {
  const field = $("chat-prompt");
  const cap = Math.round(document.querySelector(".conversation").clientHeight * 0.4);
  field.style.height = "auto";
  field.style.height = `${Math.min(field.scrollHeight, cap)}px`;
  field.style.overflowY = field.scrollHeight > cap ? "auto" : "hidden";
}
```

- [ ] Call it on the field's `input` event, and after each of the three places
      that change the value without one: after send clears it (`app.js:4706`),
      after a session load, and on startup.
- [ ] Verify: two rows at rest; grows line by line; shrinks on delete; stops at
      the cap and scrolls past it; returns to two rows after send; the window
      resizing does not leave it stuck at a stale cap. Lint. Commit.

## Task 6 · Specimen page and outcome

**Files:** `docs/ui-style-guide.html`, `docs/UI_STYLE_GUIDE.md`, spec folder, `docs/specs/README.md`

- [ ] Add a context-disclosure specimen to `docs/ui-style-guide.html`, both
      states, and rename the disclosure specimen so it is not
      approval-specific. Confirm no class it uses is missing from the
      stylesheet, the same check spec 41 used.
- [ ] Update the style guide: the disclosure section names `.disclosure`, and
      the "per-turn information docked in chrome" anti-pattern gains the
      context strip as its worked example.
- [ ] Measure the chat log at 1280×800 with the same two-message,
      seven-file exchange. Record the number against the 459px baseline and the
      589px target in `proposal.md`'s status line, whether or not it is met.
- [ ] Set the spec status and the `docs/specs/README.md` row to Done. Commit.
