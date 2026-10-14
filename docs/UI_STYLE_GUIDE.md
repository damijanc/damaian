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

Because it loads the app's stylesheet, it also inherits app-shell layout rules
that suit a window and not a document — `body { overflow: hidden }` being the
one that bites. Its page-local block undoes those. Anything you add there must
still leave the specimens themselves untouched.

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

| Size | Weight | Use |
|---|---|---|
| 18px | 750 | Page title — settings pages, one per page |
| 15px | 750 | Section heading within a page |
| 14px | — | Body copy, message text |
| 13px | 600 | Default control text, card titles |
| 12px | — | Small controls, chips, secondary copy, inline code |
| 11px | — | Commands under review, pills, metadata |
| 10px | 800 | Uppercase eyebrow labels only |

Never go below 10px, and only for uppercase labels with adequate letter-spacing.

**The one exception is a glyph-only control.** A button whose whole content is
a `+` or `×` sizes that character to the icon it is standing in for, not to the
type scale — currently 20-25px with `line-height: 1`. If a rule sets a
font-size outside the table, it must be one of those.

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
| `.btn-danger` | Transparent, `--danger` text | Delete, remove, revoke |
| `.btn-icon` | Transparent, `5px 8px`, glyph only | Overflow (vertical `.dots-icon`), close, small toggles |
| `.btn-trigger` | 12px / 600 / `5px 6px`, transparent, `--muted` label, caret | Opens a popover from an action row; the label is the current value |

**Rules**

- At most one `.btn-primary` per surface.
- `.btn-primary` and `.btn-quiet` are hierarchy; `.btn-danger` is semantics.
  A destructive action takes `.btn-danger` **and** keeps its confirm dialog —
  the colour makes it findable, the dialog is what actually protects the user.
  Never give it a fill: that would make Remove the loudest control on the page
  and invert the hierarchy.
- `.btn-trigger` is the one step whose label is a **value, not a verb** — a
  model id, an effort level, a mode. It never takes a fill in any state; hover
  and open shade the background only. In an action row the filled control is
  the action, and a setting must not outweigh it. Its label never wraps, and a
  trigger whose value can be arbitrarily long caps its width and truncates
  rather than pushing the row around.
- **The overflow trigger is a vertical ⋮, always.** It is a `.btn-icon` containing
  `<span class="dots-icon" aria-hidden="true"></span>`, with an `aria-label` that
  names what the menu is for ("More approval options", not "More"). Never a
  text glyph: not `⋯`, not `…`, not `⋮` typed as a character, because a glyph's
  weight and baseline vary by font and it drifts out of line with the drawn one.
- Every interactive control is at least 24×24 CSS px. `.btn-sm` at 12px text
  computes to roughly 29px tall — do not shrink its padding further.
  `.btn-trigger` computes to 27px on the same arithmetic.
- Border radius stays at the existing 8px across all steps. The scale changes
  size and weight, not shape.
- Focus is always visible: `box-shadow: 0 0 0 3px var(--accent-soft)` with
  `border-color: var(--accent)`. Never remove the focus ring to tidy a layout.

**Contrast floors** (measured against `--surface`)

- White on `--accent` — 6.4:1. Passes AA for all sizes.
- `--danger` on `--surface` — 5.9:1. Passes AA for all sizes.
- `--muted` on white — 4.8:1. Passes AA for normal text, with little headroom.
  `.btn-quiet` text must not be lightened past `--muted`.

---

## 4. Action hierarchy

Every group of actions resolves into three tiers:

- **Primary** — the expected action. One, filled.
- **Secondary** — the other reasonable answer, usually the negative one. Quiet.
- **Overflow** — behind the vertical ⋮ trigger (§3). Everything that is rare, destructive, or
  **persistent beyond the current interaction.**

The overflow rule is the important one. A choice that changes state after this
turn ends — writing an allowlist, granting a session-wide permission, deleting
a stored item — must not sit at the same visual weight as the one-shot action
next to it, because the cost of a mis-click is not symmetric.

**A surface with no safe action has no primary.** §1's "one primary action per
surface" is a ceiling, not a quota. The crash recovery card
([spec 45](specs/45_crash_recovery_prompt.md)) drops its fill entirely when
`Resume` is not on offer, leaving `Mark failed` quiet and `Abandon` in the
overflow: filling the only remaining control would make a terminal choice the
loudest thing on a surface that exists because Damaian could not tell what
happened.

There are two popover implementations and they are not interchangeable:

- **`.model-popover`** — `position: absolute`, anchored by CSS to a fixed
  ancestor. Correct for controls in the composer, which never scrolls.
- **`.context-menu-popover`** — a single shared `position: fixed` element on
  `document.body`, anchored to the trigger's `getBoundingClientRect()` on open.
  Correct for anything inside a scrolling region, and for repeated rows where
  per-row popovers could not hold stable state.

Anything in the conversation log or the project list uses the second, built
with `createContextMenu` (below). Dismissal (document click, `Escape`) is
already wired by that factory — do not add document-level listeners of your
own. Do not introduce a third implementation.

### The ⋮ menu

Every overflow and folder menu is an instance of one factory in `app.js`,
`createContextMenu({ className, align, placement, onAction })`
([spec 58](specs/58_header_repo_bar_and_menus/proposal.md) §3.1). The project
row, session row, thread header and repo-bar folder menus use it, as do the
approval and crash-recovery overflows and the composer's mode pill. Build the
menu once at module level and call `menu.toggle(trigger, { sections, key,
context })` from the trigger's click handler, stopping propagation.

- **Sections, not rows.** `sections` is an array of arrays; the factory draws a
  `.context-menu-divider` between non-empty sections. Group by kind — *Open in*,
  then actions on the item, then destruction — rather than placing dividers by
  hand.
- **Drill-in, not fly-out.** An entry with `items` becomes a row with a
  `.context-menu-caret` that swaps the panel for a sub-panel headed by a
  `.context-menu-back` row named after it. One behaviour for every menu
  matters more than a fly-out's speed.
- **Danger is last, always.** An entry with `danger: true` renders as
  `.context-menu-row-danger` and is moved to the end of its panel whatever
  order it was given in, so no menu can drift. It keeps its confirm dialog
  (§3).
- **Conditional rows are `hidden`, not absent from one caller.** A row that
  depends on state (the folder menu's GitHub item) passes `hidden: true`; one
  that is momentarily unavailable passes `disabled: true` and stays visible.
- **Keyboard.** Opening focuses the first row. ArrowUp/ArrowDown move and wrap,
  Home/End jump, ArrowRight drills in, ArrowLeft goes back, Escape and Tab
  close and return focus to the trigger. Choosing a row closes the menu and
  returns focus to the trigger *before* the action runs, so a dialog it opens
  takes focus from there. Opening one menu closes any other.
- **Trigger.** A `.btn-icon` with `.dots-icon` (§3), or a `.btn-trigger` when
  its label is a value (the folder name, the mode). It carries
  `aria-haspopup="menu"` and starts at `aria-expanded="false"`; the factory
  keeps that attribute current.
- **Placement.** Below the trigger, flipping above when it does not fit below
  and there is more room above. `placement: "above"` prefers above, which
  anything in the composer needs because it sits at the bottom of the window.
  `align: "start"` lines up the left edges; the default lines up the right.

A row whose ⋮ only shows on hover hides it with `opacity`, never `display` or
`visibility`, and shows it on `.active`, `:hover`, `:focus-within` and
`[aria-expanded="true"]`, so every row stays reachable by Tab.

### The repo bar

`.repo-bar` is the strip inside the composer directly above the prompt box
(spec 58 §3.3): one control-height tall, the prompt box's width, on
`--sidebar`. It holds state about the open folder, never per-turn state.

- **Left:** `.repo-folder-btn`, a `.btn-trigger` with the folder glyph, the
  folder name and a caret, opening the folder menu above it; then
  `.repo-branch`, the branch in muted monospace, or the short SHA on a detached
  HEAD.
- **Right:** `.repo-changes`, the working tree's `+N −M` in monospace,
  `.repo-insertions` in `--ok` and `.repo-deletions` in `--danger`,
  display-only and hidden when both are zero; then the contextual-action slot
  `.repo-action`.
- **A folder that is not a repository** shows the folder button alone.
- **The contextual action is one slot with a priority list**, not a row of
  buttons. The first applicable entry wins and the slot is hidden when none
  applies: open error findings (*Fix N errors*), then open warnings (*Fix N
  warnings*), then (once built) uncommitted changes (*Commit…*, spec 35), then a
  branch ahead of its base (*Create PR*, spec 37). New entries join the list in
  `repoActionProviders`; they never add a second control to the bar.
- **The action is a split button**: `.repo-action-main` runs it,
  `.repo-action-more` (a chevron) opens its detail as a `.model-popover`
  anchored to the bar and opening upward. It is tinted by `data-tone`
  (`danger`, `warning`) and **never filled** — the composer's send button stays
  the one filled control. It is disabled while a turn runs.

`.inline-actions` is the shared action row: a content-sized flex row, the same
shape as `.command-approval-actions` and `.patch-actions`. Use it anywhere a
group of buttons sits together. It was a `1fr 1fr` grid until spec 43, which
produced 456px buttons in settings and wrapped any third button onto its own
line.

**Footer control rows** — `.composer-actions` is the shape for a row of
controls that belongs to the surface *above* it rather than to a card. Glyph
buttons and `.btn-trigger` settings on the left, the single filled action on
the right, pushed there by `margin-right: auto` on the last item of the left
group. Two rules matter:

- **The controls sit below the field, never over it.** See the §8
  anti-pattern.
- **The action anchors the right edge and does not move.** Settings grow
  leftward as their values lengthen, so the button the user aims at is always
  in the same place — including when it changes into Stop mid-turn.

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
| Sizing action buttons by column fraction | A `1fr` button is as wide as its container; at 930px that is a 456px Save Key. Action rows size to content |
| Per-turn information docked in chrome | Costs space on every turn and shows only the newest turn. The context strip was 99px of permanent band showing one turn's files; it now sits inside the turn that read them |
| Full paths as full-size buttons | Maximum visual weight for reference information. Reference lists are quiet links, truncated from the left so the filename survives |
| Horizontally scrolled command under review | User approves what they cannot see |
| Escalating and one-shot actions at equal weight | Mis-click cost is not symmetric |
| Controls positioned over the field they displace | The field then pays for them in padding it cannot use. The composer's textarea reserved 70px of height and 58px of *every line* to keep text clear of three floating controls — 41px of text inside a 111px field, and text stopping 59px short of the edge on every row. The `@media` block holding that layout together at narrow widths was three rules whose only job was to stop it colliding. Controls belong in a row of their own |
| Turn state in a header badge | Nobody looks at the header during a turn; they look at the reply. `#chat-status` was the only progress signal for four turn paths. Running state is the in-bubble turn indicator; an end state is a mark on that turn's message (spec 58 §3.2) |
| Horizontal `⋯` or a text-glyph overflow trigger | Vertical ⋮ was asked for and applied to one control while this guide still said `⋯`; the approval and recovery overflows were then built from the guide and came out horizontal. The trigger is `.dots-icon` (§3), and the guide is what gets fixed first |
| Two values sharing one truncating label | Whichever is last is the one that disappears. `model + effort` in one 220px pill rendered `claude-3-5-sonnet-202…` — the effort still in force and readable nowhere but the popover. One trigger per value |

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
