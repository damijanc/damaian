# Header, Repo Bar and Menus — Task List

**Implements:** [`proposal.md`](proposal.md) · background in [`context.md`](context.md)
**Style reference:** [`../../UI_STYLE_GUIDE.md`](../../UI_STYLE_GUIDE.md) · rendered at `docs/ui-style-guide.html`
**Started:** —

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
| 1 · Git summary in the engine | Not started | |
| 2 · Shell: git-status fields and open-github | Not started | |
| 3 · One ⋮ menu component, vertical everywhere | Not started | |
| 4 · Sidebar session menu and search | Not started | |
| 5 · Header ⋮ menu, turn state into the conversation | Not started | |
| 6 · Repo bar, contextual action, mode pill | Not started | |
| 7 · Style guide, docs and close-out | Not started | |

## Task 1 · Git summary in the engine

Proposal §1 req 11–12, §3.4. File: `crates/workspace-engine/src/git_service.rs`.

- [ ] Add `GitSummary { branch: Option<String>, head: Option<String>, insertions: u64, deletions: u64, github: Option<GitHubRepo> }`, where `GitHubRepo { owner, name }` has `fn url(&self) -> String`.
- [ ] Add `parse_github_remote(url: &str) -> Option<GitHubRepo>` as a pure function. Write the tests first:
  - the three accepted forms, with and without `.git`;
  - rejects another host, `github.com.evil.example`, `https://user:pw@github.com/o/r`, an extra path segment, an empty owner, and characters outside `[A-Za-z0-9_.-]`.
- [ ] Add `GitService::summary(root)`:
  - branch from `git symbolic-ref --short -q HEAD`, with `None` when detached;
  - head from `git rev-parse --short HEAD`, with `None` when there are no commits;
  - counts from `git diff --numstat HEAD`, skipping binary `-\t-` rows;
  - untracked files from `git ls-files --others --exclude-standard`, counted by line, skipping files over 1 MiB and files with a NUL byte in the first 8 KiB;
  - with no `HEAD`, count every file as an insertion;
  - origin from `git remote get-url origin`.

  Audit the read as `git_status_read`, as `status` does.
- [ ] Add temp-repo tests: a branch with edits and an untracked file, detached HEAD, no commits, not a git repo (an error or an empty summary; pick one and record it), a binary file skipped. Use the existing temp-repo helpers.
- [ ] Run scoped checks and update this row.

## Task 2 · Shell: git-status fields and open-github

Proposal §1 req 11, 13 and 14, §3.4. File: `crates/desktop-shell/src/lib.rs`.

- [ ] Extend `handle_git_status` with `branch`, `head`, `insertions`, `deletions` and `github_url` (a string or `null`). Existing fields stay byte-identical. When the folder is not a git repo, return the existing payload with the new fields `null`/0. Do not error.
- [ ] Add `POST /api/open-github` (`repo` only): `validate_working_folder`, then `summary`, then `open <GitHubRepo::url()>`. It returns an error when there is no GitHub origin. Test that a request with an extra `url` field opens the derived URL. Inject the opener, or test the URL-building function the handler calls, so the test never launches a browser.
- [ ] Run scoped checks and update this row.

## Task 3 · One ⋮ menu component, vertical everywhere

Proposal §1 req 1 and 19, §3.1.

- [ ] Generalise `ensureProjectMenu` / `handleProjectMenuAction` into a factory that takes sections (rows, dividers, drill-in panels, danger rows last) and a handler. It owns arrows, Home/End, Escape returning focus to the trigger, outside click, `aria-expanded`, and flipping above when there is no room below.
- [ ] Re-express the project menu with the factory, with identical items and behaviour.
- [x] Switch the approval and recovery overflow buttons from `textContent = "⋯"` to `.dots-icon`. **Landed early, 2026-10-13**, with the style-guide fix. Their menus stay on `toggleApprovalMenu`; Task 3 decides whether they move to the factory.
- [ ] Confirm that `grep -n '⋯' crates/desktop-shell/static/` is empty.
- [ ] Check in the browser that the project menu still opens, drills into Open in, goes back and closes, by mouse and by keyboard. Check that the approval and recovery overflows still open.
- [ ] Run scoped checks and update this row.

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
