# UI Density and Action Hierarchy Implementation Plan

**Implements:** [`proposal.md`](proposal.md) · background in [`context.md`](context.md)
**Style reference:** [`../../UI_STYLE_GUIDE.md`](../../UI_STYLE_GUIDE.md) · rendered at `docs/ui-style-guide.html` once Task 2 lands
**Started:** 2026-09-04

## Progress

Update this table as tasks land, so anyone picking the work up mid-flight knows
where it stopped without reading the git log.

| Task | State | Commit | Notes |
|---|---|---|---|
| 1 · Button scale and pre minimum | Done | `928679b` | Verified in the running app: card 161→155px, buttons 13px/600, bare `pre` minimum gone, `#config-output` keeps 180px |
| 2 · Specimen page | Done | `ff4005a` | Verified over HTTP: real stylesheet loads (375 rules), token flip to `#b3261e` propagated. Six classes still unstyled — `command-approval-risk`, `command-approval-disclosure`, `disclosure-caret`, `patch-preview-heading`, `approval-menu-popover`, `approval-menu-row` — all created by Tasks 3-5. `file://` open not verified, see below |
| 3 + 4 · Command approval structure and overflow menu | Done | `c544590` | Landed as one commit: the plan's split needed two no-op stand-in buttons to keep an intermediate state runnable, which is not worth a deliberately-broken commit on `main` |
| 5 · Patch preview header actions | Done | `4fc9f13` | Actions 67px/59px instead of half the column; counts track selection 2→1→0→1; narrow column (740px) ellipsises the summary with actions fully visible and no log overflow |
| 6 · Record outcome in spec | Done | — | Criterion 1 met at 104px after the footer change |

### Corrections

Errors found during execution are recorded in [`context.md`](context.md) §3, so
they sit with the rest of the background rather than in the task list.

### Acceptance criterion 1 — met at 104px

Stacking the disclosure on its own row first gave **138px**, missing the ≤120px
criterion. Rather than shave padding to hide the gap, the two options were put
to the author, who chose to pair the disclosure and actions on one footer row.
That saved 34px: **104px collapsed**, holding at a 740px conversation column.

The expanded card is 162px against the 161px baseline — parity — so the card is
no longer worse in either state. Before the footer change the expanded case had
been 196px, worse than the baseline, because the disclosure row sat on top of
the rationale.

Final: collapsed 161 → 104 (-35%), expanded 161 → 162.

### Open verification

- **`docs/ui-style-guide.html` over `file://` is unverified.** It was checked by
  serving the repo over HTTP (`python3 -m http.server`), which exercises the
  same relative path, but the documented usage is `open docs/ui-style-guide.html`
  straight from a checkout. Loading a relative stylesheet over `file://` is
  ordinary browser behaviour and should work, but acceptance criterion 8 says
  "with no server", so someone should open it once in a real browser and
  confirm the buttons are styled rather than bare.

## How to verify without a model API key

The shell serves HTTP, and `app.js` is a plain script, so its top-level
functions are global. You do not need a live conversation to exercise the
approval or patch cards:

```bash
cargo build -p desktop-shell --bin damaian-desktop-shell
./target/debug/damaian-desktop-shell --port 4899
```

Then load `http://127.0.0.1:4899` and build a synthetic proposal in the console:

```js
window.__mk = (proposal) => {
  document.querySelectorAll(".message").forEach((m) => m.remove());
  const msg = appendChatMessage("assistant", "Synthetic turn for measurement.");
  msg.body.append(createCommandApprovalPreview(proposal, "/tmp/fake-repo"));
  return msg;
};
__mk({
  proposalId: "p-short",
  command: "cargo test -p damaian-core",
  prompt: "Run the core test suite.",
  risk: "review",
  blocked: false,
  allowAlways: true,
  allowBrowserDiagnosticsForSession: false,
});
```

Vary `blocked`, `allowAlways` and `allowBrowserDiagnosticsForSession` to reach
every branch. Port 4899 avoids colliding with a real app instance on the
default 4765; the two share a data directory, so keep this instance read-only.

**Kill it by PID, never by name** — a real Damaian app shares the binary name
and `pkill -f damaian-desktop-shell` would kill the user's app too.

Actually approving or rejecting a synthetic proposal will fail at the network
call, since no such proposal exists server-side. That is expected: use
synthetic cards for layout, measurement and keyboard checks, and a real
conversation for the grant behaviour in Task 4 Step 8.

---

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Give the desktop shell a button size and weight scale, and apply it so that command approval and patch preview cards communicate consequence instead of shouting uniformly.

**Architecture:** Three layers, in order. First a CSS foundation — a button scale plus removal of an inherited `min-height` that silently inflates every approval card. Then a specimen page that renders that foundation from the shipping stylesheet, which becomes the verification surface for everything after it. Then the two conversation-column cards are rebuilt against the scale: command approval gains a wrapped full-text command, a collapsed rationale disclosure, and an overflow menu for grants that outlive the turn; patch preview moves its actions onto the header row.

**Tech Stack:** Vanilla ES modules and hand-written CSS in `crates/desktop-shell/static/`. No framework, no bundler, no JS test runner. Biome 2.5.7 for lint. Tauri v2 shell, built with `cargo tauri`.

## Global Constraints

- **Spec:** [`proposal.md`](proposal.md) and [`context.md`](context.md). **Style guide:** [`../../UI_STYLE_GUIDE.md`](../../UI_STYLE_GUIDE.md). All are authoritative; if the plan disagrees with them, stop and raise it.
- **Static assets are `include_str!`-embedded.** No live reload. Every visual change requires a rebuild and app restart to be visible. An unrebuilt binary shows stale UI and will make you think your change did nothing.
- **Presentation only.** Do not change what requires approval, what may be allowlisted, or where a grant is stored. Specs 10, 12 and 34 own that. Every existing network call, parameter name, and toast string stays exactly as it is.
- **Light theme only.** Do not add `prefers-color-scheme` branches.
- **Tokens, never literals.** Use `var(--accent)` etc. The one permitted new literal is the primary-hover shade `#12564a`, defined in Task 1.
- **Minimum hit target 24×24 CSS px** for every interactive control.
- **Border radius stays 8px** across all button steps. The scale changes size and weight, not shape.
- **Lint:** `npm run lint:web` must pass clean before every commit. It covers `crates/desktop-shell/static/**/*.{js,css}` only — `docs/**` is outside the lint surface and must not be added to it.
- **Commit style:** imperative subject, no trailing period, body explaining why. Do not add a Co-Authored-By trailer unless the repo's recent history shows one.

## A note on testing

This repo has **no JavaScript test harness** — no Jest, no Vitest, no Playwright, and `package.json` defines no test script. `proposal.md` §4 and style guide §9 define verification as: Biome lint, the specimen page, and driving the surface by hand. Do not scaffold a test framework; that is a separate decision and is out of scope here.

So each task below replaces the usual write-a-failing-test step with a **measurement gate** that works the same way:

1. Capture the *current* value first and confirm it is the bad one. This is the "watch it fail" step — if the baseline is already good, your understanding of the problem is wrong and you should stop.
2. Make the change.
3. Re-measure and confirm the target value.

Measurements are exact expressions run in the page console. Use the standalone shell and synthetic proposals described in **How to verify without a model API key** above — it reaches every branch, including `blocked`, without needing a provider key or a real conversation. The one thing it cannot exercise is the server round-trip on approve/reject, which Task 4 Step 8 covers with a real conversation.

---

### Task 1: Button scale and the inherited height minimum

**Files:**
- Modify: `crates/desktop-shell/static/style.css:25-39` (base `pre`)
- Modify: `crates/desktop-shell/static/style.css:184-204` (base `button`, `button:hover`, `button:disabled`)
- Modify: `crates/desktop-shell/static/style.css:921-924` (`.message-body pre`)
- Modify: `crates/desktop-shell/static/style.css:2134-2146` (`.command-approval-details`, `.command-approval-output`)
- Modify: `crates/desktop-shell/static/style.css` (new rules; append the modifier block directly after `button:disabled`)

**Interfaces:**
- Consumes: nothing.
- Produces: CSS classes `.btn-sm`, `.btn-primary`, `.btn-quiet`, `.btn-icon`, applied by Tasks 2, 3, 4 and 5. A `button:focus-visible` rule that all later tasks rely on for requirement 8.

- [x] **Step 1: Measure the baseline**

Run the app (`npm run desktop:dev`), get a command approval card on screen, open the console and run:

```js
({
  detailsHeight: document.querySelector(".command-approval-details").getBoundingClientRect().height,
  buttonFont: getComputedStyle(document.querySelector(".command-approval-actions button")).fontSize,
  buttonWeight: getComputedStyle(document.querySelector(".command-approval-actions button")).fontWeight,
})
```

Expected — the bad baseline: `buttonFont` is `"14px"`, `buttonWeight` is `"650"`, `cardHeight` is around `161`, and `detailsHeight` is around `39`.

**`detailsHeight` is ~39, not 180.** An earlier draft of this plan and of the spec claimed the approval pane inherited `min-height: 180px` from base `pre`. It does not: approval cards are appended to `.message-body`, and `.message-body pre` already overrides the minimum to `0`. Measured 2026-09-04. The `pre` changes in Steps 2-4 are therefore a **latent-trap cleanup with no visual effect** — the only `pre` the base rule actually reaches is `#config-output`, which wants it. Make the change, but claim no pixels for it.

- [x] **Step 2: Remove the inherited minimum from base `pre`**

In `style.css`, the base `pre` rule currently reads:

```css
pre {
  margin: 0;
  min-height: 180px;
  border-radius: 8px;
  background: var(--code);
  color: #e6edf3;
  padding: 14px;
  white-space: pre-wrap;
  word-break: break-word;
  font:
    12px / 1.55 ui-monospace,
    SFMono-Regular,
    Menlo,
    monospace;
}
```

Delete the `min-height: 180px;` line. Leave everything else untouched.

- [x] **Step 3: Give the minimum to the one element that wants it**

`#config-output` is the Effective policy block in Settings › General. It is the only place a tall empty box is intentional, and it currently has no rule of its own. Add one immediately after the base `pre` rule:

```css
/* The Effective policy block is empty until the user loads a config, and a
   collapsed empty box reads as a rendering fault. This is the only place the
   180px floor that used to sit on base `pre` was actually wanted. */
#config-output {
  min-height: 180px;
}
```

- [x] **Step 4: Drop the now-redundant override**

`.message-body pre` only carried `min-height: 0` to undo the base rule. It currently reads:

```css
.message-body pre {
  min-height: 0;
  margin-bottom: 10px;
}
```

Change it to:

```css
.message-body pre {
  margin-bottom: 10px;
}
```

- [x] **Step 5: Tighten the approval panes**

`.command-approval-details, .command-approval-output` currently sets `max-height: 240px`. Change that single declaration to `max-height: 190px`. Do not add a `min-height`. Leave the other declarations in that rule alone.

- [x] **Step 6: Rewrite the base button rule**

Replace the existing `button` rule with:

```css
button {
  min-width: 0;
  border: 1px solid var(--line-strong);
  border-radius: 8px;
  background: var(--surface);
  color: var(--ink);
  font: inherit;
  font-size: 13px;
  font-weight: 600;
  padding: 7px 12px;
  cursor: pointer;
}
```

**Order matters.** `font: inherit` is a shorthand that resets `font-size` and `font-weight`, so both longhands must come after it. The existing rule already relies on this for `font-weight: 650`; keep the same ordering.

- [x] **Step 7: Add the focus ring**

There is currently no focus style for buttons anywhere in the stylesheet — they fall back to the UA outline, which the accent border overrides inconsistently. Requirement 8 needs a real one. Add immediately after `button:hover`:

```css
button:focus-visible {
  border-color: var(--accent);
  box-shadow: 0 0 0 3px var(--accent-soft);
  outline: 0;
}
```

This matches the existing `input:focus` treatment, so the app stays internally consistent.

- [x] **Step 8: Add the modifiers**

Add directly after `button:disabled`:

```css
/* Button scale — see docs/UI_STYLE_GUIDE.md §3. The base rule above is the
   medium step; these are the opt-in variants. At most one `.btn-primary`
   per surface. */
.btn-sm {
  font-size: 12px;
  padding: 5px 10px;
}

.btn-primary {
  border-color: var(--accent);
  background: var(--accent);
  color: #ffffff;
}

/* `button:hover` sets `color: var(--accent)`, which on the accent fill would
   be invisible. These two rules are (0,2,0) against its (0,1,1), so they win
   without `!important`. */
.btn-primary:hover {
  border-color: #12564a;
  background: #12564a;
  color: #ffffff;
}

.btn-quiet {
  border-color: transparent;
  background: transparent;
  color: var(--muted);
}

.btn-quiet:hover {
  border-color: transparent;
  background: var(--surface-soft);
  color: var(--ink);
}

.btn-icon {
  border-color: transparent;
  background: transparent;
  color: var(--muted);
  font-size: 13px;
  padding: 5px 8px;
}

.btn-icon:hover {
  border-color: transparent;
  background: var(--surface-soft);
  color: var(--ink);
}
```

- [x] **Step 9: Rebuild and verify the measurement moved**

Stop the app, run `npm run desktop:dev` again, get a command approval card on screen, and run the Step 1 expression again.

Expected: `buttonFont` is `"13px"`, `buttonWeight` is `"600"`, and `cardHeight` has dropped by roughly 8-12px purely from the smaller buttons. `detailsHeight` is unchanged at ~39 — the `pre` change is inert, as Step 1 explains. The real reduction comes in Task 3, when the rationale moves behind a disclosure.

Then confirm the minimum landed where intended. Open Settings › General without loading a config and run:

```js
document.querySelector("#config-output").getBoundingClientRect().height
```

Expected: `180` or slightly more.

- [x] **Step 10: Confirm nothing else regressed**

Assistant messages contain `pre` blocks for code. Ask the assistant anything that returns a fenced code block, then run:

```js
[...document.querySelectorAll(".message-body pre")].map((el) => el.getBoundingClientRect().height)
```

Expected: heights proportional to their content, none padded out to 180.

- [x] **Step 11: Lint and commit**

```bash
npm run lint:web
```

Expected: clean.

```bash
git add crates/desktop-shell/static/style.css
git commit -m "Add button scale and remove inherited pre height minimum

Base pre carried min-height: 180px, which every inheriting panel paid
silently — most visibly the command approval rationale pane, which
reserved 180px for a one-line prompt. The minimum moves to
#config-output, the only place it was wanted.

The single global button rule becomes the medium step of a real scale,
with .btn-sm, .btn-primary, .btn-quiet and .btn-icon as opt-in
variants, plus the button focus ring the stylesheet never had."
```

---

### Task 2: Specimen page

Built before the cards are rebuilt, on purpose: it gives Tasks 3-5 a single screen showing every control and state, instead of hunting for a proposal that exercises the disabled path.

**Files:**
- Create: `docs/ui-style-guide.html`

**Interfaces:**
- Consumes: `.btn-sm`, `.btn-primary`, `.btn-quiet`, `.btn-icon`, `button:focus-visible` from Task 1.
- Produces: a manual verification surface used by Tasks 3, 4 and 5.

- [ ] **Step 1: Confirm the stylesheet path resolves**

The page lives at `docs/ui-style-guide.html` and the stylesheet at `crates/desktop-shell/static/style.css`, so the relative path is `../crates/desktop-shell/static/style.css`. Verify before writing the page:

```bash
ls -l docs/../crates/desktop-shell/static/style.css
```

Expected: the file listing. If this fails, the layout has changed — stop and re-derive the path.

- [ ] **Step 2: Write the page**

Create `docs/ui-style-guide.html`:

```html
<!doctype html>
<html lang="en">
  <head>
    <meta charset="utf-8" />
    <meta name="viewport" content="width=device-width, initial-scale=1" />
    <title>Damaian UI Style Guide — Specimens</title>
    <!-- The shipping stylesheet, by relative path. This page renders the real
         thing rather than a copy, so it cannot drift. Do not inline any of it. -->
    <link rel="stylesheet" href="../crates/desktop-shell/static/style.css" />
    <style>
      /* Page-local layout ONLY. Nothing here may affect a specimen's own
         appearance — no button, card, or token styling. */
      body {
        margin: 0 auto;
        max-width: 1000px;
        padding: 32px 28px 80px;
      }
      .sg-lede {
        border-left: 3px solid var(--accent);
        padding: 8px 14px;
        margin-bottom: 28px;
      }
      .sg-section {
        margin-top: 34px;
        border-top: 1px solid var(--line);
        padding-top: 18px;
      }
      .sg-hint {
        margin: 4px 0 14px;
      }
      .sg-swatches {
        display: flex;
        flex-wrap: wrap;
        gap: 10px;
      }
      .sg-swatch {
        width: 112px;
      }
      .sg-chip {
        display: block;
        height: 40px;
        border: 1px solid var(--line-strong);
        border-radius: 8px;
      }
      .sg-swatch b {
        display: block;
        font-size: 11px;
        margin-top: 5px;
      }
      .sg-swatch code {
        font-size: 10px;
        color: var(--muted);
      }
      .sg-type-row {
        display: flex;
        align-items: baseline;
        gap: 14px;
        border-bottom: 1px solid var(--line);
        padding: 6px 0;
      }
      .sg-type-row > code {
        width: 46px;
        flex: 0 0 auto;
        font-size: 11px;
        color: var(--muted);
      }
      .sg-type-row > em {
        margin-left: auto;
        font-style: normal;
        font-size: 11px;
        color: var(--muted);
      }
      .sg-grid {
        display: grid;
        grid-template-columns: 130px 1fr;
        gap: 12px 16px;
        align-items: center;
      }
      .sg-rowlabel {
        font-size: 11px;
        font-weight: 800;
        letter-spacing: 0.05em;
        text-transform: uppercase;
        color: var(--muted);
      }
      .sg-row {
        display: flex;
        align-items: center;
        gap: 10px;
        flex-wrap: wrap;
      }
      .sg-card {
        max-width: 520px;
        border: 1px solid var(--line-strong);
        border-radius: 10px;
        background: var(--surface);
        padding: 13px 15px;
      }
      .sg-pair {
        display: flex;
        gap: 18px;
        flex-wrap: wrap;
      }
    </style>
  </head>
  <body>
    <h1>Damaian UI specimens</h1>
    <div class="sg-lede">
      <p>
        Rendered from <code>crates/desktop-shell/static/style.css</code> by relative path — this
        page shows the shipping stylesheet, not a copy of it. Rules and rationale live in
        <a href="UI_STYLE_GUIDE.md">UI_STYLE_GUIDE.md</a>; change one and change the other in the
        same commit.
      </p>
      <p>
        <strong>States are live.</strong> Hover the buttons and press Tab through them to see hover
        and focus. The disabled examples carry a real <code>disabled</code> attribute. Nothing here
        is simulated with page-local classes, because that is exactly the drift this page exists to
        prevent.
      </p>
    </div>

    <section class="sg-section">
      <h2>Colour tokens</h2>
      <p class="sg-hint">Edit a token in the stylesheet and reload — these follow it.</p>
      <div class="sg-swatches">
        <div class="sg-swatch"><span class="sg-chip" style="background: var(--bg)"></span><b>--bg</b><code>window</code></div>
        <div class="sg-swatch"><span class="sg-chip" style="background: var(--surface)"></span><b>--surface</b><code>cards</code></div>
        <div class="sg-swatch"><span class="sg-chip" style="background: var(--surface-soft)"></span><b>--surface-soft</b><code>recessed</code></div>
        <div class="sg-swatch"><span class="sg-chip" style="background: var(--sidebar)"></span><b>--sidebar</b><code>sidebar</code></div>
        <div class="sg-swatch"><span class="sg-chip" style="background: var(--ink)"></span><b>--ink</b><code>text</code></div>
        <div class="sg-swatch"><span class="sg-chip" style="background: var(--muted)"></span><b>--muted</b><code>secondary</code></div>
        <div class="sg-swatch"><span class="sg-chip" style="background: var(--line)"></span><b>--line</b><code>dividers</code></div>
        <div class="sg-swatch"><span class="sg-chip" style="background: var(--line-strong)"></span><b>--line-strong</b><code>borders</code></div>
        <div class="sg-swatch"><span class="sg-chip" style="background: var(--accent)"></span><b>--accent</b><code>primary</code></div>
        <div class="sg-swatch"><span class="sg-chip" style="background: var(--accent-soft)"></span><b>--accent-soft</b><code>focus ring</code></div>
        <div class="sg-swatch"><span class="sg-chip" style="background: var(--ok)"></span><b>--ok</b><code>state</code></div>
        <div class="sg-swatch"><span class="sg-chip" style="background: var(--warn)"></span><b>--warn</b><code>state</code></div>
        <div class="sg-swatch"><span class="sg-chip" style="background: var(--danger)"></span><b>--danger</b><code>state</code></div>
        <div class="sg-swatch"><span class="sg-chip" style="background: var(--code)"></span><b>--code</b><code>code bg</code></div>
      </div>
    </section>

    <section class="sg-section">
      <h2>Type scale</h2>
      <div class="sg-type-row"><code>14px</code><span style="font-size: 14px">Body copy and message text</span><em>message body</em></div>
      <div class="sg-type-row"><code>13px</code><span style="font-size: 13px">Default control text and card titles</span><em>buttons, headers</em></div>
      <div class="sg-type-row"><code>12px</code><span style="font-size: 12px">Small controls, chips, secondary copy</span><em>.btn-sm, chips</em></div>
      <div class="sg-type-row"><code>11px</code><span style="font-size: 11px">Commands under review, pills, metadata</span><em>approval command</em></div>
      <div class="sg-type-row"><code>10px</code><span style="font-size: 10px; font-weight: 800; letter-spacing: 0.06em; text-transform: uppercase">Eyebrow labels only</span><em>uppercase only</em></div>
    </section>

    <section class="sg-section">
      <h2>Buttons</h2>
      <p class="sg-hint">
        Hover and Tab through each row. Every control here is at least 24×24 CSS px.
      </p>
      <div class="sg-grid">
        <span class="sg-rowlabel">base · 13px</span>
        <span class="sg-row">
          <button type="button">Save Server</button>
          <button type="button" disabled>Disabled</button>
        </span>

        <span class="sg-rowlabel">.btn-sm</span>
        <span class="sg-row">
          <button type="button" class="btn-sm">Rollback</button>
          <button type="button" class="btn-sm" disabled>Disabled</button>
        </span>

        <span class="sg-rowlabel">.btn-primary</span>
        <span class="sg-row">
          <button type="button" class="btn-sm btn-primary">Approve</button>
          <button type="button" class="btn-sm btn-primary" disabled>Blocked</button>
        </span>

        <span class="sg-rowlabel">.btn-quiet</span>
        <span class="sg-row">
          <button type="button" class="btn-sm btn-quiet">Reject</button>
          <button type="button" class="btn-sm btn-quiet" disabled>Disabled</button>
        </span>

        <span class="sg-rowlabel">.btn-icon</span>
        <span class="sg-row">
          <button type="button" class="btn-icon" aria-label="More actions">⋯</button>
          <button type="button" class="btn-icon" aria-label="More actions" disabled>⋯</button>
        </span>
      </div>
    </section>

    <section class="sg-section">
      <h2>Action hierarchy</h2>
      <p class="sg-hint">
        One primary, one quiet, everything that outlives the turn behind the overflow.
      </p>
      <div class="sg-row">
        <button type="button" class="btn-sm btn-primary">Approve</button>
        <button type="button" class="btn-sm btn-quiet">Reject</button>
        <button type="button" class="btn-icon" aria-label="More actions">⋯</button>
      </div>
    </section>

    <section class="sg-section">
      <h2>Command approval</h2>
      <p class="sg-hint">Collapsed left, expanded with a long wrapped command right.</p>
      <div class="sg-pair">
        <div class="sg-card">
          <div class="command-approval-header">
            <strong>Run command</strong><span class="command-approval-risk">review</span>
          </div>
          <code class="command-approval-command">cargo test -p damaian-core</code>
          <button type="button" class="command-approval-disclosure" aria-expanded="false">
            <span class="disclosure-caret" aria-hidden="true"></span> Why this command
          </button>
          <div class="inline-actions command-approval-actions">
            <button type="button" class="btn-sm btn-primary">Approve</button>
            <button type="button" class="btn-sm btn-quiet">Reject</button>
            <button type="button" class="btn-icon" aria-label="More actions">⋯</button>
          </div>
        </div>
        <div class="sg-card">
          <div class="command-approval-header">
            <strong>Run command</strong><span class="command-approval-risk">review</span>
          </div>
          <code class="command-approval-command">docker run --rm -v "$PWD":/w -w /w --network host -e RUST_LOG=debug rust:1.79 cargo build --release --target x86_64-unknown-linux-musl</code>
          <button type="button" class="command-approval-disclosure" aria-expanded="true">
            <span class="disclosure-caret" aria-hidden="true"></span> Why this command
          </button>
          <pre class="command-approval-details">Cross-compiling for musl so the release artifact runs on the CI image.</pre>
          <div class="inline-actions command-approval-actions">
            <button type="button" class="btn-sm btn-primary">Approve</button>
            <button type="button" class="btn-sm btn-quiet">Reject</button>
            <button type="button" class="btn-icon" aria-label="More actions">⋯</button>
          </div>
        </div>
      </div>
    </section>

    <section class="sg-section">
      <h2>Patch header</h2>
      <div class="sg-card">
        <div class="patch-preview-header">
          <strong>Add retry on 503</strong>
          <span>2 files · +34 −6</span>
          <div class="patch-actions">
            <button type="button" class="btn-sm btn-primary">Apply 2</button>
            <button type="button" class="btn-sm btn-quiet">Reject</button>
          </div>
        </div>
      </div>
    </section>
  </body>
</html>
```

Note: `.command-approval-risk`, `.command-approval-disclosure`, `.disclosure-caret` and the revised `.patch-actions` do not exist yet — Tasks 3, 4 and 5 create them. Until then those specimens render unstyled. That is expected and is corrected by Step 3 of Tasks 3 and 5.

- [ ] **Step 3: Open it and verify it is not a copy**

```bash
open docs/ui-style-guide.html
```

Check: the button rows render with Task 1's scale, hovering changes them, Tab shows the focus ring, and the disabled examples do not respond.

Then prove it is loading the real stylesheet. Temporarily change `--accent` in `style.css` to `#b3261e`, reload the page, and confirm the primary buttons and focus rings turn red. **Revert the token immediately** and reload to confirm they return to green.

- [ ] **Step 4: Confirm it is outside the lint surface**

```bash
npm run lint:web
```

Expected: clean, and the output does not mention `docs/ui-style-guide.html`. `biome.json` scopes `files.includes` to `crates/desktop-shell/static/**/*.{js,css}` and `scripts/**/*.mjs`. Do not add the page to that list.

- [ ] **Step 5: Commit**

```bash
git add docs/ui-style-guide.html
git commit -m "Add rendered specimen page for the UI style guide

Loads the shipping stylesheet by relative path rather than copying it,
so the page cannot misrepresent the app. Button states are live
controls the reader hovers and tabs through — simulating them with
page-local classes would reintroduce the drift the page prevents.

Doubles as the manual verification surface for the card work that
follows."
```

---

### Task 3: Command approval — structure, wrapped command, disclosure

**Files:**
- Modify: `crates/desktop-shell/static/app.js:4067-4118` (`createCommandApprovalPreview`, construction only)
- Modify: `crates/desktop-shell/static/app.js:4248` (the `wrapper.append(...)` call)
- Modify: `crates/desktop-shell/static/style.css:2116-2156` (`.command-approval-*`)
- Modify: `docs/ui-style-guide.html` (specimen now styled)

**Interfaces:**
- Consumes: `.btn-sm`, `.btn-primary`, `.btn-quiet` from Task 1.
- Produces: inside `createCommandApprovalPreview`, the local bindings `runButton`, `rejectButton`, `details`, `output` keep their existing names and roles so Task 4 can extend the same function. New CSS classes `.command-approval-risk`, `.command-approval-disclosure`, `.disclosure-caret` used by the specimen page.

**Do not touch** `resolveCommandProposal`, `restoreActions`, or any `addEventListener` body in this task. Their behaviour is settled by specs 10 and 12. Task 4 changes which elements `restoreActions` re-enables and nothing else.

- [ ] **Step 1: Measure the baseline**

With a command approval card on screen:

```js
(() => {
  const card = document.querySelector(".command-approval");
  const cmd = card.querySelector(".command-approval-command");
  return {
    cardHeight: card.getBoundingClientRect().height,
    commandFont: getComputedStyle(cmd).fontSize,
    commandWrap: getComputedStyle(cmd).whiteSpace,
    commandOverflowX: getComputedStyle(cmd).overflowX,
    detailsVisible: !!card.querySelector(".command-approval-details"),
  };
})()
```

Expected — the bad baseline: `commandFont` is `"12px"`, `commandWrap` is `"pre"`, `commandOverflowX` is `"auto"`, `detailsVisible` is `true` (the rationale is always rendered), and `cardHeight` reflects that.

- [ ] **Step 2: Restructure the card in `app.js`**

In `createCommandApprovalPreview`, replace the block from `const header = document.createElement("div");` through `actions.append(rejectButton);` with:

```js
  const header = document.createElement("div");
  header.className = "command-approval-header";
  const title = document.createElement("strong");
  title.textContent = isBrowserDiagnostic ? "Run browser diagnostic" : "Run command";
  const meta = document.createElement("span");
  meta.className = "command-approval-risk";
  meta.textContent = proposal.blocked ? "blocked" : proposal.risk || "review";
  meta.dataset.blocked = proposal.blocked ? "true" : "false";
  header.append(title, meta);

  const command = document.createElement("code");
  command.className = "command-approval-command";
  command.textContent = proposal.command || "";

  // The rationale is behind a disclosure because the common case is a short
  // command approved without reading it. Rendered collapsed every time — the
  // state is deliberately not remembered between proposals.
  const detailsText = proposal.prompt || "";
  const disclosure = document.createElement("button");
  disclosure.type = "button";
  disclosure.className = "command-approval-disclosure";
  disclosure.setAttribute("aria-expanded", "false");
  const caret = document.createElement("span");
  caret.className = "disclosure-caret";
  caret.setAttribute("aria-hidden", "true");
  disclosure.append(caret, document.createTextNode("Why this command"));

  const details = document.createElement("pre");
  details.className = "command-approval-details";
  details.textContent = detailsText;
  details.hidden = true;

  disclosure.addEventListener("click", () => {
    const open = disclosure.getAttribute("aria-expanded") === "true";
    disclosure.setAttribute("aria-expanded", open ? "false" : "true");
    details.hidden = open;
  });

  const actions = document.createElement("div");
  actions.className = "command-approval-actions";
  const runButton = document.createElement("button");
  runButton.type = "button";
  runButton.className = "btn-sm btn-primary";
  runButton.textContent = proposal.blocked ? "Blocked" : "Approve";
  runButton.disabled = Boolean(proposal.blocked);
  const rejectButton = document.createElement("button");
  rejectButton.type = "button";
  rejectButton.className = "btn-sm btn-quiet";
  rejectButton.textContent = "Reject";
  actions.append(runButton, rejectButton);
```

Notes on what changed and why:

- The title is now a verb phrase, matching the specimen page.
- `.inline-actions` is gone from the actions row. Style guide §4 forbids it outside the settings column.
- `alwaysButton` and `browserSessionButton` are **deleted here** and rebuilt as menu items in Task 4. Between this task and that one the two grants are unreachable — that is expected, and Task 4 restores them. Do not ship Task 3 alone.
- `details.hidden = true` uses the `hidden` property, matching the pattern elsewhere in the file.

- [ ] **Step 3: Temporarily neutralise the dead references**

`restoreActions` and `resolveCommandProposal` still reference `alwaysButton` and `browserSessionButton`, which no longer exist. Task 4 removes those references properly. To keep this task independently runnable, add immediately after the `actions.append(runButton, rejectButton);` line:

```js
  // Replaced by real menu items in Task 4 of the plan. These no-op stand-ins
  // keep `resolveCommandProposal` and `restoreActions` working unchanged in
  // the meantime; both are removed in that task.
  const alwaysButton = document.createElement("button");
  const browserSessionButton = document.createElement("button");
```

- [ ] **Step 4: Update the append call**

At the end of the function, `wrapper.append(header, command, details, actions, output);` becomes:

```js
  wrapper.append(header, command, disclosure, details, actions, output);
```

- [ ] **Step 5: Rewrite the card CSS**

Replace the `.command-approval-header span` rule with a `.command-approval-risk` rule, and update the command and actions rules. The full replacement for the block from `.command-approval-header span` through `.command-approval-actions button`:

```css
.command-approval-risk {
  flex: 0 0 auto;
  border: 1px solid #e3cba8;
  border-radius: 999px;
  background: #fdf3e3;
  color: var(--warn);
  font-size: 10px;
  font-weight: 800;
  letter-spacing: 0.04em;
  padding: 1px 7px;
  text-transform: uppercase;
}

.command-approval-risk[data-blocked="true"] {
  border-color: #d89999;
  background: #fbe9e7;
  color: var(--danger);
}

/* Wrapped and uncapped, deliberately. This is a consent surface: the user
   must be able to read the whole command without interacting first, so it is
   never horizontally scrolled, elided, or capped. See UI_STYLE_GUIDE.md §5. */
.command-approval-command {
  display: block;
  border: 1px solid var(--line);
  border-radius: 8px;
  background: #f6f7f7;
  color: var(--ink);
  font-size: 11px;
  line-height: 1.5;
  padding: 7px 9px;
  overflow-wrap: anywhere;
  white-space: pre-wrap;
}

.command-approval-disclosure {
  display: inline-flex;
  align-items: center;
  gap: 5px;
  border: 0;
  border-radius: 6px;
  background: none;
  color: var(--muted);
  font-size: 12px;
  font-weight: 600;
  padding: 3px 4px;
  justify-self: start;
}

.command-approval-disclosure:hover {
  border: 0;
  color: var(--ink);
}

.disclosure-caret {
  width: 6px;
  height: 6px;
  flex: 0 0 auto;
  border-right: 1.5px solid currentColor;
  border-bottom: 1.5px solid currentColor;
  transform: rotate(-45deg);
}

.command-approval-disclosure[aria-expanded="true"] .disclosure-caret {
  transform: rotate(45deg);
}

.command-approval-actions {
  display: flex;
  align-items: center;
  flex-wrap: wrap;
  gap: 6px;
  justify-content: flex-start;
}
```

Two things to be careful about. `.command-approval` is `display: grid`, so `justify-self: start` on the disclosure is what stops it stretching the full card width. And `overflow-wrap: anywhere` rather than `word-break: break-all` — it breaks only where a line would otherwise overflow, keeping short flags and paths intact.

Also delete `min-width: 0` from the now-removed `.command-approval-actions button` rule; the buttons size to content and no longer need it.

- [ ] **Step 6: Rebuild and re-measure**

Restart the app, get an approval on screen, and run the Step 1 expression.

Expected: `commandFont` is `"11px"`, `commandWrap` is `"pre-wrap"`, `commandOverflowX` is `"visible"`, and `cardHeight` is at or under 120 for a short command with the rationale collapsed — down from the measured 161 baseline. Record the exact `cardHeight` — acceptance criterion 1 needs it, and if it exceeds 120 report the number rather than adjusting padding to hit the target.

- [ ] **Step 7: Verify the disclosure and the long-command case**

Click "Why this command": the rationale appears, the caret rotates, `aria-expanded` flips to `"true"`. Click again: it collapses. Reject the proposal, trigger another, and confirm the new card renders collapsed.

Then ask the assistant to run something long, for example a `docker run` with several flags, and check:

```js
(() => {
  const cmd = document.querySelector(".command-approval-command");
  return {
    scrollsHorizontally: cmd.scrollWidth > cmd.clientWidth,
    scrollsVertically: cmd.scrollHeight > cmd.clientHeight,
    fullTextRendered: cmd.textContent.length > 0,
  };
})()
```

Expected: both scroll checks are `false`. The whole command is visible. This is acceptance criterion 3.

- [ ] **Step 8: Check the specimen page caught up**

Reload `docs/ui-style-guide.html`. The two approval cards now render with the risk pill, the 11px wrapped command, and the disclosure row styled. The long command in the right-hand specimen wraps rather than scrolling.

- [ ] **Step 9: Lint and commit**

```bash
npm run lint:web
```

Expected: clean. If Biome flags the unused `alwaysButton` / `browserSessionButton` stand-ins, leave them — Task 4 deletes them — and note the warning in your handoff rather than suppressing it.

```bash
git add crates/desktop-shell/static/app.js crates/desktop-shell/static/style.css
git commit -m "Restructure command approval card

The command is now 11px and wraps in full: on a consent surface the
user must be able to read the whole thing without interacting, so it
is never horizontally scrolled or capped. The rationale moves behind a
collapsed disclosure, rendered collapsed every time.

Approve becomes the single primary action and Reject goes quiet. The
two persistent grants are temporarily unreachable and are restored as
overflow menu items in the next commit."
```

---

### Task 4: Command approval — overflow menu for persistent grants

**Files:**
- Modify: `crates/desktop-shell/static/app.js` (new shared menu near `ensureProjectMenu`, around line 590)
- Modify: `crates/desktop-shell/static/app.js:4067+` (`createCommandApprovalPreview`: trigger, `restoreActions`, remove the Task 3 stand-ins and the two old listeners)
- Modify: `crates/desktop-shell/static/app.js:706-708` (global dismissal)
- Modify: `crates/desktop-shell/static/style.css` (one rule)

**Interfaces:**
- Consumes: `.btn-icon` from Task 1; `runButton`, `rejectButton`, `resolveCommandProposal`, `restoreActions` from Task 3.
- Produces: `ensureApprovalMenu()`, `toggleApprovalMenu(items, anchorEl)`, `closeApprovalMenu()`. `toggleApprovalMenu` takes `items` as an array of `{ label, hint, onSelect }` and an anchor `HTMLElement`.

The pattern to copy is `ensureProjectMenu` / `positionProjectMenu` / `closeProjectMenu` at `app.js:594-658`. Read those first. The reason for a `position: fixed` popover on `document.body` rather than one nested in the card is the same reason given there: approval cards live inside the scrolling `#chat-log`, and a CSS-anchored popover would detach on scroll.

- [ ] **Step 1: Confirm the grants are currently unreachable**

With an approval card on screen that offers "always" (a repeatable project command such as a test run):

```js
[...document.querySelectorAll(".command-approval-actions button")].map((b) => b.textContent)
```

Expected after Task 3: `["Approve", "Reject"]`. The two grants are gone. That is the gap this task closes.

- [ ] **Step 2: Add the shared menu**

Add after `closeProjectMenu` (around line 658):

```js
// The approval overflow menu. Same shape and rationale as the project row
// menu above: one shared `position: fixed` popover on `document.body`,
// anchored to the trigger's rect. Approval cards live inside the scrolling
// `#chat-log` and there can be more than one, so a popover anchored by CSS
// to an ancestor would detach as the log scrolls.
let approvalMenuEl = null;

function ensureApprovalMenu() {
  if (approvalMenuEl) return approvalMenuEl;
  const el = document.createElement("div");
  el.className = "context-menu-popover approval-menu-popover";
  el.setAttribute("role", "menu");
  el.hidden = true;
  el.addEventListener("click", (event) => event.stopPropagation());
  document.body.append(el);
  approvalMenuEl = el;
  return el;
}

// `items` is [{ label, hint, onSelect }]. Rebuilt on every open because the
// available grants differ per proposal.
function toggleApprovalMenu(items, anchorEl) {
  const el = ensureApprovalMenu();
  if (!el.hidden && el.dataset.anchorId === anchorEl.dataset.menuId) {
    closeApprovalMenu();
    return;
  }
  el.innerHTML = "";
  const panel = document.createElement("div");
  panel.className = "context-menu-panel";
  items.forEach((item) => {
    const row = document.createElement("button");
    row.type = "button";
    row.className = "context-menu-row approval-menu-row";
    row.setAttribute("role", "menuitem");
    const label = document.createElement("span");
    label.textContent = item.label;
    row.append(label);
    if (item.hint) {
      const hint = document.createElement("small");
      hint.textContent = item.hint;
      row.append(hint);
    }
    row.addEventListener("click", () => {
      closeApprovalMenu();
      item.onSelect();
    });
    panel.append(row);
  });
  el.append(panel);
  el.dataset.anchorId = anchorEl.dataset.menuId || "";
  el.hidden = false;
  positionApprovalMenu(anchorEl);
}

function positionApprovalMenu(anchorEl) {
  const el = ensureApprovalMenu();
  const rect = anchorEl.getBoundingClientRect();
  const width = el.offsetWidth || 264;
  const left = Math.min(Math.max(8, rect.left), window.innerWidth - width - 8);
  const top = Math.min(rect.bottom + 4, window.innerHeight - el.offsetHeight - 8);
  el.style.left = `${left}px`;
  el.style.top = `${top}px`;
}

function closeApprovalMenu() {
  if (!approvalMenuEl) return;
  approvalMenuEl.hidden = true;
  approvalMenuEl.dataset.anchorId = "";
}
```

- [ ] **Step 3: Register with the existing dismissal**

The document click and Escape handlers at `app.js:706-708` currently read:

```js
document.addEventListener("click", () => closeProjectMenu());
document.addEventListener("keydown", (event) => {
  if (event.key === "Escape") closeProjectMenu();
});
```

Change them to close both menus:

```js
document.addEventListener("click", () => {
  closeProjectMenu();
  closeApprovalMenu();
});
document.addEventListener("keydown", (event) => {
  if (event.key === "Escape") {
    closeProjectMenu();
    closeApprovalMenu();
  }
});
```

If the surrounding lines differ from the above, adapt rather than overwrite — the requirement is that both close, not the exact shape.

- [ ] **Step 4: Build the trigger**

In `createCommandApprovalPreview`, delete the two no-op stand-ins added in Task 3 Step 3, then add after `actions.append(runButton, rejectButton);`:

```js
  // Grants that outlive this turn live behind the overflow, never at the same
  // weight as Approve — the cost of a mis-click is not symmetric. See
  // UI_STYLE_GUIDE.md §4.
  const menuItems = [];
  if (proposal.allowAlways) {
    menuItems.push({
      label: "Always allow in this project",
      hint: "Adds to the allowlist — persists after this turn",
      onSelect: () => runWithGrant({ always: true }, "Command allowed for this project"),
    });
  }
  if (proposal.allowBrowserDiagnosticsForSession) {
    menuItems.push({
      label: "Allow for this session",
      hint: "Browser diagnostics only, until this session ends",
      onSelect: () =>
        runWithGrant(
          { allowBrowserDiagnosticsForSession: true },
          "Browser diagnostics allowed for this session",
        ),
    });
  }

  let overflowButton = null;
  if (menuItems.length && !proposal.blocked) {
    overflowButton = document.createElement("button");
    overflowButton.type = "button";
    overflowButton.className = "btn-icon";
    overflowButton.dataset.menuId = `approval-${proposal.proposalId}`;
    overflowButton.setAttribute("aria-haspopup", "menu");
    overflowButton.setAttribute("aria-label", "More approval options");
    overflowButton.title = "More approval options";
    overflowButton.textContent = "⋯";
    overflowButton.addEventListener("click", (event) => {
      event.stopPropagation();
      toggleApprovalMenu(menuItems, overflowButton);
    });
    actions.append(overflowButton);
  }
```

`event.stopPropagation()` is required — without it the document click handler from Step 3 closes the menu in the same tick it opens.

A blocked proposal gets no trigger at all, which is acceptance criterion 5: it cannot be approved through the menu because the menu does not exist.

- [ ] **Step 5: Add the shared grant handler**

Add immediately before `runButton.addEventListener(...)`. This replaces the two deleted listeners; their bodies were identical apart from the options and the toast.

```js
  async function runWithGrant(options, successToast) {
    try {
      await resolveCommandProposal(true, options);
      toast(successToast);
    } catch (error) {
      restoreActions();
      output.textContent = error.message;
      toast(error.message);
    }
  }
```

Then delete the `alwaysButton.addEventListener(...)` and `browserSessionButton.addEventListener(...)` blocks entirely.

- [ ] **Step 6: Fix `restoreActions` and the disable sweep**

`restoreActions` currently re-enables four buttons, two of which no longer exist. Replace it with:

```js
  function restoreActions() {
    runButton.disabled = Boolean(proposal.blocked);
    rejectButton.disabled = false;
    if (overflowButton) overflowButton.disabled = false;
  }
```

In `resolveCommandProposal`, the four disable lines at the top become:

```js
    runButton.disabled = true;
    rejectButton.disabled = true;
    if (overflowButton) overflowButton.disabled = true;
    closeApprovalMenu();
```

Everything else in `resolveCommandProposal` — the streaming, the resume parameters, the output text branches, the `loadSessions` call — stays exactly as it is.

Note the ordering hazard: `overflowButton` is declared with `let` at Step 4, which runs before these functions are *called* but after they are *defined*. That is fine because both are function declarations reading the binding at call time. Do not convert them to arrow functions assigned before Step 4's block.

- [ ] **Step 7: Style the menu rows**

The `.context-menu-popover` and `.context-menu-row` rules already carry the popover chrome. Add only the two-line row treatment, after the existing `.context-menu-row-danger` rule:

```css
.approval-menu-popover {
  width: min(272px, calc(100vw - 32px));
}

.approval-menu-row {
  display: grid;
  gap: 1px;
  text-align: left;
}

.approval-menu-row small {
  color: var(--muted);
  font-size: 10.5px;
  font-weight: 400;
}
```

- [ ] **Step 8: Rebuild and verify both grants**

Restart. Get an approval offering "always" and check:

```js
[...document.querySelectorAll(".command-approval-actions button")].map((b) => b.textContent)
```

Expected: `["Approve", "Reject", "⋯"]`.

Click `⋯`: the menu opens below it. **Scroll the chat log while it is open** — it must stay anchored to the button, which is the whole reason for the fixed-position pattern. Press Escape: it closes. Reopen, click elsewhere: it closes.

Then exercise each grant for real:

- Choose "Always allow in this project". Expected: the command runs, the toast reads `Command allowed for this project`, and the same command in a later turn no longer prompts. This is spec 10's behaviour, unchanged.
- For a browser diagnostic proposal, choose "Allow for this session". Expected: the toast reads `Browser diagnostics allowed for this session` and later diagnostics in that session do not prompt. This is spec 12's behaviour, unchanged.

- [ ] **Step 9: Verify the blocked path**

Get a blocked proposal on screen — a command the policy refuses, such as one using shell control syntax. Check:

```js
(() => {
  const actions = document.querySelector(".command-approval-actions");
  return {
    labels: [...actions.querySelectorAll("button")].map((b) => b.textContent),
    approveDisabled: actions.querySelector(".btn-primary").disabled,
    hasOverflow: !!actions.querySelector(".btn-icon"),
  };
})()
```

Expected: `approveDisabled` is `true`, `hasOverflow` is `false`, and the risk pill reads `blocked` in the danger colour. Acceptance criterion 5.

- [ ] **Step 10: Verify keyboard access**

Tab from the command into the card. Expected order: disclosure → Approve → Reject → `⋯`. Every stop shows the focus ring from Task 1. Focus must not be on Approve when the card first renders — click into the chat log and Tab in to confirm. Acceptance criterion 7, and requirement 8.

- [ ] **Step 11: Lint and commit**

```bash
npm run lint:web
```

Expected: clean, including the stand-in warnings from Task 3, which are now gone.

```bash
git add crates/desktop-shell/static/app.js crates/desktop-shell/static/style.css
git commit -m "Move persistent approval grants behind an overflow menu

Always-allow and the session-wide browser diagnostic grant outlive the
turn, so they no longer sit at the same visual weight as the one-shot
Approve next to it — the cost of a mis-click is not symmetric.

Follows the project row menu pattern: one shared fixed-position
popover anchored to the trigger's rect, because approval cards live in
the scrolling log. A blocked proposal renders no trigger, so it cannot
be approved through the menu. Grant behaviour from specs 10 and 12 is
unchanged."
```

---

### Task 5: Patch preview header actions

**Files:**
- Modify: `crates/desktop-shell/static/app.js:3790-3816` (`createPatchPreview` header and actions)
- Modify: `crates/desktop-shell/static/app.js` (`render()` inside `createPatchPreview`, to keep the count live)
- Modify: `crates/desktop-shell/static/style.css:781-805` (`.patch-preview-header`, `.patch-actions`)
- Modify: `docs/ui-style-guide.html` (specimen now styled)

**Interfaces:**
- Consumes: `.btn-sm`, `.btn-primary`, `.btn-quiet` from Task 1.
- Produces: nothing consumed by later tasks.

**Read the comment at `style.css:773-780` before you start.** It explains that `min-width: 0` on `.patch-preview-header` is load-bearing: both children are `white-space: nowrap`, so without it the flex row's automatic minimum is the untruncated text width, which propagates up and forces the whole conversation column to overflow. Adding buttons to this row means the *text* group must keep `min-width: 0` and the *button* group must be `flex: 0 0 auto`. Getting this wrong reintroduces a horizontal scrollbar on the chat log.

- [ ] **Step 1: Measure the baseline**

Get a patch proposal on screen — ask the assistant to make a small edit to a file. Then:

```js
(() => {
  const actions = document.querySelector(".patch-actions");
  const log = document.querySelector(".chat-log");
  return {
    buttonWidths: [...actions.querySelectorAll("button")].map((b) =>
      Math.round(b.getBoundingClientRect().width),
    ),
    actionsDisplay: getComputedStyle(actions).display,
    logOverflows: log.scrollWidth > log.clientWidth,
  };
})()
```

Expected — the bad baseline: `actionsDisplay` is `"grid"` and each button is hundreds of pixels wide, roughly half the conversation column. Note `logOverflows`; it should be `false` now and must still be `false` at the end.

- [ ] **Step 2: Rebuild the header**

In `createPatchPreview`, replace the block from `const header = document.createElement("div");` through `wrapper.append(header, actions, secretNotice, list);` with:

```js
  const header = document.createElement("div");
  header.className = "patch-preview-header";

  const headerText = document.createElement("div");
  headerText.className = "patch-preview-heading";
  const title = document.createElement("strong");
  title.textContent = payload.summary || "Patch preview";
  const meta = document.createElement("span");
  headerText.append(title, meta);

  const actions = document.createElement("div");
  actions.className = "patch-actions";
  const applyButton = document.createElement("button");
  applyButton.type = "button";
  applyButton.className = "btn-sm btn-primary";
  const rejectButton = document.createElement("button");
  rejectButton.type = "button";
  rejectButton.className = "btn-sm btn-quiet";
  rejectButton.textContent = "Reject";
  actions.append(applyButton, rejectButton);
  header.append(headerText, actions);

  // Both the label and the metadata track the current selection, so the
  // header states what will actually happen rather than "Selected".
  function updateHeaderCounts() {
    const selected = state.files.filter((file) => file.selected);
    const additions = selected.reduce((sum, file) => sum + file.additions, 0);
    const deletions = selected.reduce((sum, file) => sum + file.deletions, 0);
    applyButton.textContent = `Apply ${selected.length}`;
    applyButton.disabled = selected.length === 0;
    meta.textContent =
      selected.length === 1
        ? `1 file · +${additions} −${deletions}`
        : `${selected.length} files · +${additions} −${deletions}`;
  }

  updateHeaderCounts();

  const secretNotice = document.createElement("div");
  secretNotice.className = "patch-secret-notice";
  secretNotice.hidden = true;

  const list = document.createElement("div");
  list.className = "diff-list";
  wrapper.append(header, secretNotice, list);
```

The patch id previously shown in `meta` is replaced by the file and line counts, which are more useful at a glance. It remains available in `state.patchId`.

- [ ] **Step 3: Keep the count live**

`createPatchPreview` has a local `render()` at `app.js:3899` that redraws the diff list whenever a file or hunk checkbox changes — it is called from eight sites within the function. It currently opens:

```js
  function render() {
    list.innerHTML = "";
    if (!state.files.length) {
```

Add the counts refresh as its first statement:

```js
  function render() {
    updateHeaderCounts();
    list.innerHTML = "";
    if (!state.files.length) {
```

One line, and every existing call site picks it up. `render()` is defined after `updateHeaderCounts` in source order but that is irrelevant — both are function declarations, so both are hoisted.

- [ ] **Step 4: Update the header CSS**

Replace the `.patch-preview-header strong, .patch-preview-header span` and `.patch-preview-header span` and `.patch-actions` rules with:

```css
/* The min-width: 0 note above still applies, but now to the text group
   rather than the header itself: it is the nowrap children that would
   otherwise set the row's automatic minimum and overflow the column. */
.patch-preview-heading {
  min-width: 0;
  display: flex;
  align-items: baseline;
  gap: 10px;
}

.patch-preview-heading strong,
.patch-preview-heading span {
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.patch-preview-heading span {
  flex: 0 0 auto;
  color: var(--muted);
  font-size: 11px;
  font-weight: 700;
}

.patch-actions {
  flex: 0 0 auto;
  display: flex;
  align-items: center;
  gap: 6px;
}
```

Keep the existing `.patch-preview-header` rule and its explanatory comment exactly as they are.

- [ ] **Step 5: Rebuild and re-measure**

Restart, get a patch proposal on screen, and run the Step 1 expression.

Expected: `actionsDisplay` is `"flex"`, each button width is roughly 60-90px rather than hundreds, and `logOverflows` is still `false`.

- [ ] **Step 6: Verify the count tracks selection**

Deselect one file's checkbox. Expected: the apply label decrements, and the `+N −N` counts drop by that file's contribution. Deselect all. Expected: the label reads `Apply 0` and the button is disabled.

Re-select and apply. Expected: the patch applies exactly as before — this task changed no apply logic.

- [ ] **Step 7: Verify the narrow-column case**

This is acceptance criterion 6. Narrow the window until the conversation column is at its 420px minimum (`.app` is `grid-template-columns: 300px minmax(420px, 1fr)`), with a patch whose summary is long. Then:

```js
(() => {
  const log = document.querySelector(".chat-log");
  const actions = document.querySelector(".patch-actions");
  return {
    logOverflows: log.scrollWidth > log.clientWidth,
    actionsVisible: actions.getBoundingClientRect().width > 0,
    summaryEllipsised: (() => {
      const s = document.querySelector(".patch-preview-heading strong");
      return s.scrollWidth > s.clientWidth;
    })(),
  };
})()
```

Expected: `logOverflows` is `false`, `actionsVisible` is `true`, and `summaryEllipsised` is `true` — the summary truncates and the buttons keep their full width, which is the correct priority.

- [ ] **Step 8: Check the specimen page**

Reload `docs/ui-style-guide.html`. The patch header specimen renders with the actions on the right at their natural width.

Note the specimen markup from Task 2 puts `<strong>` and `<span>` directly in `.patch-preview-header`. Update it to match the new structure — wrap them in `<div class="patch-preview-heading">` — so the specimen reflects what the app builds.

- [ ] **Step 9: Full manual pass**

Acceptance criterion 9. Drive the whole surface once: approve a command, reject a command, use both overflow grants, get a blocked proposal, apply a patch, reject a patch, and toggle hunk selection. Confirm no console errors throughout.

- [ ] **Step 10: Lint and commit**

```bash
npm run lint:web
```

Expected: clean.

```bash
git add crates/desktop-shell/static/app.js crates/desktop-shell/static/style.css docs/ui-style-guide.html
git commit -m "Size patch preview actions to their content

The actions were in .inline-actions, a two-column grid meant for the
narrow settings panel, so each button stretched to half the
conversation column. They move onto the header row at their natural
width, and the apply label carries the live selection count so the
header states what will happen rather than 'Selected'.

The load-bearing min-width: 0 moves to the new text group, since the
nowrap children are what would otherwise overflow the column."
```

---

### Task 6: Record the outcome in the spec

**Files:**
- Modify: `proposal.md` (status line)
- Modify: `docs/specs/README.md` (row 41)

- [ ] **Step 1: Update the spec status**

In `proposal.md`, change the `Status:` line to `Status: Done`, followed by the measured collapsed-card height recorded in Task 3 Step 6 and any deviation from the design. Follow the style of spec 16's status line, which states what was verified and points at deviations.

If the measured height exceeded the 120px in acceptance criterion 1, say so plainly with the real number and why. Do not adjust padding after the fact to hit the target — the criterion documents an expectation, and a miss is information.

- [ ] **Step 2: Update the README row**

Change row 41's `**Not started.**` to `**Done.**` and add the measured before/after height.

- [ ] **Step 3: Commit**

```bash
git add docs/specs/41_ui_density_and_action_hierarchy/proposal.md docs/specs/README.md
git commit -m "Mark spec 41 done with measured card heights"
```

---

## Self-review

**Spec coverage.** Requirement 1 → Task 1 Step 8. Requirement 2 → Task 1 Step 6. Requirement 3 → Task 1 Steps 2-4. Requirement 4 → Task 3 Steps 2, 5, 7. Requirement 5 → Task 3 Step 2, verified Step 7. Requirement 6 → Task 4 Steps 4-5. Requirement 7 → Task 5 Steps 2, 4. Requirement 8 → Task 1 Step 7, verified Task 4 Step 10. Requirement 9 → Task 2. Requirement 10 → every task's lint step. Acceptance criteria 1-9 map to Task 3 Step 6, Task 3 Step 7, Task 3 Step 7, Task 4 Step 8, Task 4 Step 9, Task 5 Step 7, Task 4 Step 10, Task 2 Step 3, Task 5 Step 9.

**Known gaps, stated rather than hidden.**

1. **Task 3 does not stand alone.** It removes the two grant buttons and Task 4 restores them as menu items. Between the two commits the grants are unreachable. Splitting differently would have meant either building the menu against buttons that were about to be deleted, or one oversized commit touching structure and behaviour together. The stand-ins in Task 3 Step 3 keep the intermediate state runnable, and both commits should land before the branch merges.
2. **No automated tests.** Every gate is a manual measurement, because the repo has no JS test harness and adding one is out of scope. The gates are exact expressions with exact expected values, so they are checkable rather than impressionistic, but nothing here runs in CI.
3. **Task 5 Step 7 depends on window resizing.** The 420px minimum column is reached by narrowing the app window, which on a large display may not be possible without also resizing the sidebar. If the column will not narrow far enough, verify by temporarily setting `.app { grid-template-columns: 300px 420px; }` in the WebView inspector rather than editing the stylesheet, and revert before measuring anything else.

**Type consistency.** `runButton`, `rejectButton`, `overflowButton`, `details`, `output`, `resolveCommandProposal`, `restoreActions`, `runWithGrant` are used consistently across Tasks 3 and 4. `toggleApprovalMenu(items, anchorEl)` is defined once in Task 4 Step 2 and called once in Step 4 with a matching shape. `updateHeaderCounts()` is defined and called in Task 5 Steps 2 and 3.
