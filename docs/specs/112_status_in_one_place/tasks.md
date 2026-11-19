# Spec 112 tasks

**Started:** 2026-11-19

Implementation plan for [`proposal.md`](proposal.md). The evidence, the code
survey and the decisions are in [`context.md`](context.md).

## Global constraints

- One task, one session (AGENTS.md).
- Build before spec 110 Task 3 and never at the same time as another spec that
  edits `app.js` or `style.css` (context D4).
- No change to approval policy or to the routes that resolve a decision.
- Web assets stay vanilla JavaScript; `node --check` and Biome lint pass.
  Every UI change is checked in the running app
  (`serves_the_ui_for_manual_inspection` on its own port and data dir,
  stopped by PID) and the result is recorded in the task row.
- `docs/UI_STYLE_GUIDE.md` and `docs/ui-style-guide.html` change in the same
  task as the surface they describe.
- Scoped checks per task; the workspace gate stays with CI.

| Task | State | Durable result |
|---|---|---|
| 1 · Classes and tokens (req 8) | Done 2026-11-19 | No behaviour change. `app.js`: memory card Remember `btn-sm btn-primary`, Dismiss `btn-sm btn-quiet` (was `btn-secondary`, which had no rule); *Compact now* `btn-sm btn-quiet`; the two context-panel pin controls use a new `icon("pin")` glyph (`ICON_PATHS.pin`) instead of 📌, keeping their `aria-label`. `index.html`: the terminal close is an `.icon-btn`. `style.css`: `.icon-button` and its dead `.panel-icon` / `.is-collapsed` rules removed (nothing else used them); the terminal bar rule now sizes `.icon-btn` to 26px; `--accent-soft`'s `#f0f5ff` fallback removed. Colours: literals equal to a token replaced (`#ffffff`/`#fff` backgrounds → `--surface`, `#f0f0ed`, `#fbfbfa`, `#f7f7f5`, `#e7f1ee`); twelve new tokens for literals in three or more rules (`--on-accent`, `--ink-strong`, `--faint`, `--hover`, `--line-soft`, `--code-soft`, `--ok-line`, `--warn-soft`, `--warn-line`, `--warn-ink`, `--danger-soft`, `--danger-line`), listed in style guide §2. **Deviations:** `#eeeeea` (3 rules) folded into `--line-soft` (`#eeeeeb`), `#fbfbfb` into `--surface-soft` (`#fbfbfa`), both 1/255 apart; `#626568` (3 settings rules) became `--muted` (`#646b77`), a hair lighter and still above the contrast floor, rather than a near-duplicate token. Syntax colours were already grouped under their own comment and stay literals; one- and two-use literals stay (req 8 only asks for three or more). Distinct literals outside `:root`: about 85 → 67. Test: desktop-shell `static_assets_use_one_button_scale_and_drawn_icons` (no `btn-secondary`, `icon-button` or 📌 in the three assets; no `--accent-soft` fallback). Checked in the running app (`serves_the_ui_for_manual_inspection`, port 4911, its own data dir, stopped by PID): the terminal close renders as a quiet 26×26 `icon-btn`; the context panel's pin controls show the drawn pin (34×28, no text); `.btn-primary` text resolves to white; tokens resolve (`--warn-ink` = `#7a4b12`). Not exercised: the memory proposal card and the compaction boundary (no fixture shows them; the change there is class names only). Checks (local, 2026-11-19): `scripts/quality-gate.sh fmt node lint typos deny` ok (deny outside the agent sandbox for its advisory-db lock); `cargo clippy -p desktop-shell --all-targets --locked -- -D warnings` clean; `cargo nextest run -p desktop-shell --locked` 335 passed, 20 skipped (outside the sandbox for loopback binds; 57 s wall). Workspace clippy and tests left to CI. |
| 2 · One Stop (req 3) | Not started | |
| 3 · One composer status slot (req 2, 5) | Not started | |
| 4 · A pending card drawn once; no empty rows (req 4, 6) | Not started | |
| 5 · Session state in the sidebar (req 7) | Not started | |
| 6 · Documentation and close-out | Not started | |

## Task 1 · Classes and tokens (req 8)

No behaviour change.

- [x] Memory proposal: Remember `btn-sm btn-primary`, Dismiss `btn-sm
      btn-quiet`; *Compact now* `btn-sm`.
- [x] Merge `.icon-button` into `.icon-btn` (terminal close and any other
      user); delete the old rule.
- [x] Replace the 📌 control label with an `icon()` SVG and an `aria-label`.
- [x] Drop `--accent-soft`'s fallback; replace colour literals that equal an
      existing token; make a token of any literal used in three or more rules;
      group syntax-highlighting literals under one comment.
- [x] Desktop-shell test: `static/` contains no `btn-secondary` and no
      `icon-button` class.
- [x] Style guide §2 lists any new tokens; `ui-style-guide.html` still renders.

Files: `static/app.js`, `static/style.css`, `static/index.html`,
`docs/UI_STYLE_GUIDE.md`, `docs/ui-style-guide.html`, desktop-shell tests.
Checks: `scripts/quality-gate.sh fmt node lint typos deny`; `cargo nextest run
-p desktop-shell` for the new test; a visual pass in the running app (settings,
a card, the context panel, the terminal).

## Task 2 · One Stop (req 3)

- [ ] One function decides what the composer button stops for the current
      session: this window's stream (abort), else its running action
      (`action.stop()`), else work from working-sessions (cancel route), else
      nothing (Send). Click and Escape both use it.
- [ ] The question answer and fetch decision flows become stoppable through
      it (today a no-op).
- [ ] `startTurnIndicator` no longer renders a Stop; `onStop` is kept as the
      stop path the composer uses. `#composer-running`'s Stop is removed.
- [ ] Unstoppable work: the button shows a disabled Stop with the reason in
      its `title`.
- [ ] Steering notes unchanged (Enter with text during a turn).
- [ ] Desktop-shell `app_js_…` test: one Stop site; the turn row builds no
      button.

Files: `static/app.js`, `static/style.css`, desktop-shell tests.
Checks: web checks; `cargo nextest run -p desktop-shell`; in the running app:
a live mocked turn, an answered question resuming, and a resumed stream each
show one Stop that stops it (record the request each made).

## Task 3 · One composer status slot (req 2, 5)

- [ ] `index.html`: replace the three boxes with `#composer-status` (label,
      clock, one optional button).
- [ ] One `syncComposerStatus()` derives the entry by priority (needs you,
      working, busy elsewhere) and replaces `syncComposerRunning`,
      `syncComposerDecision`'s box and `syncInvisibleWork`'s box; the question
      lock and the reload-on-hide behaviour are kept.
- [ ] An `IntersectionObserver` on the pending frame and on the running
      action's status row (rooted at `#chat-log`) hides the matching entry
      while it is in view.
- [ ] Remove `appendPendingPointer` and its CSS; the placeholder reads
      "Answer the question".
- [ ] Entry text ≤ ~60 characters; the long explanations go in `title`.
- [ ] Desktop-shell test updated (spec 107's composer test and any that name
      the removed ids).

Files: `static/index.html`, `static/app.js`, `static/style.css`, desktop-shell
tests. Checks: web checks; `cargo nextest run -p desktop-shell`; in the running
app: the question fixture with the card in and out of view; a live turn
scrolled away; an injected working-sessions row for the open session.

## Task 4 · A pending card drawn once; no empty rows (req 4, 6)

- [ ] On reload, a transcript decision that the recovery sweep also reattached
      renders read-only; exactly one answerable copy is framed (context D7).
- [ ] An assistant `.message-body` with no content is not drawn; the status
      row stands alone until the first token.
- [ ] `markMessageRewindable` is skipped when `fileCount` is 0; the live path
      finds the checkpoint whose `userMessageId` is that turn's user message.
- [ ] Desktop-shell tests for the three.

Files: `static/app.js`, `static/style.css`, desktop-shell tests.
Checks: web checks; `cargo nextest run -p desktop-shell`; in the running app:
the question fixture after a server restart (one answerable card); a live
mocked turn (no empty box, no "0 files covered").

## Task 5 · Session state in the sidebar (req 7)

- [ ] `/api/sessions` rows carry `state`: `working` (from `lanes::working()`),
      `needs_you` (latest task `WaitingForApproval`), `failed` (latest task
      `Failed`), else absent. Measure the listing on 100 sessions (context D6).
- [ ] `renderProjectSession` draws `createWorkerDot`-style dots for those three
      states in place of the time, with `role="img"`, `aria-label` and `title`.
- [ ] The collapsed project row names its "N need you".
- [ ] The poll refetches the project's sessions when the working set changes,
      and a local turn's end refetches them.
- [ ] Spec 110 Task 3's sidebar bullet points here (done in the spec change;
      check it still reads true).
- [ ] Rust test for the `state` field (working, needs_you, failed, absent);
      desktop-shell `app_js_…` test for the dots and labels.

Files: `crates/desktop-shell/src/json.rs`, `session_api.rs`, possibly
`workspace-engine/src/session.rs` (latest task status), `static/app.js`,
`static/style.css`, tests. Checks: scoped clippy for each crate touched;
`cargo nextest run -p desktop-shell`; scoped workspace-engine tests if it
changes; in the running app: the fixture's question session shows Needs you,
a failed session Failed, a live mocked turn Working, idle rows nothing.

## Task 6 · Documentation and close-out

- [ ] `UI_STYLE_GUIDE.md`: principles "every state has one home" and "one
      vocabulary of states"; §4 the composer status slot and the one Stop;
      session dots beside worker dots; §8 anti-patterns "the same state in
      several places" and "a Stop on every surface".
- [ ] `ui-style-guide.html`: specimens for the slot and a session row with
      each dot.
- [ ] Spec 107: a dated note on requirement 1's pointer and requirement 2's
      composer line (shown only while the card is out of view).
- [ ] `USER_GUIDE.md` / `TROUBLESHOOTING.md`: what the slot, the Stop and the
      sidebar dots mean; why a session shows Needs you after a restart.
- [ ] Close-out per AGENTS.md: Status in `proposal.md`, this header, the
      README row and *What to build next*; the CHANGELOG *Unreleased* line;
      every `Depends on:` naming #112.

Checks: `scripts/quality-gate.sh fmt node lint typos deny`; `npm run
specs:check`. Workspace clippy and tests left to CI.
