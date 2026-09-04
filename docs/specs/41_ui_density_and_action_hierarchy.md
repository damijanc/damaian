# Feature Spec: UI Density and Action Hierarchy

Status: Not started
Order: 41 of 41
Reference: [`../UI_STYLE_GUIDE.md`](../UI_STYLE_GUIDE.md) — this spec introduces
the button scale and action-hierarchy rules recorded there, and is the first
consumer of them. Later UI specs refer to the guide rather than restating it.
Related spec sections: `ai_coding_assistant_specification.md` §7.1 (distinct UI
states), §7.4 (command approval), §7.7 (diff and patch engine).
Related implementation specs:
[`10_persistent_command_approval.md`](10_persistent_command_approval.md) (the
`Allow Always` action this spec demotes to an overflow menu),
[`12_web_app_troubleshooting.md`](12_web_app_troubleshooting.md) (the
session-scoped browser diagnostic grant, likewise demoted),
[`13_docker_command_support.md`](13_docker_command_support.md) (Docker approvals
render through the same card).

**Not a roadmap graduation.** Like #7, #8, #10 and #34, this spec comes from
usability feedback in real use rather than from a roadmap work package: the
approval and patch cards were reported as too bulky to work with comfortably.

## 1. Motivation

The desktop shell has exactly one button style. Every `<button>` in the
application resolves to the same rule at `crates/desktop-shell/static/style.css:184`
— 14px text, weight 650, `9px 11px` padding. That single rule covers the 39
buttons in `index.html` and the 42 more that `app.js` builds at runtime. A
settings `Load`, a patch
`Reject Selected`, and a command `Allow Always` are visually identical.

Two consequences:

- **Nothing communicates consequence.** `Allow Always` writes a persistent
  entry to the user's command allowlist for that repository; `Approve Run`
  executes once. They are the same size, weight, and colour, sitting adjacent.
  Spec 10 added `Allow Always` specifically to reduce approval fatigue, but a
  control that reduces fatigue while being indistinguishable from the one-shot
  control next to it converts fatigue into accidental persistent grants.
- **Everything is bulky.** With no smaller step available, in-conversation
  cards use full-size controls for what is contextual, secondary work.

The bulk is not only buttons. Base `pre` carries `min-height: 180px`
(`style.css:25`). `.message-body pre` overrides it to `0`; the approval panes
never do, so **every command approval reserves 180px for its rationale even
when the rationale is one line.** That single declaration is the largest
contributor to the card's height.

## 2. Current state

| Element | Location | Problem |
|---|---|---|
| Global `button` | `style.css:184` | Single style for all 40+ buttons; no scale |
| Base `pre` | `style.css:25` | `min-height: 180px` inherited by both approval panes |
| `.command-approval-details` | `style.css:2134` | Sets `max-height: 240px`, never resets the inherited minimum |
| `.command-approval-command` | `style.css:2123` | `white-space: pre` + `overflow-x: auto` — a long command is one line behind a horizontal scrollbar |
| `.command-approval-actions` | `style.css:2148` | Four equal-weight buttons in a wrapping flex row |
| `.inline-actions` | `style.css:479` | `grid-template-columns: 1fr 1fr`; correct in the settings column, but `createPatchPreview` reuses it in the conversation column where each button stretches to half the chat width |
| `createCommandApprovalPreview` | `app.js:4067` | Builds the four-button row, including both escalating actions |
| `createPatchPreview` | `app.js:3768` | Builds `Apply Selected` / `Reject Selected` into `.inline-actions` |

The diff cards themselves are already dense — 11px pills, 12px monospace diff
lines, 1px line padding (`style.css:2197-2299`) — and are not changed here
beyond the patch-level action row.

## 3. Requirements

1. A button scale exists in `style.css` with a default step and `.btn-sm`,
   `.btn-primary`, `.btn-quiet`, `.btn-icon` modifiers, matching
   [`UI_STYLE_GUIDE.md`](../UI_STYLE_GUIDE.md) §3.
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
9. `npm run lint:web` passes clean.

## 4. Non-goals

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

## 5. Design

### 5.1 Button scale

Rewrite the global `button` rule to the medium step and add the four modifiers
from the style guide. Existing call sites are unchanged except where §5.3 and
§5.4 apply, so the rest of the application simply renders one step lighter.

`.inline-actions` keeps its two-column grid. The fix for the patch preview is
to stop using it there, not to change it.

### 5.2 Removing the inherited minimum

Delete `min-height: 180px` from base `pre` (`style.css:25`) and add it to
`#config-output`, the Effective policy block, which is the one place that wants
a tall empty box. `.message-body pre`'s existing `min-height: 0` override
becomes redundant and is removed.

`.command-approval-details` and `.command-approval-output` keep a
`max-height` (reduced to 190px) with `overflow: auto`, and gain no minimum.

### 5.3 Command approval

`createCommandApprovalPreview` (`app.js:4067`) is restructured to:

1. **Header** — title plus a risk pill. The pill replaces the current uppercase
   muted span.
2. **Command** — 11px monospace, `white-space: pre-wrap`, `word-break:
   break-all`, no `max-height`, no `overflow`. Requirement 4: the user reads
   the whole command without interacting. A pathological command grows the
   card; that is the correct trade on a consent surface.
3. **Disclosure** — a quiet `<button>` labelled "Why this command" carrying
   `aria-expanded`, toggling the details pane's `hidden` property. Rendered
   collapsed unconditionally.
4. **Actions** — `Approve` (`.btn-sm .btn-primary`), `Reject`
   (`.btn-sm .btn-quiet`), and a `.btn-icon` overflow trigger.

The overflow menu holds `Always allow in this project` (present when
`proposal.allowAlways`) and `Allow for this session` (present when
`proposal.allowBrowserDiagnosticsForSession`). It reuses the existing
`.model-popover` markup and dismissal behaviour rather than adding a second
popover implementation. When neither grant is offered, the trigger is not
rendered.

`resolveCommandProposal` is unchanged in behaviour: it still disables all
controls on entry, still restores them through `restoreActions` on failure, and
still resumes the turn with the same parameters. Only the elements it disables
change. A blocked proposal keeps its disabled primary action.

### 5.4 Patch preview

In `createPatchPreview` (`app.js:3768`), the actions move onto the header row:
`Apply {n}` (`.btn-sm .btn-primary`) and `Reject` (`.btn-sm .btn-quiet`), where
`n` is the count of currently selected files and updates with selection. The
`.inline-actions` wrapper is dropped. The header also carries the aggregate
file and line counts.

The secret-scanner notice, per-file diff cards, hunk selection, and rollback
controls are unchanged.

## 6. Acceptance criteria

1. A collapsed command approval card for a single-line command and a one-line
   rationale measures **no more than 120px** tall in the running app, down from
   the current ~330px. The figure is measured in the running app, not
   estimated, and recorded in this spec's status line on completion.
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
8. `npm run lint:web` passes clean and the app has been driven by hand through
   approve, reject, both overflow grants, and a blocked proposal.
