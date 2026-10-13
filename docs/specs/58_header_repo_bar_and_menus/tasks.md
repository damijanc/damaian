# Header, Repo Bar and Menus — Task List

**Implements:** [`proposal.md`](proposal.md) · background in [`context.md`](context.md)
**Style reference:** [`../../UI_STYLE_GUIDE.md`](../../UI_STYLE_GUIDE.md) · rendered at `docs/ui-style-guide.html`
**Started:** 2026-10-13

## Global constraints

- **Each task is its own agent session** (AGENTS.md). Read the proposal
  sections a task names, not the whole spec.
- **Order:**
  - Tasks 1 → 2 are backend.
  - Tasks 3 → 4 → 5 → 6 all edit `static/app.js`, `index.html` and
    `style.css`, so run them strictly in sequence.
  - Tasks 1–2 may run in parallel with Task 3, because they touch different
    files.
  - Task 5 needs Task 2's fields.
- **No semantics change.** Findings repair, session mode, export redaction and
  delete confirmation keep their current server paths. A test that pinned old
  behaviour may change its selectors, not its assertions.
- **Verify in the running app.** Use the ignored
  `serves_the_ui_for_manual_inspection` harness (`desktop-shell`, port 4899,
  its own data dir). Static assets are `include_str!`-embedded, so rebuild and
  restart after every asset edit. Never touch port 4765, and never kill a
  process by name.
- **Scoped checks per task:**
  - `cargo nextest run -p <crate> -E 'test(<filter>)'`
  - `cargo clippy -p <crate> --all-targets --locked -- -D warnings`
  - `node --check crates/desktop-shell/static/app.js`
  - `npm run lint:web`

  Task 7 runs the full seven-check gate.

## Progress

| Task | State | Notes |
|---|---|---|
| 1 · Git summary in the engine | Done | 2026-10-13. `git_service.rs`: `GitSummary`, `GitHubRepo` (`owner()`, `name()`, `url()`), pure `parse_github_remote`, `GitService::summary(root) -> Result<GitSummary>`; all four re-exported from the crate root. **Non-git folder → `Ok(GitSummary::default())`** (all `None`/0), not an error, so Task 2 can map the summary straight onto `null`/0 fields with no error branch; it is still audited as `git_status_read` with `status: failed` and git's exit code, as `status` records it. Audit fields add `scope: summary` and never include the remote. **Deviations:** `GitHubRepo`'s fields are private (getters instead), so the only way to get one, and so a `url()`, is `parse_github_remote`; Task 2's test must build one through that function. The parser also rejects `.`/`..` as owner or name, a trailing `/`, ports, and any whitespace (it does not trim; `summary` trims git's output). `head` uses `rev-parse --short --verify -q HEAD`; numstat runs with `--no-ext-diff --no-textconv`; untracked symlinks count 0. With no commits, `ls-files --cached --others` is counted, so staged-but-uncommitted files are included. Tests: 16 in `git_service::tests` (9 `github_remote_*` parser tests, 6 `git_service_summary_*` temp-repo tests, plus `github_remote_url_is_rebuilt_from_the_parsed_parts`), confirmed red against stubs first; `cargo nextest run -p workspace-engine -E 'test(git_service) + test(github)'` 16/16 pass; clippy `-p workspace-engine` clean; `cargo fmt --check` and `typos` clean. The temp-repo tests build a `GitService` directly (no `WorkspaceEngine`, so no watcher). |
| 2 · Shell: git-status fields and open-github | Not started | |
| 3 · One ⋮ menu component, vertical everywhere | Done | 2026-10-13. `app.js`: **`createContextMenu({ className, align = "end", placement = "below", onAction })` → `{ toggle, open, close, isOpen }`**; `menu.toggle(triggerEl, { sections, key, context })`. `sections` is an array of arrays of entries; a divider is drawn between non-empty sections. An entry is `{ label, action?, hint?, danger?, hidden?, disabled?, className?, onSelect?(context), items? }`: `items` (itself sections) makes a drill-in panel with a back row labelled after the entry; `onSelect` wins over the menu's `onAction(action, context)`; `hidden` entries are dropped (use it for the GitHub item); danger rows are moved to the end of their panel and stray dividers trimmed. One popover per menu instance on `document.body`, `position: fixed`, rebuilt on every open. The factory owns: focus to the first row on open (the first non-back row on drill-in; the opener row on back); ArrowUp/Down (wrapping), Home/End, ArrowRight into a drill-in, ArrowLeft back; Escape and Tab close and return focus to the trigger; selecting a row closes and returns focus to the trigger *before* the action runs (dialogs then take focus); outside click closes; opening any menu closes every other (`closeAllContextMenus`); `aria-expanded` on the trigger (callers set the initial `"false"`); `key` (default: the trigger) makes a second click on the same target close it even after a re-render replaced the trigger. Placement: below, flipping above when the menu does not fit below and there is more room above; `placement: "above"` prefers above (Task 6's repo bar); `align: "start"` lines up left edges. `data-placement` on the popover records which. Rows get `role="menuitem"`, dividers `.context-menu-divider` `role="separator"`. **Project menu** re-expressed as `projectMenu` + `toggleProjectMenu(path, btn)` + `handleProjectMenuAction(action, path)`; same items, `data-action` values and drill-in. **Approval/recovery decision: moved** onto the factory (`approvalMenu`, `align: "start"`, `.approval-menu-popover`/`.approval-menu-row` kept); `toggleApprovalMenu(items, btn)` and `closeApprovalMenu()` keep their signatures, so no call site changed beyond the initial `aria-expanded`. It was trivial and gains the keyboard handling req 19 asks for. **Deviations:** `style.css` adds `:focus-visible` row background and `.context-menu-divider`, and fixes a pre-existing bug: `.approval-menu-row` inherited `.context-menu-row`'s three grid columns, putting the hint beside the label and squeezing the label to a word per line; it is now one column. **Verified** on the `serves_the_ui_for_manual_inspection` harness (port 4899) in the Browser pane: project ⋮ by mouse opens below, drills into Open in, back, closes on outside click, `aria-expanded` true/false; by keyboard (Tab to ⋮, Enter): ArrowDown/Up wrap, Home/End, ArrowRight drills in, ArrowUp+Enter on back and ArrowLeft go back, Escape closes with focus on the ⋮, Rename via keyboard opens its dialog. Recovery ⋮ and approval ⋮ open, toggle closed on a second click, Escape and Tab return focus, outside click closes; the approval ⋮ near the window bottom flipped above. VS Code/Finder not clicked (they launch apps). Checks: `node --check` OK; `npm run lint:web` clean (1 pre-existing info in a scripts file); `typos` clean; `grep -n '⋯' crates/desktop-shell/static/` empty; no `desktop-shell` test reads the static assets, so no nextest filter applies. **For Tasks 4–6:** build a menu once at module level with `createContextMenu`, call `.toggle(btn, { key, context, sections })` from the trigger's click handler with `event.stopPropagation()`, give the trigger `aria-haspopup="menu"` and `aria-expanded="false"`. Do not add document-level close handlers; the factory has them. |
| 4 · Sidebar session menu and search | Not started | |
| 5 · Header ⋮ menu, turn state into the conversation | Not started | |
| 6 · Repo bar, contextual action, mode pill | Not started | |
| 7 · Style guide, docs and close-out | Not started | |

## Task 1 · Git summary in the engine

Proposal §1 req 11–12, §3.4. File: `crates/workspace-engine/src/git_service.rs`.

- [x] Add `GitSummary { branch: Option<String>, head: Option<String>, insertions: u64, deletions: u64, github: Option<GitHubRepo> }`, where `GitHubRepo { owner, name }` has `fn url(&self) -> String`.
- [x] Add `parse_github_remote(url: &str) -> Option<GitHubRepo>` as a pure function. Write the tests first:
  - the three accepted forms, with and without `.git`;
  - rejects another host, `github.com.evil.example`, `https://user:pw@github.com/o/r`, an extra path segment, an empty owner, and characters outside `[A-Za-z0-9_.-]`.
- [x] Add `GitService::summary(root)`:
  - branch from `git symbolic-ref --short -q HEAD`, with `None` when detached;
  - head from `git rev-parse --short HEAD`, with `None` when there are no commits;
  - counts from `git diff --numstat HEAD`, skipping binary `-\t-` rows;
  - untracked files from `git ls-files --others --exclude-standard`, counted by line, skipping files over 1 MiB and files with a NUL byte in the first 8 KiB;
  - with no `HEAD`, count every file as an insertion;
  - origin from `git remote get-url origin`.

  Audit the read as `git_status_read`, as `status` does.
- [x] Add temp-repo tests: a branch with edits and an untracked file, detached HEAD, no commits, not a git repo (an error or an empty summary; pick one and record it), a binary file skipped. Use the existing temp-repo helpers.
- [x] Run scoped checks and update this row.

## Task 2 · Shell: git-status fields and open-github

Proposal §1 req 11, 13 and 14, §3.4. File: `crates/desktop-shell/src/lib.rs`.

- [ ] Extend `handle_git_status` with `branch`, `head`, `insertions`, `deletions` and `github_url` (a string or `null`). Existing fields stay byte-identical. When the folder is not a git repo, return the existing payload with the new fields `null`/0. Do not error.
- [ ] Add `POST /api/open-github` (`repo` only): `validate_working_folder`, then `summary`, then `open <GitHubRepo::url()>`. It returns an error when there is no GitHub origin. Test that a request with an extra `url` field opens the derived URL. Inject the opener, or test the URL-building function the handler calls, so the test never launches a browser.
- [ ] Run scoped checks and update this row.

## Task 3 · One ⋮ menu component, vertical everywhere

Proposal §1 req 1 and 19, §3.1.

- [x] Generalise `ensureProjectMenu` / `handleProjectMenuAction` into a factory that takes sections (rows, dividers, drill-in panels, danger rows last) and a handler. It owns arrows, Home/End, Escape returning focus to the trigger, outside click, `aria-expanded`, and flipping above when there is no room below.
- [x] Re-express the project menu with the factory, with identical items and behaviour.
- [x] Switch the approval and recovery overflow buttons from `textContent = "⋯"` to `.dots-icon`. **Landed early, 2026-10-13**, with the style-guide fix. Their menus stay on `toggleApprovalMenu`; Task 3 decides whether they move to the factory.
- [x] Confirm that `grep -n '⋯' crates/desktop-shell/static/` is empty.
- [x] Check in the browser that the project menu still opens, drills into Open in, goes back and closes, by mouse and by keyboard. Check that the approval and recovery overflows still open.
- [x] Run scoped checks and update this row.

## Task 4 · Sidebar session menu and search

Proposal §1 req 3, 4 and 8.

- [ ] Replace `.project-session-delete` and `.project-session-export` with a `.dots-icon` trigger opening Rename / Export as Markdown / — / Delete. It is visible on `.active`, `:hover` and `:focus-within`. Double-click rename stays.
- [ ] Move `#session-search-btn` into the sidebar header next to `+`. The panel and its listeners are unchanged.
- [ ] Remove the CSS that only the old buttons used.
- [ ] Browser check: rename, export (a file is saved and carries the redaction line) and delete (confirm, then gone) from the menu; search from the sidebar.
- [ ] Run scoped checks and update this row.

## Task 5 · Header ⋮ menu, turn state into the conversation

Proposal §1 req 5–7, §3.2 (including its table).

- [ ] Give every turn path the in-bubble `startTurnIndicator`:
  - the resumes in `createCommandApprovalPreview`, `createPlanReview` and
    `createFallbackOffer`;
  - `proposePatchFromChat`.

  Pass a stop handler only where that path already cancels. Otherwise draw no
  Stop. Record which paths got Stop.
- [ ] Add the Incomplete mark on the message. Add the "Desktop API
  unavailable" banner above `#chat-log`. Check whether the budget-exhausted
  marks survive a session reload, and record the answer.
- [ ] Remove `#chat-status`, `setChatStatus` and every caller. Remove
  `#open-vscode-btn` and `#thread-repo` from the header. `#thread-repo`'s
  content moves to the repo bar in Task 6, so until then it may stay hidden.
  Add the header ⋮: Open in › (VS Code, Finder, Terminal) / Rename session /
  Export as Markdown / — / Delete session.
- [ ] Browser check: each header ⋮ item, keyboard only; the indicator on a chat
  turn and on a command-approval resume; Stop; a failed turn (bad key); the
  Incomplete mark.
- [ ] Run scoped checks and update this row.

## Task 6 · Repo bar, contextual action, mode pill

Proposal §1 req 9–10 and 14–18, §3.3, §3.5.

- [ ] Add the repo bar inside `.composer`, above `.prompt-box`, with the folder button and folder menu, branch or short SHA, counts, and the action slot. It is fed by `/api/git-status`. Replace `setRepoState`'s "N changed" text.
- [ ] Refresh triggers: session load, turn done, patch apply and rollback, rewind, and window focus (throttled to 5 s). No timers.
- [ ] Contextual action: Fix N errors, then Fix N warnings, then hidden. The main button sends all open findings of that severity through the existing repair path, and is disabled while `chatSubmitting`. The chevron moves the existing findings list into an upward popover. Remove `#findings-toggle-btn`.
- [ ] Replace `#session-mode-select` with the composer mode pill (after attach, before the model). Same endpoint, same snap-back, disabled with no session.
- [ ] Browser check, non-git folder: bar shows the folder only.
- [ ] Browser check, detached HEAD: shows the SHA.
- [ ] Browser check, counts: match `git diff --numstat HEAD` plus untracked lines.
- [ ] Browser check, the three action states.
- [ ] Browser check, a Fix all turn.
- [ ] Browser check, mode: switch, reload, and snap-back on a forced failure.
- [ ] Browser check, GitHub: on this repo, opens the right URL.
- [ ] Run scoped checks and update this row.

## Task 7 · Style guide, docs and close-out

Proposal §1 req 2, acceptance criteria 1–14. Fresh session; may read the whole spec.

- [x] **Landed early, 2026-10-13, with the spec itself** (requirement 2): `UI_STYLE_GUIDE.md` §3 rule and table, §4 and the §8 anti-pattern now prescribe the vertical `.dots-icon`; every `⋯` specimen in `docs/ui-style-guide.html` became `.dots-icon`. Two stylesheet lines came with it: `.dots-icon` is `inline-block` (it was invisible outside a grid button) and `.btn-icon` has `min-width: 24px` (a lone 3px dot left it 21px wide, under the 24px floor). Verified on the specimen page: all seven overflow specimens render, centred, 24×31. The shipped approval and recovery overflow buttons (`app.js`) were switched to `.dots-icon` in the same change, so Task 3's step for them is done too.
- [ ] Guide: add the ⋮ menu and repo-bar patterns; specimens for the ⋮ menu and the repo bar in its three action states.
- [ ] `docs/USER_GUIDE.md`:
  - Search and Export: export is now in the session ⋮;
  - Findings: the repo-bar action replaces the Findings button;
  - Working Modes: the mode is now a pill in the composer.

  Check whether `docs/TROUBLESHOOTING.md` mentions any moved control.
- [ ] Add a one-line "placement superseded by #58" note where #20, #22, #43 and #53 describe a control's position. Do not rewrite them.
- [ ] Walk every acceptance criterion in the running app and record the result per criterion below.
- [ ] Run the full seven-check gate and `npm run specs:check`.
- [ ] Mark Done in every place this spec's status is recorded:
  - the proposal's `Status:` line;
  - the README row;
  - this file's header and table.

  Then:
  - re-derive README "What to build next";
  - remove the `CHANGELOG.md` Unreleased line;
  - grep for `Depends on:` lines naming #58.
