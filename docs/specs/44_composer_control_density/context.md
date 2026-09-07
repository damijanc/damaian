# Context: Composer Control Density

Why this spec exists and what the code looks like today. The decision is in
[`proposal.md`](proposal.md); the execution order is in [`tasks.md`](tasks.md).

## 1. Motivation

Specs 41, 42 and 43 came from one usability review of the desktop shell. They
built a button scale, applied it to the conversation column, then to the
sidebar, settings and terminal. The composer was touched once along the way —
#42 grew it from four reserved rows to two — but its controls were never
designed around the scale.

Reported in use: the model selector, the attach button and the send button read
as oversized, and all three sit *inside* the input field rather than beside it.
Both halves of that are literally true.

**Three 38px controls float over the textarea, and the textarea pays for them
in padding.** `.prompt-box textarea` carries `padding: 14px 58px 56px 14px`.
The 56px bottom and 58px right exist for no reason except to stop text running
underneath the controls layered on top of it. Of a 111px field, 70px is
padding; 41px — two rows at 20.3px — is text. Every line of typed text also
stops 58px short of the box's right edge, on every line, whether or not the
cursor is anywhere near the controls.

**The model pill reserves 220px to show a truncated label.** `.model-menu-button`
is `width: min(220px, 42vw)` — a fixed width, not a content size, which guide
§1.4 forbids ("no element reserves space it is not using"). Inside it,
`modelSummaryLabel` builds one string, `` `${model} ${reasoning}` ``, and
`.model-menu-summary` ellipsizes it.

That combination produces a real defect, not just wasted space. Because effort
is appended *last*, a long model id truncates the effort label away:

| Selected model | Label built | Rendered at 220px |
|---|---|---|
| `gpt-4.1` | `gpt-4.1 High` | `gpt-4.1 High` |
| `deepseek-v4-flash` | `deepseek-v4-flash High` | `deepseek-v4-flash Hi…` |
| `claude-3-5-sonnet-20241022` | `claude-3-5-sonnet-20241022 High` | `claude-3-5-sonnet-2…` |

The reasoning level is still in force, still submitted with the request, and no
longer readable. The user has to open the popover to find out what effort they
are running at. Splitting the two triggers fixes this structurally rather than
by widening anything: effort gets its own box and cannot be crowded out.

## 2. Current state

| Element | Location | Problem |
|---|---|---|
| `.prompt-box` | `style.css:1299` | `position: relative` only — a positioning context, not a frame. The textarea draws the border |
| `.prompt-box textarea` | `style.css:1308` | `min-height: 111px` with `padding: 14px 58px 56px 14px`; 70px of the height and 58px of every line reserved for controls placed over it |
| `.attach-menu` | `style.css:1316` | `position: absolute; left: 12px; bottom: 12px` |
| `.attach-menu-button` | `style.css:1323` | 38×38 filled circle for a `+` |
| `.model-menu` | `style.css:1416` | `position: absolute; right: 60px; bottom: 12px` |
| `.model-menu-button` | `style.css:1423` | `width: min(220px, 42vw)`, `height: 38px`, filled pill with a shadow |
| `.model-menu-summary` | `style.css:1455` | Ellipsizes a combined `model + effort` string; effort loses |
| `.send-button` | `style.css:1677` | `position: absolute; right: 12px; bottom: 12px`, 38×38 filled circle |
| `#composer-context` | `index.html:120` | Chip strip docked above the box rather than inside it |
| `openModelMenu` / `closeModelMenu` | `app.js:1763`, `app.js:1770` | Write `aria-expanded` to a single hardcoded trigger id |
| `[data-panel]` binding | `app.js:4941` | Blanket-binds every `[data-panel]` element to `showModelMenuPanel`, which switches panel without opening the popover |
| `@media` composer rules | `style.css:2862` | Three position-based overrides that exist only to keep the absolute layout from colliding below 900px |

## 3. Defects this resolves

Both were found while reading the composer for this spec, not reported:

1. **Effort silently truncated.** §1 above. A setting in force but unreadable.
   Requirement 3 fixes it.
2. **Phantom padding.** 70px vertical and 58px horizontal reserved in the
   textarea so text clears controls drawn over it. This is guide §1.4 and §6
   ("no phantom minimums") violated in the one place the guide had not yet been
   applied — and the `@media` block at `style.css:2862` is the counter-override
   the guide's §6 warns about, three rules whose only job is to stop the
   absolute layout colliding at narrow widths. Requirement 1 fixes it.

Neither is a behaviour bug: the right model, effort and prompt are submitted
throughout. Both are presentation defects.

## 4. Where the design came from

The arrangement — quiet controls in a slim row below the box, chips inside it,
no control over the text — was taken from reference screenshots of another
assistant's composer supplied with the request. Three things in those
screenshots were deliberately **not** adopted, and are recorded in
`proposal.md` §2: a permission-mode control (that is #31), a microphone, and a
send button so quiet it is only a `⏎` hint. The last was considered and
rejected in favour of keeping a real 26px filled button, because damaian's send
button is also its Stop control and Stop must stay findable mid-turn.
