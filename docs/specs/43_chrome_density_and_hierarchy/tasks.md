# Chrome Density and Hierarchy — Task List

**Implements:** [`proposal.md`](proposal.md) · background in [`context.md`](context.md)
**Style reference:** [`../../UI_STYLE_GUIDE.md`](../../UI_STYLE_GUIDE.md) · rendered at `docs/ui-style-guide.html`
**Started:** 2026-09-04

## Progress

| Task | State | Commit | Notes |
|---|---|---|---|
| 1 · `.inline-actions` and settings hierarchy | Done | — | Max button 456→129px, all four groups one row with one primary each. Supersedes spec 41 §3.1 |
| 2 · Type scale | Not started | — | |
| 3 · Terminal chrome | Not started | — | |
| 4 · Specimen page, guide and outcome | Not started | — | |

## Baseline

Measured 2026-09-04, 1280×800:

| | Value |
|---|---|
| Save Key / Remove Key width | 456px each |
| Save Provider / Remove width | 365px each |
| MCP action row | 3 buttons, wraps to 2 rows |
| Terminal tab bar | 51px |
| Terminal cwd strip | 27px |
| Terminal body at 220px panel | 142px |

## How to verify

Settings is a `hidden` section in the same document, so it needs no navigation:

```js
document.querySelector("#settings-shell").hidden = false;
document.querySelector('[data-settings-page="providers"]').click();
```

The terminal panel likewise:

```js
document.querySelector("#terminal-panel").hidden = false;
```

Shell on port 4899, killed by PID, never by name. Static assets are
`include_str!`-embedded — rebuild and restart for every visual change.

---

## Task 1 · `.inline-actions` and settings hierarchy

**Files:** `style.css`, `index.html`

- [ ] Replace `.inline-actions`' grid with the flex row from `proposal.md` §3.1.
- [ ] Delete `.compact-actions` from `style.css` and its use at `index.html:296`.
- [ ] Add `.btn-danger` and its hover from §3.3, after `.btn-quiet`.
- [ ] Apply the §3.2 table: `.btn-primary` on `#config-save-btn`,
      `#mcp-save-btn`, `#provider-save-btn`, `#model-key-save-btn`;
      `.btn-danger` on `#mcp-remove-btn`, `#provider-remove-btn`,
      `#model-key-delete-btn`; `.btn-quiet` on `#config-load-btn` and
      `#mcp-test-btn`.
- [ ] Verify: no settings action button over 200px; the MCP row on one line;
      one primary per group; each Remove still opens its confirm dialog. Lint.
      Commit.

## Task 2 · Type scale

**Files:** `style.css`

- [ ] `.settings-nav-item` → `font-size: 13px; font-weight: 600`.
- [ ] `.settings-section h3` → `font-size: 15px` (keep 750).
- [ ] `.projects-title` → `font-size: 13px; font-weight: 600`.
- [ ] `.terminal-tab span:last-child` → `font-size: 12px; font-weight: 600`.
- [ ] Sweep every `font-size` in `style.css` and list any value outside
      {18, 15, 14, 13, 12, 11, 10}. Fix what is a stray; if a value is load
      bearing — an icon glyph sized in px, for instance — leave it and record
      why in the progress table rather than forcing it.
- [ ] Verify the sidebar and settings still read with a clear hierarchy rather
      than flat. Lint. Commit.

## Task 3 · Terminal chrome

**Files:** `index.html`, `style.css`

- [ ] Move `#terminal-cwd` inside `.terminal-tabbar`, after the tab and before
      `.terminal-spacer`, keeping its id so `app.js` needs no change.
- [ ] `.terminal-tabbar` → `min-height: 36px`, padding `4px 10px`, gap `8px`.
- [ ] `.terminal-tab` → `min-height: 26px`, `border-radius: 999px`, padding
      `0 10px`.
- [ ] Restyle `#terminal-cwd` as inline muted 11px text: no border, no
      background, `min-width: 0`, `direction: rtl` with ellipsis so the leaf
      directory survives truncation.
- [ ] Verify: tab bar ≤ 36px, no separate cwd strip, cwd visible in the bar,
      terminal body ≥ 177px at the 220px minimum, and xterm still fits its
      container without clipping. Lint. Commit.

## Task 4 · Specimen page, guide and outcome

**Files:** `docs/ui-style-guide.html`, `docs/UI_STYLE_GUIDE.md`, spec folder, `docs/specs/README.md`

- [ ] Add `.btn-danger` to the specimen page's button grid, default and
      disabled, and add the two heading steps to its type-scale section.
- [ ] Update the guide: `.btn-danger` in the §3 table, the §3 rule about
      destructive actions rewritten to name it, the §2 type table replaced with
      §3.4's, and `.inline-actions` removed from the §4 anti-pattern note since
      the rule no longer has the flaw the note describes.
- [ ] Correct spec 41: add a note to its `context.md` §3 recording that its
      §3.1 `.inline-actions` decision was superseded, and why.
- [ ] Run the specimen page's missing-class check.
- [ ] Record measurements in `proposal.md`'s status line and set the
      `docs/specs/README.md` row to Done. Commit.
