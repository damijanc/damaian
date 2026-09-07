# Composer Control Density — Task List

**Implements:** [`proposal.md`](proposal.md) · background in [`context.md`](context.md)
**Style reference:** [`../../UI_STYLE_GUIDE.md`](../../UI_STYLE_GUIDE.md) · rendered at `docs/ui-style-guide.html`
**Started:** 2026-09-07

## Progress

| Task | State | Notes |
|---|---|---|
| 0 · Baseline measurement | Not started | |
| 1 · Prompt box frame and chips | Not started | |
| 2 · Action row and control sizes | Not started | |
| 3 · Split triggers | Not started | |
| 4 · Specimen page, guide and outcome | Not started | |

## Baseline

**Not yet measured.** Task 0 records it before anything changes. The guide's
§8 anti-pattern table is explicit that an estimate is not a measurement, so the
figures below are the *declared CSS values* — facts about the stylesheet, not
about the rendered composer — and Task 0 replaces the rendered column.

| | Declared in CSS | Measured at 1280×800 |
|---|---|---|
| `.prompt-box textarea` min-height | 111px | — |
| ... of which padding | 70px (14 top + 56 bottom) | — |
| ... of which text | 41px (2 rows @ 20.3px) | — |
| Right gutter per line | 58px | — |
| Attach button | 38×38 | — |
| Model pill | `min(220px, 42vw)` × 38 | — |
| Send button | 38×38 | — |
| Composer at rest (14 + field + 18) | — | — |

## How to verify

Shell on port 4899. Static assets are `include_str!`-embedded — rebuild and
restart for every visual change, or the binary shows stale UI. **Kill by PID,
never by name**: the installed app shares the `damaian-desktop-shell` binary
name and `pkill -f` takes it down too.

Measure the composer and its controls from the browser console:

```js
const px = (el) => { const r = el.getBoundingClientRect(); return `${Math.round(r.width)}×${Math.round(r.height)}`; };
["footer.composer", ".prompt-box", "#chat-prompt", ".composer-actions",
 "#composer-attach-btn", "#chat-model-menu-btn", "#ask-btn"]
  .forEach((s) => { const el = document.querySelector(s); console.log(s, el ? px(el) : "absent"); });
```

Pin chips without a file picker:

```js
document.querySelector("#composer-context").hidden = false;
```

Drive the busy state without a provider key:

```js
setComposerBusy(true);   // send must read as Stop, in place
setComposerBusy(false);
```

Exercise requirements 3 and 4 by applying a custom model id through the
popover's Model panel — `deepseek-v4-flash`, then
`us.anthropic.claude-sonnet-4-20250514-v1:0`.

---

## Task 0 · Baseline measurement

**Files:** none — `tasks.md` only

- [ ] Rebuild and restart the shell at 1280×800.
- [ ] Run the measurement snippet above and fill the Baseline table's measured
      column, including composer height at rest with nothing pinned.
- [ ] Record the rendered width of `.model-menu-button` with `gpt-4.1`
      selected and with `claude-3-5-sonnet-20241022` selected, and confirm the
      effort truncation from `context.md` §1 actually reproduces. If it does
      not, correct `context.md` §1 rather than leaving the claim standing.
- [ ] Commit the baseline.

## Task 1 · Prompt box frame and chips

**Files:** `index.html`, `style.css`

- [ ] Move `#composer-context` inside `.prompt-box`, before the textarea,
      keeping its id and the `#pinned-context-files` child so `app.js` needs no
      change.
- [ ] `.prompt-box` → flex column; take the border, radius and `--surface`
      background off the textarea and onto it; add `.prompt-box:focus-within`
      carrying the standard focus ring from guide §3.
- [ ] `.prompt-box textarea` → transparent, `border: 0`, `padding: 10px 12px`,
      `min-height` recomputed for two rows with symmetric padding. Keep
      `max-height: 40vh` and `resize: none`.
- [ ] Cap `#composer-context` with `max-height` + `overflow-y: auto`. It sets
      `display: grid`, so it needs its own `[hidden] { display: none }` — guide
      §7's last bullet.
- [ ] Rewrite the comment above `.prompt-box textarea` (`style.css:1303`): it
      documents the 70px control reserve, which is the thing being removed.
- [ ] Verify: chips appear inside the frame; unpinning returns the composer to
      its exact at-rest height; focus ring lights the whole frame; `autoGrowPrompt`
      still grows and caps. Lint. Commit.

## Task 2 · Action row and control sizes

**Files:** `index.html`, `style.css`

- [ ] Add `.composer-actions` after `.prompt-box` and move `#composer-attach-menu`,
      `#chat-model-menu` and `#ask-btn` into it, in that order. The three hidden
      inputs stay where they are.
- [ ] `.composer-actions` → flex row, `align-items: center`, `gap: 6px`,
      `margin-top: 6px`; `#composer-attach-menu` takes `margin-right: auto`
      directly — no wrapper element — so send right-aligns and the model group
      grows leftward.
- [ ] Drop `position: absolute` and all offsets from `.attach-menu`,
      `.model-menu` and `.send-button`. Delete the `@media` block at
      `style.css:2862`–`2868`.
- [ ] `.attach-menu-button` and `.send-button` → 26×26. Keep the send fill and
      the `.is-stopping` square; scale the glyphs to match.
- [ ] Re-anchor `.attach-popover` and `.model-popover` upward from the row —
      `bottom: calc(100% + 6px)`, attach left-aligned, model right-aligned.
- [ ] Verify: nothing overlaps the textarea; every control ≥ 24px; both
      popovers open upward and clear the row; the row does not wrap at the
      900px breakpoint; send does not move when the model changes. Lint. Commit.

## Task 3 · Split triggers

**Files:** `index.html`, `style.css`, `app.js`

- [ ] Add `#chat-effort-menu-btn` beside `#chat-model-menu-btn`, both inside
      `#chat-model-menu`, both `.btn-trigger`, each with `aria-haspopup="menu"`,
      `aria-expanded="false"`, `aria-controls="chat-model-popover"` and a
      `data-entry-panel` of `root` and `reasoning`. Keep the bolt icon on the
      model trigger only.
- [ ] Add `.btn-trigger` per `proposal.md` §3.4 and give the model trigger
      `max-width: min(340px, 40%)` with ellipsis. Delete
      `.model-menu-button`'s fixed width.
- [ ] `openModelMenu` / `closeModelMenu` → write `aria-expanded` to both
      triggers (query `[data-entry-panel]` rather than naming ids).
- [ ] `toggleModelMenu(panel)` → close if that panel is already showing, switch
      panel if the other is, open otherwise. Bind each trigger to its own
      `data-entry-panel`. **Do not** reuse `data-panel`: the blanket binding at
      `app.js:4941` would switch panels without opening the popover.
- [ ] Split `modelSummaryLabel` into `modelTriggerLabel` and
      `effortTriggerLabel`; have `renderChatModelMenu` set both labels and put
      the full model id in the model trigger's `title`. Keep the `"Configured"`
      and `"Default"` fallbacks.
- [ ] Verify acceptance criteria 3, 4, 6, 7 and 8 by hand, including the
      42-character id. Lint. Commit.

## Task 4 · Specimen page, guide and outcome

**Files:** `docs/ui-style-guide.html`, `docs/UI_STYLE_GUIDE.md`, spec folder, `docs/specs/README.md`

- [ ] Guide §3: add `.btn-trigger` to the button table, with a note that it
      never takes a fill and that its label is a value rather than a verb.
- [ ] Guide §4: add the composer action row to the section as the standard
      shape for a footer control row — quiet triggers and glyphs left, the one
      filled action right — alongside the existing `.inline-actions` note.
- [ ] Guide §8: add the anti-pattern this spec removes — *controls positioned
      over the field they displace*, with the 70px/58px reserve and the
      `@media` counter-override as the evidence.
- [ ] Specimen page: add the composer action row and `.btn-trigger` (default,
      hover, open, and a truncated long label) to its inventory, and run the
      page's missing-class check.
- [ ] Fill the Baseline table's after-column, record before/after in
      `proposal.md`'s status line, and set the `docs/specs/README.md` row to
      Done. Commit.
