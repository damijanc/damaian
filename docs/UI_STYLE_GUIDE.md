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
6. **Every state has one home.** A pending decision lives in its card at the
   end of the conversation; a running action lives in its status row in the
   turn. Nothing else repeats the state while that home is in view. A pointer
   elsewhere — the composer status slot (§4) — appears only while the home is
   scrolled out of view, and goes away when it comes back
   ([spec 112](specs/112_status_in_one_place/proposal.md)).
7. **One vocabulary of states.** The same state has the same name and the same
   mark on every surface: *Working*, *Needs you*, *Failed*, drawn with the one
   `.worker-dot` (§4) in the sidebar, on the Workers board and in the worker
   strip, and named the same in the composer status slot. Do not invent a
   second word or a second colour for a state that already has one.

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
| `--muted` | `#646b77` | Secondary text, quiet controls |
| `--line` | `#deded8` | Dividers, low-emphasis borders |
| `--line-strong` | `#c8c8c0` | Control borders |
| `--accent` | `#176b5d` | Primary actions, focus |
| `--accent-soft` | `#e7f1ee` | Focus ring, accent backgrounds |
| `--ok` / `--warn` / `--danger` | `#26734d` / `#a15c18` / `#a33737` | State only, never decoration |
| `--code` | `#101820` | Code block background |
| `--on-accent` | `#ffffff` | Text on a filled `--accent` or `--danger` control |
| `--ink-strong` | `#181a1c` | Settings page and section headings |
| `--faint` | `#9a9da0` | Drawn glyph strokes (chevrons, reset), "Soon" labels. Not for text that must be read: below the `--muted` floor |
| `--hover` | `#f4f4f1` | Hover background of menu rows and the attach button |
| `--line-soft` | `#eeeeeb` | Dividers inside a popover; pressed attach button |
| `--code-soft` | `#f6f7f7` | Inline command and output backgrounds on cards and dialogs |
| `--ok-line` | `#8dbda5` | Border of an applied or OK state |
| `--warn-soft` / `--warn-line` / `--warn-ink` | `#fdf3e3` / `#e3cba8` / `#7a4b12` | Warning chip: fill, border, text |
| `--danger-soft` / `--danger-line` | `#fbe9e7` / `#d89999` | Danger hover and blocked fill; border of a rejected or error state |

A literal that appears in three or more rules becomes a token (spec 112). The
syntax-highlighting colours stay literals, grouped under their own comment in
`style.css`.

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
| `.icon-btn` | 28×28, transparent, `--muted`; hover `rgba(0,0,0,.05)`; `[aria-pressed="true"]` takes the accent tint | Every header control: symbol search, terminal and the session ⋮ (which holds `.dots-icon` instead of an SVG). Other glyphs hold one `icon()` SVG (24×24 viewBox, 1.7 stroke, 16px). Keep its `aria-label` and `title` |
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
- **The overflow trigger is a vertical ⋮, always.** It is a `.btn-icon` (the header's, a `.icon-btn`, so it matches its neighbours) containing
  `<span class="dots-icon" aria-hidden="true"></span>`, with an `aria-label` that
  names what the menu is for ("More approval options", not "More"). Never a
  text glyph: not `⋯`, not `…`, not `⋮` typed as a character, because a glyph's
  weight and baseline vary by font and it drifts out of line with the drawn one.
- Every interactive control is at least 24×24 CSS px. `.btn-sm` at 12px text
  computes to roughly 29px tall — do not shrink its padding further.
  `.btn-trigger` computes to 27px on the same arithmetic.
- Border radius stays at the existing 8px across all steps. The scale changes
  size and weight, not shape. The one exception is `.btn-trigger`, which is
  borderless and uses 6px.
- Focus is always visible: `box-shadow: 0 0 0 3px var(--accent-soft)` with
  `border-color: var(--accent)`. Never remove the focus ring to tidy a layout.

**Contrast floors**

- White on `--accent` — 6.4:1. Passes AA for all sizes.
- `--danger` on `--surface` — 6.6:1. Passes AA for all sizes.
- `--muted` is the floor for secondary text, and it sits on every background,
  so it is measured against each: 5.4:1 on `--surface`, 5.0:1 on `--bg`, 4.7:1
  on `--sidebar` (the sidebar and the repo bar), and 4.7:1 on `--accent-soft`
  (your own messages, hovered search results, a pressed terminal toggle). All
  pass AA for normal text. The weakest pair sets the limit: `--muted` must not
  be lightened, and secondary text must not be put on a background darker than
  `--sidebar`, without re-measuring.

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

A row whose ⋮ only shows on hover (a session row, or a project row, whose *New
session* and ⋮ both hide this way) hides it with `opacity`, never `display` or
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
  applies: uncommitted changes (*Commit…*, spec 35), then a branch ahead of
  its base (*Create PR*, spec 37). Open findings are deliberately not an entry
  (owner report 2026-11-27: a finding that never closed kept *Fix N errors*
  on screen); the findings list opens from the header ⋮. New entries join the list in
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
- **A `.btn-trigger` names its setting.** A `.trigger-key` (muted, weight 500)
  precedes the value — *Mode Code*, *Effort Default* — so a bare value is never
  ambiguous. The model trigger is exempt: its id is self-evident.
- **The action anchors the right edge and does not move.** Settings grow
  leftward as their values lengthen, so the button the user aims at is always
  in the same place — including when it changes into Stop mid-turn.

### The composer status slot and the one Stop

Spec 112 replaced the composer's three stacked status boxes (running,
decision, work this window never streamed) and the in-message pointer ("… the
card is below") with one slot and one Stop.

**The slot.** `#composer-status` (`.composer-status`) sits directly above the
prompt box. Like the repo bar's action it is **one slot with a priority
list**: the first entry that applies is shown, and the slot is hidden when
none does.

| Priority | `data-kind` | Shown when | Text | Button |
|---|---|---|---|---|
| 1 | `needs-you` | A pending decision whose card is out of view | "Needs your answer" for a question, else the decision's label | **Answer** (question) or **Show**: scrolls to and focuses the card |
| 2 | `working` | A running action of this session whose status row (or card's running row) is out of view | The action's label, or "Stopping: …" | None |
| 3 | `busy` | The session works on something this window never streamed (working-sessions poll) | "Busy elsewhere: {label}" | None |

- **Shown only while the home is out of view.** An `IntersectionObserver`
  watches the pending frame and the running action's status rows; while one is
  visible its entry is skipped (§1, "every state has one home"). The observer's
  root is the viewport, because below 1100px the page scrolls rather than the
  log.
- **One line.** `.composer-status-label` is clipped to about 60 characters,
  then a `.composer-status-clock` (muted, tabular numerals). The full text goes
  in the slot's `title`.
- **Only Needs you is tinted** — `--accent` edge on `--accent-soft`. Working
  and Busy elsewhere sit on `--surface-soft` with a `--line` border: they are
  information, not a request.
- **The slot never holds a Stop.** Its one optional button is a `.btn-sm`.

**The one Stop.** While anything in the current session runs — a turn this
window streams, a resumed stream, a running action, or work another window
started — the composer's send button is the Stop (`.send-button.is-stopping`,
`aria-label` "Stop", `title` "Stop (Esc)"), and Escape does the same. It stops
the work through the path that work needs: the stream's abort, the running
action's `stop()`, or `/api/session-work-cancel` for work this window does not
own.

- **The turn's status row has no Stop**, and neither does the slot.
- **Work that cannot be stopped** leaves a disabled Stop with the reason in its
  `title`, rather than an enabled control that does nothing.
- **A card's own Cancel row stays** (`.running-row` on a check batch or a
  reattached command): it is that card's action, not a second Stop for the
  session.
- Enter with text in the prompt during a turn still sends a steering note
  (spec 110).

### Worker sessions

Worker sessions ([spec 57](specs/57_worker_sessions/proposal.md) §5.7) appear
on four surfaces — the sidebar, the Workers board, the strip at the top of a
worker's own session, and its composer — and all four share one status mark.

**The status dot.** `.worker-dot` is an 8px circle whose `data-state` picks its
look. State colours only (§2):

| `data-state` | Look | Means |
|---|---|---|
| `running` | `--accent` fill | A turn is in progress |
| `needs-you` | `--warn` fill | Waiting for an approval |
| `paused` | Hollow, 1.5px `--warn` ring | Paused: it needs you, but it is not asking |
| `finished` | `--line-strong` fill | Idle, or ended cleanly |
| `failed` | `--danger` fill | Failed, interrupted, or its outcome is unknown |

- **Finished is neutral, not `--ok`.** Green beside the accent reads as a
  second "running".
- **The dot never carries the state alone.** In the sidebar and on the board it
  is `role="img"` with an `aria-label` and `title` naming the state; in the
  strip it sits inside `.worker-pill` beside the state's text and is
  `aria-hidden`. Keep it that way: the finished dot is about 1.5:1 against
  `--sidebar`, too faint to be the only signal.

**Sidebar.** There is no Workers entry (spec 95): the header ⋮'s *Workers*
opens the one board for every project. A worker's session is listed under the
session that started it as `.project-worker-item.nested`, indented one 14px
step, with its dot where a child's bullet would be.

**Session dots.** An ordinary session row uses the same dot for three states
only (spec 112), from `/api/sessions`' `state` field:

| `state` | `data-state` | Label | Means |
|---|---|---|---|
| `working` | `running` | Working | The session is in the working-sessions list |
| `needs_you` | `needs-you` | Needs you | Its latest task waits on a decision |
| `failed` | `failed` | Failed | Its latest task ended failed |

- **Idle draws nothing.** Idle, complete, stopped and cancelled sessions keep
  their relative time (`.session-when`); there is no finished dot on a session
  row, because a column of grey dots would say nothing.
- **The dot replaces the time** in the same column (`.session-state-dot`,
  centred there): while a session needs you, that is the news, not when it last
  changed.
- Like every sidebar dot it is `role="img"` with an `aria-label` and `title`
  naming the state. Worker rows keep their own engine-computed dot.
- **A collapsed project names its count**: `.project-need-you`, "2 need you" in
  `--warn-ink` at 11px, after the project name. Text, not a dot: the count is
  the information. An expanded project shows no count, because its rows carry
  the dots.

**The Workers board.** `.workers-board` takes the conversation's place: while
`body.workers-board-open` is set, the log, the composer and the worker strip
are hidden, and the board uses the conversation gutters (§6). Its
`.workers-table` is grouped into one `tbody.workers-group` per state — *Needs
you*, *Running*, *Finished* — each headed by an uppercase eyebrow row with its
count. Columns: dot, worker (title, then its branch in muted monospace),
project (muted, truncated like the planner), planner, rounds (`.workers-rounds-bar`, 48×4px, `--accent` on `--line`, then
`n/m`), what it is doing, elapsed time, and actions.

- **One `.btn-sm` per row, unfilled** — *Review* when it needs you, *Pause*
  while it runs, otherwise *Open* — plus the ⋮ menu (§4) with *Reveal worktree
  in Finder*, *Dismiss from board* and, last, *Stop*. A board is many surfaces
  at once, so no row takes a fill.
- Text cells truncate on one line; the board redraws only when what it shows
  changed, so a focused button or open menu is not replaced every second.

**The worker strip.** `.worker-strip` sits under the header of a worker's own
session: one row at least 40px tall, on `--surface-soft`. Left to right: the
`.worker-pill` names the task outcome, not bare process state — *Working*,
*Needs your decision*, *Task complete*, or the specific stopped outcome. While
working or paused it is followed by `.worker-round` (the rounds bar and *Round
n/m*); `.worker-strip-note` gives the durable next action or the time since the
last worker update, and may wrap rather than hiding that outcome.
`.worker-strip-actions` are pushed right.
The actions follow §4: *Pause* is a `.btn-sm`, *Resume* is the one
`.btn-primary`, *Stop* is `.btn-danger`; an ended worker shows only *Restart
worker*.

**The worker's composer.** The round send button becomes
`.send-button.is-worker-command`, a labelled pill that says what it sends —
*Send note* while running, *Resume with note* while paused, *Send message*
when idle — except *Send follow-up* after completion and *Continue task* after
an unfinished settled turn — and stays the row's one filled control. A pending
decision disables it so the review card remains the only route forward.
`body.worker-view` hides
the attach and model menus, because neither reaches a worker.

**In the parent conversation.** A `worker-prompt` code block gets a
`.worker-prompt-footer` under it: a *Start worker* `.btn-sm`, then a
`.worker-link` (accent text, no button chrome) to each worker it started. A
`.worker-spawned-entry`, one quiet `--muted` line, follows the message. The
*New worker* dialog is `.app-dialog-worker`, 640px wide because the prompt is
its content, edited in a monospace `.worker-dialog-prompt`. When the working
tree has uncommitted changes, `.worker-dialog-dirty` (a `--warn` left border on
`--surface-soft`) names them and asks whether the worker starts from them.

---

## 5. Approval surfaces

Command approval and patch preview are held to stricter rules than the rest of
the UI, because the user is granting consent rather than navigating.

- **The subject is shown in full.** A command under review wraps and is shown
  in its entirety — never horizontally scrolled, elided, or capped behind a
  scroll container. The user must be able to read everything they are
  approving without interacting first.
- **The deciding reason is not progressive.** The policy's first cause and the
  agent's own stated reason sit on the card outside any disclosure (spec 62):
  they are what tells the user whether a prompt is routine, and hiding them is
  what made forty prompts read alike. The full prompt, with every cause, stays
  behind a collapsed **Why this command** disclosure. Explanatory prose beyond
  those two lines is progressive; it renders collapsed every time, and
  disclosure state is not remembered between proposals.
- **Escalations live in the overflow.** "Always allow", session-wide grants,
  and anything else that outlives the turn.
- **Nothing is pre-selected toward approval.** Focus does not land on the
  primary action.

### Hook review

Spec 32 lets a repository ship a hook. It does not run until the user approves
the exact definition, so this is a consent surface rather than a setting:

- **The command line is shown in full**, wrapped, never scrolled or elided.
- **Its events and fail direction sit outside any disclosure** — "Events:
  before_patch_apply, before_tool · If it fails: blocks".
- **Decline is the default action.** It takes focus; Approve is never
  `.btn-primary` and is never pre-selected.
- **Dismissing records no decision**, and the notice returns next time.
- The id, command and arguments are repository-authored, so they enter the DOM
  through `textContent`, never as markup.
- An approval is taken at the definition's hash: if the definition changes, the
  hook stops until it is reviewed again, and the prompt says so.

### Question surface

Spec 50 lets the model ask one bounded question mid-turn. That surface is
deliberately **not** an approval surface: answering a question authorises
nothing, and a user who learns to click through questions must not have learned
to click through approvals. It lives in `.question-card`.

- **It borrows nothing from `.command-approval`** — no header, no risk badge,
  no footer, no disclosure. The dashed border marks a prompt, not consent.
- **Every option is a secondary `.btn-sm`, and there is no `.btn-primary`.**
  No answer is the safe one, so no answer is emphasised. That is §4's "a surface
  with no safe action has no primary" applied to content as well as outcome.
- **Nothing is focused or pre-selected.** The card is answerable, not answered
  by default; the free-text field takes focus only when the user acts on it.
- **Answered, it shows the answer and disables itself.** The answer is also a
  user-side transcript message, so a reloaded session reads the exchange as
  what it was: a question the assistant asked and an answer the user gave.
- **The question and options are model-authored text**, so they are set with
  `textContent`: no markdown, no links, no HTML.

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

### Command steps

Inside a turn's Activity (spec 67), a command whose result has the engine's
fixed shape (`Command: … Exit code: … STDOUT: … STDERR: …`, spec 68 §5.1) is
one row, `.activity-command`, which is itself the trigger for its output:

- `$ <command>` in monospace, truncated with an ellipsis, full text in `title`.
- An `.exit-mark[data-state]`: `✓ 0` in `--ok`, `exit N` in `--danger`, and
  `timed out` or `cancelled` in `--warn`. Exit `-1` is a failure, never `✓`.
- The model's reason in `--muted`, and `approved` after the exit mark when the
  user approved this run.
- The collapsed-by-default panel holds the working directory only when it is not the
  repository, one labelled `<pre>` per non-empty stream (stored bytes,
  `[truncated]` kept), or `No output` when both are empty.
- **A hook note is never stderr.** The `after_tool` hook's trailing
  `` Hook `<id>` `` line is Damaian's note, not the command's output: it is
  split off and shown under the output in `--warn` (`.activity-hook-note`).
- The approval request the run answered is drawn inside its step, under a
  quiet *Approval request* label, not as its own bubble. Only a request
  followed by a run that parses is absorbed; a declined or pending one stays a
  bubble. This is history, not consent, so §5 does not apply to it; the live
  approval card is unchanged.
- A result that does not parse keeps the plain title and `<pre>`: nothing is
  hidden by a failed parse.

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
| Chrome with nothing to say | An empty *Workers* entry and a permanent *Check Updates* button cost space on every screen to report nothing. Draw an entry only when it has state to show, and keep the way back to it in a menu or in Settings |
| The same state in several places | One pending question was announced six times: a pointer in the assistant's message ("the card is below"), the waiting row, the card, a second answerable copy inside a recovery card, a composer line and a placeholder saying "above". The pointer said *below* and the placeholder *above*, both true of different copies. A state has one home; the composer slot points at it only while it is out of view, and a reattached card is answerable once (spec 112) |
| A Stop on every surface | A running turn had a Stop on its status row and a Stop in the composer, sometimes a third in a status line, and answering a question turned Send into a Stop that did nothing. The user cannot tell which one stops what. The composer's send button is the one Stop for a session's running work; a card keeps only its own Cancel (spec 112) |
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
