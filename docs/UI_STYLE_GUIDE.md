# Damaian Desktop UI Style Guide

Status: Living document. Introduced alongside
[`specs/41_ui_density_and_action_hierarchy/`](specs/41_ui_density_and_action_hierarchy/proposal.md).

This is the standing reference for the desktop shell's visual language. Specs
describe *what changes*; this document describes *what the result must look
like*, so successive UI specs do not each re-derive the same scale. When a spec
and this guide disagree, the spec wins for the surface it owns and this guide
should be updated in the same change.

**See it rendered:** [`ui-style-guide.html`](ui-style-guide.html) is the visual
counterpart to this document — open it in a browser from a checkout, no server
needed. It links the shipping `style.css` by relative path, so it shows the
real thing rather than a copy that can drift. Its buttons are live controls:
hover and tab through them to see hover, focus and disabled states. This
document holds the rules and the reasoning; that page holds the pixels. Change
one and change the other in the same commit.

Scope is the Tauri desktop shell only:
`crates/desktop-shell/static/{index.html,style.css,app.js}`. Those assets are
`include_str!`-embedded into the binary, so any change requires a rebuild and
restart to be visible — there is no live reload.

---

## 1. Principles

1. **Permanent chrome must earn its pixels.** Anything docked outside the
   scrolling conversation costs vertical space on every turn forever. Per-turn
   information belongs *in* the turn, not in the chrome.
2. **Visual weight tracks consequence.** The size and colour of a control
   should reflect what happens when it is clicked, not how the markup happened
   to be nested.
3. **Consent content is never truncated.** On any surface where the user is
   approving something, the thing being approved is shown in full.
4. **Containers size to content.** No element reserves space it is not using.
5. **One primary action per surface.** If everything is emphasised, nothing is.

---

## 2. Tokens

Defined on `:root` in `style.css`. Use the token, never the literal.

| Token | Value | Use |
|---|---|---|
| `--bg` | `#f7f7f5` | Window background |
| `--surface` | `#ffffff` | Cards, panels, composer |
| `--surface-soft` | `#fbfbfa` | Recessed areas within a surface |
| `--sidebar` | `#f0f0ed` | Sidebar background |
| `--ink` | `#1f2428` | Primary text |
| `--muted` | `#6c737f` | Secondary text, quiet controls |
| `--line` | `#deded8` | Dividers, low-emphasis borders |
| `--line-strong` | `#c8c8c0` | Control borders |
| `--accent` | `#176b5d` | Primary actions, focus |
| `--accent-soft` | `#e7f1ee` | Focus ring, accent backgrounds |
| `--ok` / `--warn` / `--danger` | `#26734d` / `#a15c18` / `#a33737` | State only, never decoration |
| `--code` | `#101820` | Code block background |

The app is light-only (`color-scheme: light`). Do not add
`prefers-color-scheme` branches piecemeal; a dark theme is a whole-app change.

### Type

Body is `14px/1.45` system sans. Monospace is
`ui-monospace, SFMono-Regular, Menlo, monospace`.

| Size | Use |
|---|---|
| 14px | Body copy, message text |
| 13px | Default control text, card titles |
| 12px | Small controls, chips, secondary copy, inline code |
| 11px | Commands under review, pills, metadata |
| 10px | Uppercase eyebrow labels only |

Never go below 10px, and only for uppercase labels with adequate letter-spacing.

---

## 3. Buttons

There is one base rule plus opt-in modifiers. Do not set ad-hoc padding or
font-size on a button; if a surface needs something the scale lacks, extend the
scale.

| Class | Spec | Use |
|---|---|---|
| *(base)* `button` | 13px / 600 / `7px 12px` | Default. Settings, dialogs, panels |
| `.btn-sm` | 12px / 600 / `5px 10px` | Actions inside chat cards, diff rows, chips |
| `.btn-primary` | Accent fill, white text | The single most likely action on a surface |
| `.btn-quiet` | Transparent border and background, `--muted` text | Dismiss, cancel, reject |
| `.btn-icon` | Transparent, `5px 8px`, glyph only | Overflow `⋯`, close, small toggles |

**Rules**

- At most one `.btn-primary` per surface.
- `.btn-primary` and `.btn-quiet` are hierarchy, not semantics. A destructive
  action is marked by `--danger` text or an explicit confirm step, not by being
  the loudest button.
- Every interactive control is at least 24×24 CSS px. `.btn-sm` at 12px text
  computes to roughly 29px tall — do not shrink its padding further.
- Border radius stays at the existing 8px across all steps. The scale changes
  size and weight, not shape.
- Focus is always visible: `box-shadow: 0 0 0 3px var(--accent-soft)` with
  `border-color: var(--accent)`. Never remove the focus ring to tidy a layout.

**Contrast floors** (measured against `--surface`)

- White on `--accent` — 6.4:1. Passes AA for all sizes.
- `--muted` on white — 4.8:1. Passes AA for normal text, with little headroom.
  `.btn-quiet` text must not be lightened past `--muted`.

---

## 4. Action hierarchy

Every group of actions resolves into three tiers:

- **Primary** — the expected action. One, filled.
- **Secondary** — the other reasonable answer, usually the negative one. Quiet.
- **Overflow** — behind `⋯`. Everything that is rare, destructive, or
  **persistent beyond the current interaction.**

The overflow rule is the important one. A choice that changes state after this
turn ends — writing an allowlist, granting a session-wide permission, deleting
a stored item — must not sit at the same visual weight as the one-shot action
next to it, because the cost of a mis-click is not symmetric.

There are two popover implementations and they are not interchangeable:

- **`.model-popover`** — `position: absolute`, anchored by CSS to a fixed
  ancestor. Correct for controls in the composer, which never scrolls.
- **`.context-menu-popover`** — a single shared `position: fixed` element on
  `document.body`, anchored to the trigger's `getBoundingClientRect()` on open.
  Correct for anything inside a scrolling region, and for repeated rows where
  per-row popovers could not hold stable state.

Anything in the conversation log or the project list uses the second. Dismissal
(document click, `Escape`) is already wired globally — register with it rather
than adding listeners. Do not introduce a third implementation.

`.inline-actions` is a two-column grid intended for the narrow settings
column. Do not reuse it in the conversation column, where it stretches buttons
to half the available width.

---

## 5. Approval surfaces

Command approval and patch preview are held to stricter rules than the rest of
the UI, because the user is granting consent rather than navigating.

- **The subject is shown in full.** A command under review wraps and is shown
  in its entirety — never horizontally scrolled, elided, or capped behind a
  scroll container. The user must be able to read everything they are
  approving without interacting first.
- **Rationale is progressive.** Explanatory prose sits behind a collapsed
  disclosure. It renders collapsed every time; disclosure state is not
  remembered between proposals.
- **Escalations live in the overflow.** "Always allow", session-wide grants,
  and anything else that outlives the turn.
- **Nothing is pre-selected toward approval.** Focus does not land on the
  primary action.

---

## 6. Density

- **No phantom minimums.** An element must not reserve height it is not using,
  and a minimum belongs on the one element that wants it, never on a shared
  base element. Base `pre` carried `min-height: 180px` for exactly one
  intended consumer (`#config-output`) and was kept survivable only by a
  counter-override on `.message-body pre` — two rules cancelling out, with any
  new `pre` outside `.message-body` silently inheriting the cost. If you find
  yourself writing an override whose only job is to undo a base rule, move the
  rule instead.
- **Cap with scroll, not with a floor.** Long content gets `max-height` plus
  `overflow: auto` — with the exception in §5.
- Standard spacing steps: 4, 6, 8, 10, 14, 18px. Prefer the smaller step.
- Conversation gutters use `clamp(18px, 5vw, 72px)`. Keep any new full-width
  band on the same clamp so edges line up.

---

## 7. Disclosure pattern

Used for rationale, file lists, and any detail that is occasionally wanted.

- Build it with `createDisclosure(label, panel)` in `app.js`. There is one
  implementation, styled by `.disclosure` and `.disclosure-caret`; do not write
  a second.
- Trigger is a quiet inline row: caret glyph plus a short label naming what is
  inside, with a count where one exists ("Read 7 files", not "Details").
- **Pair the trigger with the card's action row rather than giving it a row of
  its own** — disclosure left, actions right, as one footer. A dedicated row
  costs about 34px on every card for a control most readers never open. The
  expanded content goes above the footer, next to whatever it explains.
- Collapsed by default. Cost when collapsed is one text row.
- Expanded content is capped with `max-height` and scrolls, unless §5 applies.
- The trigger is a real `<button>` with `aria-expanded`, and the panel is
  toggled with the `hidden` property. **If the panel's own rule sets
  `display`, add `[hidden] { display: none }` for it** — an author `display`
  declaration beats the UA `[hidden]` rule, and the panel stays on screen
  while every scripted check reports it hidden.

---

## 8. Anti-patterns

Each of these existed in the shell and was removed. Do not reintroduce them.

| Anti-pattern | Why |
|---|---|
| One button style for everything | A settings "Load" and an approval "Reject" read as equally consequential |
| `min-height` on a shared base element | Needs a counter-override elsewhere to stay survivable, and the next element added inherits the cost silently |
| Trusting an estimate as a measurement | The card this guide was written for was claimed at ~330px from a mockup; it measured 161px. Measure in the running app, and say which |
| Reusing `.inline-actions` outside settings | Stretches actions to half the chat column |
| Per-turn information docked in chrome | Costs space on every turn and shows only the newest turn. The context strip was 99px of permanent band showing one turn's files; it now sits inside the turn that read them |
| Full paths as full-size buttons | Maximum visual weight for reference information. Reference lists are quiet links, truncated from the left so the filename survives |
| Horizontally scrolled command under review | User approves what they cannot see |
| Escalating and one-shot actions at equal weight | Mis-click cost is not symmetric |

---

## 9. Verification

There is no JS test suite for the shell. A UI change is verified by:

1. `npm run lint:web` (Biome) — must pass clean.
2. Open [`ui-style-guide.html`](ui-style-guide.html) and check the specimens
   still render correctly. If the change added a component, add it there in the
   same commit — the page keeps styling current automatically, but its
   inventory is maintained by hand.
3. Rebuild and restart the app; static assets are embedded, so an unrebuilt
   binary shows stale UI.
4. Drive the changed surface by hand, including keyboard focus order and the
   disclosure/overflow states.

Measure before/after heights when a change claims a density improvement, and
record the numbers in the spec.
