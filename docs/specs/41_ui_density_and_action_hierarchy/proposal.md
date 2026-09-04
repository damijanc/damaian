# Feature Spec: UI Density and Action Hierarchy

Status: Done. All six tasks complete and verified in the running app.
Collapsed command approval measures **104px**, down from a measured **161px**
baseline; expanded is 162px, parity with the old always-expanded card. Patch
actions measure 67px and 59px instead of roughly half the conversation column
each. Three defects the implementation surfaced, and the one design change made
along the way, are recorded in [`context.md`](context.md) §3 and
[`tasks.md`](tasks.md).
Order: 41 of 41
Also in this spec: [`context.md`](context.md) (motivation, current state, and
corrections found during implementation), [`tasks.md`](tasks.md) (execution
order and progress).
Reference: [`../../UI_STYLE_GUIDE.md`](../../UI_STYLE_GUIDE.md) — this spec introduces
the button scale and action-hierarchy rules recorded there, and is the first
consumer of them. Later UI specs refer to the guide rather than restating it.
Related spec sections: `ai_coding_assistant_specification.md` §7.1 (distinct UI
states), §7.4 (command approval), §7.7 (diff and patch engine).
Related implementation specs:
[`../10_persistent_command_approval.md`](../10_persistent_command_approval.md) (the
`Allow Always` action this spec demotes to an overflow menu),
[`../12_web_app_troubleshooting.md`](../12_web_app_troubleshooting.md) (the
session-scoped browser diagnostic grant, likewise demoted),
[`../13_docker_command_support.md`](../13_docker_command_support.md) (Docker approvals
render through the same card).

**Not a roadmap graduation.** Like #7, #8, #10 and #34, this spec comes from
usability feedback in real use rather than from a roadmap work package: the
approval and patch cards were reported as too bulky to work with comfortably.

## 1. Requirements

1. A button scale exists in `style.css` with a default step and `.btn-sm`,
   `.btn-primary`, `.btn-quiet`, `.btn-icon` modifiers, matching
   [`UI_STYLE_GUIDE.md`](../../UI_STYLE_GUIDE.md) §3.
2. The global `button` default becomes the medium step (13px / 600 /
   `7px 12px`).
3. No element inherits a height minimum it does not use. `min-height: 180px`
   is removed from base `pre` and applied only to `#config-output`.
4. A command under review is displayed in full: wrapped, uncapped, with no
   horizontal or vertical scrolling of the command text.
5. The command rationale renders behind a disclosure that is collapsed on every
   render and is not remembered between proposals.
6. Command approval offers one primary action (`Approve`), one quiet action
   (`Reject`), and an overflow menu containing every grant that outlives the
   current turn.
7. Patch preview actions size to their content and do not use `.inline-actions`.
8. Keyboard focus order remains linear through the card, the focus ring is
   preserved on every control, and focus does not land on the primary action by
   default.
9. A specimen page at `docs/ui-style-guide.html` renders every token, type
   step, button step and variant, and both approval-card states, by loading the
   shipping stylesheet rather than a copy of it.
10. `npm run lint:web` passes clean.

## 2. Non-goals

- **The context-file strip.** `#chat-context` (`index.html:118`) will be folded
  into the assistant turn that used it, as a collapsed "Read N files"
  disclosure. That is the opening item of the next UI spec, not this one.
- **Sidebar, thread header, composer, terminal panel and settings pages.**
  These inherit the new default step and will look slightly lighter, but their
  layout and hierarchy are a later spec.
- **A dark theme.** The app remains light-only.
- **Any change to approval policy.** What requires approval, what may be
  allowlisted, and where the grant is stored are settled by specs 10 and 34 and
  are untouched. This spec changes presentation only.

## 3. Design

### 3.1 Button scale

Rewrite the global `button` rule to the medium step and add the four modifiers
from the style guide. Existing call sites are unchanged except where §3.3 and
§3.4 apply, so the rest of the application simply renders one step lighter.

`.inline-actions` keeps its two-column grid. The fix for the patch preview is
to stop using it there, not to change it.

### 3.2 Scoping the height minimum to the element that wants it

**This change is visually inert.** It removes a trap, not pixels — see `context.md` §1.

Delete `min-height: 180px` from base `pre` (`style.css:25`) and add it to
`#config-output`, the Effective policy block, which is the one element that
wants a tall empty box and the only `pre` the base rule currently reaches.
`.message-body pre`'s `min-height: 0` override then has nothing left to undo
and is removed.

The three changes must land together. Removing the base minimum without adding
it to `#config-output` collapses the Effective policy block; removing the
`.message-body` override first would expose every message `pre` to the trap.

`.command-approval-details` and `.command-approval-output` keep a
`max-height` (reduced to 190px) with `overflow: auto`, and gain no minimum.

### 3.3 Command approval

`createCommandApprovalPreview` (`app.js:4067`) is restructured to:

1. **Header** — title plus a risk pill. The pill replaces the current uppercase
   muted span.
2. **Command** — 11px monospace, `white-space: pre-wrap`, `overflow-wrap:
   anywhere`, no `max-height`, no `overflow`. Requirement 4: the user reads
   the whole command without interacting. A pathological command grows the
   card; that is the correct trade on a consent surface. The rule is written
   as `code.command-approval-command`, because `.message-body code` is
   `(0,1,1)` and outranks a bare class.
3. **Details** — the rationale pane, `hidden` by default, capped at 190px with
   scroll. It sits directly under the command it explains, above the footer.
4. **Footer** — one row: the disclosure on the left, the actions on the right.
   - **Disclosure** — a quiet `<button>` labelled "Why this command" carrying
     `aria-expanded`, toggling the details pane's `hidden` property. Rendered
     collapsed unconditionally, never remembered between proposals.
   - **Actions** — `Approve` (`.btn-sm .btn-primary`), `Reject`
     (`.btn-sm .btn-quiet`), and a `.btn-icon` overflow trigger.

   Pairing them on one row rather than stacking them saves 34px on every card.
   Measured both ways during implementation: stacked was 138px and missed
   acceptance criterion 1, paired is 104px. The disclosure is secondary chrome
   most approvals never open, so a dedicated row was poor value, and the
   result reads as a conventional card footer.

The overflow menu holds `Always allow in this project` (present when
`proposal.allowAlways`) and `Allow for this session` (present when
`proposal.allowBrowserDiagnosticsForSession`). When neither grant is offered,
the trigger is not rendered.

The menu follows the **`.context-menu-popover` pattern**, not `.model-popover`.
`.model-popover` is `position: absolute` anchored to the composer, which would
detach from an approval card as the log scrolls. The project row menu
(`ensureProjectMenu`, `app.js:594-658`) already solves this exact problem: one
shared `position: fixed` popover appended to `document.body`, anchored to the
trigger's `getBoundingClientRect()`, re-anchored on each open. Approval cards
have the same constraint — they live inside the scrolling `#chat-log` and there
may be more than one — so the approval overflow reuses that pattern and its
existing `.context-menu-*` styling.

Dismissal is already global: `document` click and `Escape` handlers at
`app.js:706-708`. The approval menu registers alongside them rather than adding
new listeners.

`resolveCommandProposal` is unchanged in behaviour: it still disables all
controls on entry, still restores them through `restoreActions` on failure, and
still resumes the turn with the same parameters. Only the elements it disables
change. A blocked proposal keeps its disabled primary action.

### 3.4 Patch preview

In `createPatchPreview` (`app.js:3768`), the actions move onto the header row:
`Apply {n}` (`.btn-sm .btn-primary`) and `Reject` (`.btn-sm .btn-quiet`), where
`n` is the count of currently selected files and updates with selection. The
`.inline-actions` wrapper is dropped. The header also carries the aggregate
file and line counts.

The secret-scanner notice, per-file diff cards, hunk selection, and rollback
controls are unchanged.

### 3.5 Specimen page

`docs/ui-style-guide.html` is a static page committed alongside the written
guide, giving it a rendered counterpart. It links
`../crates/desktop-shell/static/style.css` by relative path and is opened
directly from a checkout — no server, no build step, nothing to install.

Loading the shipping stylesheet rather than a copy is the whole point: the
page cannot misrepresent what the application looks like, because it *is* what
the application looks like. It carries only enough page-local CSS to lay itself
out — a grid, headings, swatch boxes — and none that affects the specimens
themselves.

It renders:

- every `:root` colour token as a labelled swatch;
- every step of the type scale, with its intended use;
- every button step and variant as a **live** control;
- the action-hierarchy group, the disclosure row, both approval-card states,
  and the patch header.

**States are live, not simulated.** The page does not reproduce `:hover`,
`:focus-visible` or `[disabled]` with page-local classes, because doing so
would reintroduce exactly the drift the page exists to prevent. Each variant is
a real, interactive control the reader hovers and tabs through, with the
disabled example carrying a real `disabled` attribute. The trade is that not
every state is visible at a glance; tabbing the page is also how focus order
gets verified, so the cost is small.

Coverage is maintained by hand. When a later spec adds a component, adding it
here is part of that spec's work — the page guarantees the styling is current,
not that the inventory is complete.

No tooling change is needed: `biome.json` scopes `files.includes` to
`crates/desktop-shell/static/**/*.{js,css}` and `scripts/**/*.mjs`, so a page
under `docs/` is outside the lint surface and must not be added to it.

## 4. Acceptance criteria

1. A collapsed command approval card for a single-line command and a one-line
   rationale measures **no more than 120px** tall in the running app, down from
   a measured baseline of **161px**. **Met at 104px**, and it holds at a 740px
   conversation column. Reached by pairing the disclosure and actions on one
   row (§3.3); the stacked layout measured 138px and missed.
2. Expanding "Why this command" reveals the rationale; collapsing restores the
   original height. A second proposal in the same session renders collapsed.
3. A command long enough to exceed the card width is fully readable without
   scrolling in either axis and without truncation.
4. `Always allow in this project` and `Allow for this session` are reachable
   only through the overflow menu, and each still produces the behaviour
   specified in specs 10 and 12 respectively.
5. A blocked proposal renders with a disabled primary action and cannot be
   approved through the overflow menu.
6. Patch preview actions occupy their natural width at every window size from
   the 420px minimum conversation column upward, and the apply label reflects
   the current selection count.
7. Every control in both cards is at least 24×24 CSS px, shows a visible focus
   ring, and is reachable by keyboard in visual order.
8. `docs/ui-style-guide.html` opens from a checkout with no server and renders
   correctly. Editing a token in `style.css` changes the page on reload,
   demonstrating it is not a copy. Its buttons respond to real hover and
   keyboard focus, and its disabled example is genuinely disabled.
9. `npm run lint:web` passes clean and the app has been driven by hand through
   approve, reject, both overflow grants, and a blocked proposal.
