# Feature Spec: Composer Control Density

Status: Done. Composer at rest **149px → 129px** at 1280×800, and the 58px
right gutter every line paid is gone — text now reaches within 13px of the box
edge. Usable text went 39px → 41px, a clean two rows instead of 1.92. Attach
and send 38px → 26px; the 220px fixed model pill became two content-sized
triggers, 81px and 66px at the default model. The effort label can no longer be
truncated away. Four corrections the implementation forced are recorded in
[`tasks.md`](tasks.md) under Corrections.
Order: 44 of 44
Also in this spec: [`context.md`](context.md) (motivation, current state, and
the two defects this redesign resolves), [`tasks.md`](tasks.md) (execution
order, baseline measurements and progress).
Reference: [`../../UI_STYLE_GUIDE.md`](../../UI_STYLE_GUIDE.md) — this spec adds
the `.btn-trigger` step, the composer action-row pattern, and one anti-pattern
to the guide, then applies them to the composer.
Related implementation specs:
[`../41_ui_density_and_action_hierarchy/proposal.md`](../41_ui_density_and_action_hierarchy/proposal.md)
(the button scale and the specimen page this extends),
[`../42_conversation_column_density/proposal.md`](../42_conversation_column_density/proposal.md)
(grew the composer from two rows instead of four, and moved the docked context
strip into the turn — this finishes the composer that spec left),
[`../43_chrome_density_and_hierarchy/proposal.md`](../43_chrome_density_and_hierarchy/proposal.md)
(the 24×24 control floor and the type scale this obeys),
[`../31_permission_profiles.md`](../31_permission_profiles.md) (the working-mode
control the new action row leaves room for; not built here).

**Not a roadmap graduation.** Fourth spec from the same usability review as
#41, #42 and #43, covering the one surface those three did not reach. Reported
in use: the model, attach and send controls read as oversized and sit inside
the input field rather than beside it.

## 1. Requirements

1. No composer control is positioned over the textarea, and the textarea
   reserves no padding for one. Its padding is symmetric.
2. Attach, model, effort and send sit in one content-sized row below the prompt
   box, in that visual and DOM order.
3. Model and effort are separate triggers. Neither can truncate the other.
4. The model trigger sizes to its content up to 340px, truncates from the
   right beyond that, and carries the full model id in `title`. The cap is
   absolute, not a percentage — see [`tasks.md`](tasks.md) Corrections §1.
5. Send remains the Stop control while a turn is running, in place, and is the
   only filled control in the row.
6. Pinned context chips render inside the prompt box, above the textarea, and
   the box costs nothing extra when nothing is pinned.
7. Every control in the row is at least 24×24 CSS px, keeps a visible focus
   ring, and is reachable by keyboard in visual order.
8. One popover implementation, one popover element, two triggers. No third
   popover implementation and no second copy of the panel markup.
9. The style guide documents the new step and the row pattern, and the specimen
   page renders them.

## 2. Non-goals

- **No working-mode control.** The reference design this was modelled on shows
  a permission-mode control (`Accept edits`) in the same row. Damaian has no
  such control; it belongs to #31. The row's left group is built so one can be
  added beside attach without relayout, and nothing more.
- **No microphone or voice input.** Also in the reference design, also absent
  here, and not proposed.
- **No change to the model menu's contents.** Provider, Model, Effort and Reset
  keep their current panels, options and persistence. Only the entry points
  change.
- **No change to send/stop behaviour.** `setComposerBusy`, `stopCurrentTurn`
  and the disabled-when-empty rule are presentation-only edits here.
- **No change to the at-rest row count.** The textarea still shows two rows of
  text at rest, as #42 set it.
- **No new truncation policy for model ids.** Right-truncation with a `title`
  and the popover's full-name row. The app does not attempt to parse provider
  naming conventions to shorten an id "intelligently".

## 3. Design

### 3.1 Prompt box owns the frame

`.prompt-box` becomes a flex column that draws the border, background and
radius. The textarea inside goes transparent and borderless, and the focus ring
moves to `.prompt-box:focus-within` so the whole frame lights up.

```
.prompt-box            flex column, border, radius, --surface
├── #composer-context  chips, [hidden] when empty, max-height + scroll
└── #chat-prompt       textarea, transparent, no border, padding 10px 12px
```

`autoGrowPrompt` is unchanged: it sizes the textarea from `scrollHeight`, and
the box grows around it. `max-height: 40vh` stays on the textarea, so the cap
still holds. The chip area is capped with `max-height` and scrolls per guide
§6 rather than growing without bound.

Chips move inside because they describe the message about to be sent, not the
application — guide §1.1. The strip above the box was already hidden when
empty, so this is a placement change, not a density one.

### 3.2 The action row

A new `.composer-actions` flex row follows the prompt box:

```
[ + ]                        [ gpt-4.1 ▾ ] [ Extra High ▾ ]  ( ➤ )
└ left group, margin-right: auto   └────── #chat-model-menu ─────┘
```

- There is no wrapper element for the left group. `#composer-attach-menu` takes
  `margin-right: auto` directly, which is what pushes everything else right.
  #31's mode control becomes its sibling, inserted after it and before
  `#chat-model-menu`, and the `margin-right: auto` moves to whichever is last
  in that group. No relayout, one declaration.
- `#chat-model-menu` keeps both triggers **and** the single popover, so the
  outside-click test at `app.js:4922` — `#chat-model-menu.contains(target)` —
  keeps working with no change.
- Send is last, right-aligned. Because the model/effort group grows leftward,
  send never moves when the model name changes.

Sizes: 26px for attach and send, ~27px for the two triggers, all above the
24×24 floor from guide §3. Exact parity is unnecessary — the row centres them
with `align-items: center` — so the heights come from padding rather than from
a `min-height`, per guide §6.

The popover is right-aligned to the row and opens upward for both triggers, so
no dynamic anchoring is needed. It stays a `.model-popover` — `position:
absolute` in a non-scrolling ancestor, which is what guide §4 prescribes for
composer controls.

### 3.3 Split triggers, one popover

`openModelMenu(panel = "root")` already takes an entry panel, so the split
needs no new opening machinery:

| Trigger | Label | Opens |
|---|---|---|
| `#chat-model-menu-btn` | model id | popover at `root` |
| `#chat-effort-menu-btn` | effort | popover at `reasoning` |

Four edits in `app.js`:

1. `openModelMenu` / `closeModelMenu` write `aria-expanded` to **both**
   triggers. They hardcode one id today (`app.js:1766`, `app.js:1772`).
2. `toggleModelMenu(panel)` takes the panel. Clicking the trigger whose panel
   is already showing closes the popover; clicking the other trigger switches
   panel rather than closing, so the two triggers do not fight.
3. The triggers carry **`data-entry-panel`**, not `data-panel`. Line 4941 blanket
   binds every `[data-panel]` element to a bare `showModelMenuPanel` call that
   switches panels *without* opening the popover; reusing that attribute would
   half-fire and leave the menu shut.
4. `modelSummaryLabel` splits into `modelTriggerLabel` and
   `effortTriggerLabel`. `renderChatModelMenu` sets both, and sets `title` on
   the model trigger to the full id.

The `"Configured"` fallback for an unset model is kept, and `reasoningLabels`
still supplies `"Default"` where a model has no effort setting.

### 3.4 `.btn-trigger`

The guide has no step for a quiet control whose label *is* its current value.
Per guide §3 ("if a surface needs something the scale lacks, extend the
scale"), add one rather than styling these two ad hoc:

| Class | Spec | Use |
|---|---|---|
| `.btn-trigger` | 12px / 600 / `5px 6px`, transparent, `--muted` label, caret | Opens a popover from an action row; the label shows the current value |

At 12px text that computes to roughly 27px tall — the same arithmetic guide §3
records for `.btn-sm`, with the narrower horizontal padding a label-only
control can afford. It keeps the standard focus ring and takes no fill in any
state; hover and open shade the background only, because a fill here would put
the loudest control in the row on a setting rather than on send. Two uses
today; #31's mode control is the expected third.

### 3.5 What gets deleted

| Rule | Location | Why |
|---|---|---|
| `.prompt-box textarea` padding gutters | `style.css:1311` | 56px bottom and 58px right existed only to clear the floating controls |
| `.attach-menu` offsets | `style.css:1316` | `position: absolute; left; bottom` — now a flex child |
| `.model-menu` offsets | `style.css:1416` | Same |
| `.model-menu-button` fixed width | `style.css:1424` | `min(220px, 42vw)`; replaced by the §3.4 content sizing |
| `.send-button` offsets | `style.css:1677` | Same as attach |
| `@media` composer overrides | `style.css:2862`–`2868` | `.model-menu { right }`, `.model-menu-button { width }`, `.model-popover { right }` — all position-based, all dead once the row is a flex layout |

## 4. Acceptance criteria

1. Composer height at rest, measured at 1280×800, is lower than the recorded
   baseline, and the textarea's usable text height is unchanged at two rows.
2. A line of text in the prompt box reaches within 12px of the box's right
   edge — the 58px gutter is gone.
3. With `deepseek-v4-flash` selected, the model trigger shows the id in full
   and the effort label is fully legible beside it.
4. An id too long for the 340px cap ellipsizes, `title` holds the full id, and
   the popover's Model row shows it in full. (Drafted as "a 42-character id";
   42 characters measures 312px and renders in full, which is the generous cap
   working as intended. Verified with a 63-character id.)
5. Selecting a longer model id does not move the send button.
6. Tab from the textarea reaches attach, model, effort, send in that order,
   each with a visible focus ring.
7. Clicking the effort trigger opens the popover directly on the Effort panel;
   clicking the model trigger opens it on the root panel; clicking either while
   the other's panel is open switches panels without closing.
8. `Escape` and an outside click still dismiss both popovers.
9. During a turn, send reads as Stop in place and cancels the turn; attach,
   model and effort stay operable.
10. An empty prompt is refused at submit time with `Prompt is required`.
    (Drafted as "send is disabled", which was a wrong claim about existing
    behaviour — see [`tasks.md`](tasks.md) Corrections §3.)
11. Pinning four files renders chips inside the box; unpinning all of them
    returns the composer to its at-rest height exactly.
12. `npm run lint:web` passes clean, and the specimen page renders the action
    row and `.btn-trigger` with no missing classes.
