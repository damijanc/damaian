# Feature Spec: Chrome Density and Hierarchy

Status: Done. All four tasks complete and verified in the running app at
1280×800. Widest settings action button **456px → 129px**, every group on one
row with one primary and its destructive action in `.btn-danger`. Terminal tab
bar **51px → 36px** with the working directory folded into it; terminal body
**142px → 183px**. Every `font-size` in the stylesheet is on the documented
scale bar four glyph buttons, now an explicit exception.
Order: 43 of 43
Also in this spec: [`context.md`](context.md) (motivation, current state, and
the spec 41 decision this supersedes), [`tasks.md`](tasks.md) (execution order
and progress).
Reference: [`../../UI_STYLE_GUIDE.md`](../../UI_STYLE_GUIDE.md) — this spec
completes the scale by documenting its heading steps and adding the danger
variant, then applies the whole thing to the surfaces 41 and 42 did not reach.
Related implementation specs:
[`../41_ui_density_and_action_hierarchy/proposal.md`](../41_ui_density_and_action_hierarchy/proposal.md)
(**supersedes its §3.1 decision to leave `.inline-actions` alone** — see
[`context.md`](context.md) §3),
[`../42_conversation_column_density/proposal.md`](../42_conversation_column_density/proposal.md)
(the shared disclosure and `.visually-hidden` utilities this reuses).

**Not a roadmap graduation.** Last of the three UI specs from the same
usability review as #41 and #42.

## 1. Requirements

1. `.inline-actions` sizes its buttons to their content and does not wrap a
   three-button row. `.compact-actions` becomes unnecessary and is removed.
2. Each settings action group has exactly one primary action, and every
   destructive action is visually distinct from it.
3. A `.btn-danger` variant exists: quiet, `--danger` text, no fill. It signals
   consequence; it does not replace the existing confirm dialogs.
4. The style guide's type table documents every step the app actually uses,
   including headings, and no rule uses a size outside it.
5. The terminal tab bar is at most 36px tall, and the working directory is
   shown within it rather than in its own strip.
6. The terminal body gains at least 35px at the panel's 220px minimum.
7. Every control changed is at least 24×24 CSS px, keeps a visible focus ring,
   and is reachable by keyboard in visual order.
8. The specimen page shows the danger variant and the heading steps.
9. `npm run lint:web` passes clean.

## 2. Non-goals

- **Restructuring settings.** Which pages exist, what they contain and how they
  are navigated is unchanged. This is presentation only.
- **The confirm dialogs.** `.btn-danger` is signalling. Every destructive action
  keeps the dialog it has; none is added or removed.
- **Terminal behaviour.** Sessions, tabs, PTY handling and the `+` button's
  behaviour are untouched. Only the bar's height and the cwd's placement change.
- **A dark theme.** The app remains light-only.
- **The sidebar's structure.** Project rows, session rows and their menus keep
  their layout; only off-scale type is corrected.

## 3. Design

### 3.1 `.inline-actions`

Replace the two-column grid with a content-sized flex row, matching
`.command-approval-actions` and `.patch-actions`:

```css
.inline-actions {
  display: flex;
  flex-wrap: wrap;
  align-items: center;
  gap: 8px;
}
```

Every current caller improves: the three-button MCP row stops wrapping, and the
provider and key rows drop from 365px and 456px to their natural widths.
`.compact-actions` exists only to cap the grid and is deleted along with its one
use in `index.html`.

This supersedes spec 41 §3.1 — see [`context.md`](context.md) §3.

### 3.2 Settings hierarchy

Per style guide §4, one primary per group:

| Group | Primary | Quiet | Danger |
|---|---|---|---|
| General · user configuration | Save | Load | — |
| MCP · server details | Save Server | Test Connection | Remove |
| Providers · provider details | Save Provider | — | Remove |
| Providers · model API key | Save Key | — | Remove Key |

`New` on the two list headings stays at the base step: it sits in a section
heading rather than an action group, and making it primary would put two
primaries on the page.

### 3.3 `.btn-danger`

```css
.btn-danger {
  border-color: transparent;
  background: transparent;
  color: var(--danger);
}

.btn-danger:hover {
  border-color: transparent;
  background: #fbe9e7;
  color: var(--danger);
}
```

Quiet-weight, like `.btn-quiet`, but carrying the danger colour. It marks
consequence without competing with the primary action — a destructive control
should be findable, not loud. `--danger` on `--surface` measures 5.9:1, above
the 4.5:1 floor.

### 3.4 Type scale

The guide's table gains the two heading steps the app already uses, so it
describes all of the text rather than half:

| Size | Weight | Use |
|---|---|---|
| 18px | 750 | Page title — settings pages only, one per page |
| 15px | 750 | Section heading within a page |
| 14px | — | Body copy, message text |
| 13px | 600 | Default control text, card titles |
| 12px | — | Small controls, chips, secondary copy |
| 11px | — | Commands under review, pills, metadata |
| 10px | 800 | Uppercase eyebrow labels only |

Then the strays are brought into it:

- `.settings-nav-item` 14px/650 → 13px/600, matching every other control.
- `.settings-section h3` 14px/750 → 15px/750, taking the section-heading step
  rather than colliding with body size.
- `.projects-title` 18px/650 → 13px/600 with the muted colour it already has.
  It labels a sidebar list; it is not a page title, and 18px made "Projects"
  the largest text in the window.
- `.terminal-tab span:last-child` 15px/650 → 12px/600, a control label.

### 3.5 Terminal chrome

The tab bar drops to `min-height: 36px` with tighter padding, and the tab pill
shrinks with it. The working directory moves into the bar as muted 11px text
after the tab, replacing the separate `.terminal-cwd` strip; it truncates from
the left, so the leaf directory stays visible. `#terminal-cwd` keeps its id and
its update path — only its position and styling change.

Removing the strip's border and padding, and shrinking the bar, returns roughly
40px to the terminal body at the panel's 220px minimum.

## 4. Acceptance criteria

1. No settings action button exceeds 200px. Measured at 1280×800 on the
   Providers and MCP pages. **Met — widest is 129px.**
2. The MCP row renders its three buttons on one line at 1280×800.
3. Each of the four settings action groups shows exactly one `.btn-primary`,
   and every Remove-type action uses `.btn-danger`.
4. Every destructive action still opens its existing confirm dialog, unchanged.
5. No CSS rule in `style.css` sets a `font-size` absent from the guide's type
   table.
6. The terminal tab bar measures at most 36px, `.terminal-cwd` no longer exists
   as a separate strip, and the working directory is visible in the bar.
7. The terminal body measures at least 177px at the panel's 220px minimum, up
   from 142px. **Met at 183px**, with the tab bar at exactly 36px.
8. Every control changed is at least 24×24 CSS px, shows a focus ring, and is
   reachable by keyboard in visual order.
9. `docs/ui-style-guide.html` shows `.btn-danger` and the heading steps, and no
   class it uses is missing from the stylesheet.
10. `npm run lint:web` passes clean.
