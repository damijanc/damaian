# Feature Spec: Conversation Column Density

Status: Not started
Order: 42 of 42
Also in this spec: [`context.md`](context.md) (motivation, current state, and
the reload limitation), [`tasks.md`](tasks.md) (execution order and progress).
Reference: [`../../UI_STYLE_GUIDE.md`](../../UI_STYLE_GUIDE.md) — this spec
consumes the button scale and disclosure pattern spec 41 established, and
promotes the disclosure to a shared class.
Related implementation specs:
[`../41_ui_density_and_action_hierarchy/proposal.md`](../41_ui_density_and_action_hierarchy/proposal.md)
(the scale and disclosure pattern this builds on; §2 of that spec names the
context strip as this spec's opening item),
[`../26_context_assembly.md`](../26_context_assembly.md) and
[`../27_context_inspector.md`](../27_context_inspector.md) (the eventual
first-class context view; this spec is presentation only and does not
anticipate it).

**Not a roadmap graduation.** Second of three UI specs from the same usability
review as #41: the conversation column was reported as cramped, with too much
permanent chrome around too little conversation.

## 1. Requirements

1. The docked context strip is removed. The files a turn read are shown inside
   that turn's assistant message, behind a collapsed disclosure labelled with
   the count.
2. The disclosure renders collapsed, and each file remains clickable to open in
   Visual Studio Code, as the strip's buttons were.
3. The thread header shows the active working folder and session name in place
   of the static caption. Both update when either changes.
4. Message role labels are removed from view but remain available to assistive
   technology.
5. The composer's textarea starts at two rows and grows with its content to a
   cap, shrinking again when the content shrinks.
6. The disclosure pattern is a shared class used by both the command approval
   card and the context list, not duplicated.
7. At 1280×800 with a two-message exchange and seven context files, the chat
   log gains **at least 130px** over the measured 459px baseline.
8. Every control added or changed is at least 24×24 CSS px, keeps a visible
   focus ring, and is reachable by keyboard in visual order.
9. `npm run lint:web` passes clean.

## 2. Non-goals

- **Persisting context files per message.** The disclosure is live-turn only
  and disappears on session reload — see [`context.md`](context.md) §3. Making
  it durable is a server-side storage change, and it belongs with
  [`../27_context_inspector.md`](../27_context_inspector.md), which will own the
  context view properly.
- **Changing what goes into context.** Assembly, budgets and ordering are
  specs 26 and 27. This spec changes only how the result is displayed.
- **Sidebar, terminal panel and settings pages.** Phase 3.
- **The pinned-context chips.** `#composer-context` already works, is already
  compact, and represents user intent rather than model behaviour. Untouched.
- **A dark theme.** The app remains light-only.

## 3. Design

### 3.1 Shared disclosure

Spec 41 introduced `.command-approval-disclosure` and `.disclosure-caret`. The
context list needs the same control, so the pattern is promoted rather than
copied: a generic `.disclosure` class carrying the layout, the 24px hit-target
floor and the caret rotation, with `.command-approval-disclosure` reduced to
whatever remains specific to that card, or removed if nothing does.

A small helper builds one, so neither caller repeats the `aria-expanded`
wiring:

```js
createDisclosure(label, panel)
```

It returns the trigger button, sets `aria-expanded="false"`, hides `panel` via
its `hidden` property, and toggles both on click. Callers own placement.

### 3.2 Context files inside the turn

`renderContextFiles` is replaced by `appendContextDisclosure(body, files)`,
called with the assistant message's body at the three sites that currently
populate the strip (`app.js:4327, 4654, 4756`). The two clearing calls
(`app.js:581, 4447`) are deleted with the strip.

The disclosure is labelled by count — "Read 7 files", "Read 1 file" — matching
the style guide's rule that a disclosure names what is inside. Expanding
reveals the paths as a plain list of quiet links, not buttons: this is
reference information and should not carry action weight. Each opens in Visual
Studio Code through the existing `/api/open-vscode-file` call, unchanged.

Paths are shown in full but ellipsised from the left when they overflow, so the
filename — the part that identifies the file — stays visible.

When a turn read no files, nothing is appended.

`#chat-context` is removed from `index.html`, its CSS deleted, and the
`.conversation` grid drops from five rows to four.

### 3.3 Thread header

The static `Workspace Agent` / `Conversation` pair is replaced by the active
working folder's name and the session title, updated wherever the existing
`repo-state` text and session list are updated. The full path is available as a
`title` attribute; the visible text is the folder's basename, so a deep path
does not crowd the row.

With no repository selected the header reads as it does elsewhere in the app —
"No repository selected" — rather than showing an empty band.

The status badge, VS Code button and terminal toggle keep their current
positions and behaviour.

### 3.4 Role labels

`appendChatMessage` keeps emitting the role element, with a `visually-hidden`
class rather than the visible `.message-role`. The existing `.session-select-hidden`
rule (`style.css:156`) is the same clip-rect technique and is renamed to a
general `.visually-hidden` so both callers share it.

Screen readers still announce "Assistant" or "You" before each message; the
bubble's alignment and background carry it visually, as they already did.

### 3.5 Composer auto-grow

`#chat-prompt` becomes `rows="2"`, and the base `textarea { min-height: 78px }`
is scoped off it. On `input`, the field resets `height` to `auto` and sets it
to `scrollHeight`, capped at 40% of the conversation column so a long prompt
cannot push the log off screen; past the cap it scrolls.

The height is recalculated after send, after a session load, and after the
field is cleared, since none of those fire `input`.

## 4. Acceptance criteria

1. At 1280×800 with a two-message exchange and seven context files, the chat
   log measures **at least 589px**, up from the measured 459px baseline. The
   final figure is measured in the running app and recorded in the status line.
2. The context strip is gone from the DOM, and a turn that read files shows a
   collapsed "Read N files" row inside its assistant message. Expanding lists
   the paths; clicking one opens it in Visual Studio Code.
3. A turn that read no files appends no disclosure.
4. The thread header shows the working folder's name and the session title, and
   both update when the project or session changes. With no repository
   selected it shows the no-repository state rather than an empty band.
5. Role labels are absent from the rendered text but present in the
   accessibility tree — verifiable by reading the message's accessible name.
6. The composer starts at two rows, grows as content is typed, shrinks when it
   is deleted, stops growing at the cap and scrolls beyond it, and returns to
   two rows after send.
7. One disclosure implementation serves both the approval card and the context
   list; the specimen page shows it once, under a name that is not
   approval-specific.
8. Every control added or changed is at least 24×24 CSS px, shows a focus ring,
   and is reachable by keyboard in visual order.
9. `npm run lint:web` passes clean, and `docs/ui-style-guide.html` is updated
   to match.
