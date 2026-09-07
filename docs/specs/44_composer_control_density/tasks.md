# Composer Control Density — Task List

**Implements:** [`proposal.md`](proposal.md) · background in [`context.md`](context.md)
**Style reference:** [`../../UI_STYLE_GUIDE.md`](../../UI_STYLE_GUIDE.md) · rendered at `docs/ui-style-guide.html`
**Started:** 2026-09-07

## Progress

| Task | State | Notes |
|---|---|---|
| 0 · Baseline measurement | Done | Composer 980×149 at 1280×800. Two spec corrections: no `High` effort level exists, and the truncation defect is worse than drafted — effort is absent, not clipped |
| 1+2 · Prompt box frame, chips and action row | Done | **Merged.** Task 1's symmetric padding is only safe once the controls have moved out, so splitting them would have committed a state with text running under the controls. Composer 149→140px after the move, 129px once the triggers landed |
| 3 · Split triggers | Done | Model 81px, effort 66px at the default model. Two defects found and fixed in the process — see Corrections §1 and §2 |
| 4 · Specimen page, guide and outcome | Done | Guide gains `.btn-trigger`, the footer-row pattern in §4 and two anti-patterns in §8. Specimen page gains a `.btn-trigger` row and a Composer action row section, at rest and mid-turn |

## Result

Measured 2026-09-07 at 1280×800, same conditions as the baseline:

| | Before | After |
|---|---|---|
| Composer at rest | 980×149 | **980×129** |
| `.prompt-box` | 852×116 | 852×63 |
| Action row | — (controls overlaid) | 852×27 |
| Usable text height | 39px (1.92 rows) | **41px (2.02 rows)** |
| Right gutter per line | 58px | **13px** (12px padding + 1px border) |
| Attach | 38×38 | 26×26 |
| Model control | 220×38 fixed | 81×27, content-sized (cap 340px) |
| Effort control | none — folded into the pill | 66×27 |
| Send | 38×38 | 26×26 |

Every acceptance criterion verified in the running app. Notes on three:

- **Criterion 2** reads "within 12px"; measured 13px, which is the 12px
  padding plus the box's 1px border. The 58px gutter is what mattered.
- **Criterion 5** holds exactly: send stays at x=1190 across model ids from 7
  to 63 characters.
- **Criterion 7** verified through the full sequence — open on root, switch to
  Effort, close, reopen on Effort, switch back to root — with `aria-expanded`
  tracking on both triggers throughout. Both report `true` while the shared
  popover is open, which is accurate: both `aria-controls` the same element.

## Corrections

Four things the implementation found that the spec had wrong:

1. **`min(340px, 40%)` collapsed the label to `g…`.** A percentage `max-width`
   resolves against the containing block, which is the content-sized
   `.model-menu` — circular, so it computed to almost nothing. The cap is now
   an absolute 340px, with `flex-shrink` handling narrow rows instead. The
   requirement and §3.4 are updated.
2. **The effort label wrapped to two lines under compression**, taking the row
   from 27px to 45px. Only `.model-menu-summary` had `white-space: nowrap`;
   `Extra High` is two words. `nowrap` moved onto `.btn-trigger` itself, and
   `.effort-trigger` now refuses to shrink (`flex: 0 0 auto`) so the model id
   absorbs all compression — which is what requirement 3 actually asks for.
3. **Criterion 10 asserted behaviour that never existed.** Send is not
   disabled on an empty prompt and never was: `#ask-btn` starts disabled and is
   enabled once bootstrap resolves *or* fails (`app.js:236`), gated on
   bootstrap readiness rather than prompt content. An empty submit is refused
   inside `sendChatPrompt` with `Prompt is required`. Criterion rewritten to
   match; no behaviour changed, per `proposal.md` §2.
4. **Criterion 4's 42-character id does not ellipsize.** It measures 312px,
   under the 340px cap, so it renders in full — the generous cap working as
   designed. The truncation path was verified with a 63-character id: capped at
   exactly 340px, ellipsized, full string on `title` and in the popover.

One thing checked and found **not** to be ours: at viewports under ~500px the
page overflows horizontally. The cause is `#chat-status` rendering
`Desktop API unavailable` at 166px in a browser-only session, which stretches
`.thread-header`. In the packaged app that badge reads `Idle`. Confirmed by
neutralising the badge, after which the composer row measures clean at 600px
and 700px with a 42-character model id.

## Baseline

Measured 2026-09-07 in the running app at 1280×800, nothing pinned, empty
prompt. Conversation column 980px wide.

| | Declared in CSS | Measured |
|---|---|---|
| Composer at rest | — | **980×149** |
| `.prompt-box` | — | 852×116 |
| Textarea | `min-height: 111px` | 852×111 |
| ... padding | `14px 58px 56px 14px` | same |
| ... usable text height | 41px expected | **39px** — under two 20.3px rows |
| Right gutter per line | 58px | same |
| Attach button | 38×38 | 38×38 |
| Model pill | `min(220px, 42vw)` × 38 | **220×38**, label box 160px |
| Send button | 38×38 | 38×38 |

Two corrections to `proposal.md` / `context.md` came out of this and are
already applied:

1. **The effort levels are `Default`, `Minimal`, `Low`, `Medium`,
   `Extra High`.** The spec was drafted using a `High` level that does not
   exist. `Extra High` is 60px of label on its own.
2. **The truncation defect is worse than drafted.** `context.md` §1 now carries
   the measured table. `deepseek-v4-flash` overflows at *every* effort level,
   and at `claude-3-5-sonnet-20241022` the effort label is absent rather than
   clipped — the pill renders `claude-3-5-sonnet-202…`.

A third observation, not a defect: the field's usable text height is 39px
against a 20.3px line height, so at rest it shows slightly **under** two full
rows. Task 1 recomputes `min-height` for symmetric padding and should land on
a clean two rows rather than reproducing 39px.

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

## Task 0 · Baseline measurement — Done

**Files:** none — `tasks.md`, `context.md`, `proposal.md` corrections only

- [x] Rebuild and restart the shell at 1280×800.
- [x] Run the measurement snippet above and fill the Baseline table's measured
      column, including composer height at rest with nothing pinned.
- [x] Record the rendered width of `.model-menu-button` with `gpt-4.1`
      selected and with `claude-3-5-sonnet-20241022` selected, and confirm the
      effort truncation from `context.md` §1 actually reproduces. If it does
      not, correct `context.md` §1 rather than leaving the claim standing.
      — **It reproduces, and worse than drafted.** Table corrected with
      measured widths; the `High` level the draft used does not exist.
- [x] Commit the baseline.

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
