# Context: Chrome Density and Hierarchy

Why this spec exists and what the code looks like today. The decision is in
[`proposal.md`](proposal.md); the execution order is in [`tasks.md`](tasks.md).

## 1. Motivation

Specs 41 and 42 built a button scale and applied it to the conversation column.
The sidebar, terminal panel and settings pages inherited the new default step
without ever being designed around it. Three problems remain, all measured
2026-09-04 in the running app at 1280×800.

**`.inline-actions` produces 456px buttons, and spec 41 was wrong about it.**
That spec kept the rule untouched, reasoning that its `1fr 1fr` grid "is
correct in the settings column, it was just being borrowed somewhere it didn't
fit". The settings column is not narrow. Measured widths of its four uses:

| Row | Column width | Result |
|---|---|---|
| General · Load / Save | 220px (capped by `.compact-actions`) | ~106px each — the only correct one |
| MCP · Save Server / Test Connection / Remove | 760px | **Three buttons in a two-column grid** — Remove wraps to its own row |
| Providers · Save Provider / Remove | 760px | 365px each |
| Providers · Save Key / Remove Key | 930px | **456px each** |

The one row that looks right only survives because `.compact-actions` caps it.
The grid was never the fix; the width was.

**Settings has no action hierarchy.** Every button on those pages is the base
step in the same colour. `Save Provider` and `Remove` are indistinguishable,
and so are `Save Key` and `Remove Key`. The destructive ones are gated by
confirm dialogs, so nothing is silently lost — but the user cannot tell which
is which before clicking. Style guide §4's one-primary-per-surface rule is
unapplied here.

**Headings sit outside the documented type scale.** The guide's table runs
14/13/12/11/10 and never mentions headings, yet `.settings-page-title` is 18px,
`.projects-title` is 18px, `.settings-nav-item` is 14px/650 and
`.settings-section h3` is 14px/750. The scale documents about half of the text
in the app.

**The terminal spends 78px of a 220px panel on chrome.** A 44px-minimum tab bar
(51px in practice) holding one tab, a `+`, `Clear` and `×`, plus a 27px working
-directory strip below the output. The terminal body gets 142px — 65% of a
panel whose whole purpose is showing output.

## 2. Current state

| Element | Location | Problem |
|---|---|---|
| `.inline-actions` | `style.css:479` | `grid-template-columns: 1fr 1fr` with no width cap; 365–456px buttons, and a three-button row wraps |
| `.compact-actions` | `style.css:2061` | `width: min(220px, 100%)` — a per-callsite patch for the missing cap |
| `#provider-remove-btn`, `#model-key-delete-btn`, `#mcp-remove-btn` | `index.html` | Destructive, styled identically to Save |
| `#provider-save-btn`, `#mcp-save-btn`, `#model-key-save-btn` | `index.html` | The expected action on their surface, with no primary treatment |
| `.settings-nav-item` | `style.css:1749` | 14px / 650 — a step the scale does not define |
| `.settings-page-title` | `style.css:1933` | 18px / 750, undocumented |
| `.projects-title` | `style.css:294` | 18px / 650 for the word "Projects" |
| `.terminal-tabbar` | `style.css:1190` | `min-height: 44px`, 51px in practice, for a tab and three controls |
| `.terminal-tab span:last-child` | `style.css:1212` | 15px / 650 — above body size for a tab label |
| `.terminal-cwd` | `style.css:1261` | A 27px full-width strip for one line of muted 11px text |

## 3. What this spec does not inherit

Spec 41 §3.1 said `.inline-actions` "keeps its two-column grid". That was based
on an assumption about the settings column's width which §1 above disproves.
This spec supersedes that decision. Spec 41's actual change — stopping the patch
preview from using the rule — remains correct and is untouched.
