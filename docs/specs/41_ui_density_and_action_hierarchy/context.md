# Context: UI Density and Action Hierarchy

Why this spec exists and what the code looks like today. The decision is in
[`proposal.md`](proposal.md); the execution order is in [`tasks.md`](tasks.md).

## 1. Motivation

The desktop shell has exactly one button style. Every `<button>` in the
application resolves to the same rule at
`crates/desktop-shell/static/style.css:184` — 14px text, weight 650, `9px 11px`
padding. That single rule covers the 39 buttons in `index.html` and the 42 more
that `app.js` builds at runtime. A settings `Load`, a patch `Reject Selected`,
and a command `Allow Always` are visually identical.

Two consequences:

- **Nothing communicates consequence.** `Allow Always` writes a persistent
  entry to the user's command allowlist for that repository; `Approve Run`
  executes once. They are the same size, weight, and colour, sitting adjacent.
  Spec 10 added `Allow Always` specifically to reduce approval fatigue, but a
  control that reduces fatigue while being indistinguishable from the one-shot
  control next to it converts fatigue into accidental persistent grants.
- **Everything is bulky.** With no smaller step available, in-conversation
  cards use full-size controls for what is contextual, secondary work.

The bulk is spread across the card rather than concentrated in one place. A
command approval with a one-line command and a one-line rationale measures
**161px** (measured 2026-09-04 in the running app at `127.0.0.1:4899`). Of
that, roughly 45px is a rationale pane that is always rendered whether or not
the user wants it, and the rest is four full-size buttons and their wrap.

Base `pre` also carries `min-height: 180px` (`style.css:25`). This does *not*
currently affect the approval card — approval cards are appended to
`.message-body`, and `.message-body pre` (`style.css:921`) already overrides
the minimum to `0`. The only `pre` outside `.message-body` is `#config-output`,
which wants the tall box. So the declaration is a latent trap rather than an
active cost: it is scoped correctly today by accident, and the next `pre` added
outside `.message-body` inherits 180px of empty space for no reason. This spec
fixes it while the area is open, and claims no pixels for doing so.

## 2. Current state

| Element | Location | Problem |
|---|---|---|
| Global `button` | `style.css:184` | Single style for all 81 buttons; no scale |
| Base `pre` | `style.css:25` | `min-height: 180px`. Neutralised for the approval panes by `.message-body pre`, so it costs nothing today — a latent trap for the next `pre` added outside `.message-body` |
| `.command-approval-details` | `style.css:2134` | Always rendered, whether or not the user wants the rationale; ~45px of the card |
| `.command-approval-command` | `style.css:2123` | `white-space: pre` + `overflow-x: auto` — a long command is one line behind a horizontal scrollbar |
| `.command-approval-actions` | `style.css:2148` | Four equal-weight buttons in a wrapping flex row |
| `.inline-actions` | `style.css:479` | `grid-template-columns: 1fr 1fr`; correct in the settings column, but `createPatchPreview` reuses it in the conversation column where each button stretches to half the chat width |
| `createCommandApprovalPreview` | `app.js:4067` | Builds the four-button row, including both escalating actions |
| `createPatchPreview` | `app.js:3768` | Builds `Apply Selected` / `Reject Selected` into `.inline-actions` |

The diff cards themselves are already dense — 11px pills, 12px monospace diff
lines, 1px line padding (`style.css:2197-2299`) — and are not changed here
beyond the patch-level action row.

## 3. Corrections found during implementation

Recorded here rather than silently fixed, because both were stated as fact in
an earlier draft of this spec and acted on.

- **2026-09-04 — the "180px phantom" does not exist.** The first draft claimed
  base `pre { min-height: 180px }` inflated every command approval pane, and
  that the card measured ~330px. Neither was true: `.message-body pre` already
  overrode the minimum to `0`, and the card measured **161px**. The ~330px
  figure was an estimate taken from a design mockup and written up as though
  measured. §1 and §2 above are the corrected account; the `pre` change is a
  latent-trap cleanup with no visual effect.
- **2026-09-04 — the overflow menu cannot use `.model-popover`.** The first
  draft named it as the base to reuse. It is `position: absolute` anchored to
  the composer, so it would detach from an approval card as the log scrolls.
  The correct base is the `.context-menu-popover` pattern — see
  [`proposal.md`](proposal.md) §3.3.
- **2026-09-04 — `render()` does not run when a file checkbox changes.** The
  task list assumed it did, and wired the patch header's live selection count
  through it. The file checkbox handler only mutates `file.selected`;
  `render()` is reserved for hunk changes and apply/reject, and calling it from
  the checkbox would rebuild the list and drop focus mid-selection. The handler
  calls `updateHeaderCounts()` directly instead.
- **2026-09-04 — `.message-body code` outranks `.command-approval-command`.**
  At `(0,1,1)` against `(0,1,0)` it held the command at 12px with `1px 4px`
  padding, so the first attempt at the 11px wrapped command silently did
  nothing to either. Fixed by qualifying the selector with the element. The
  specimen page caught this by disagreeing with the app, since it renders the
  card outside `.message-body`.

## 4. Superseded by spec 43

- **2026-09-04 — §3.1's decision to leave `.inline-actions` alone was wrong.**
  This spec kept its `1fr 1fr` grid, reasoning that it "is correct in the
  settings column, it was just being borrowed somewhere it didn't fit". The
  settings column is 760-930px wide, so the grid produced 456px buttons there
  and wrapped the MCP page's third button onto its own row. The assumption was
  never measured. [`../43_chrome_density_and_hierarchy/proposal.md`](../43_chrome_density_and_hierarchy/proposal.md)
  §3.1 replaces the grid with a content-sized flex row. This spec's actual
  change — stopping the patch preview from using the rule — stands.
