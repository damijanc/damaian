# Context: Header, Repo Bar and Menus

Background for [`proposal.md`](proposal.md). Nothing here is a requirement.

## 1. Motivation

This came from usability feedback on 2026-10-12, given with Claude Code's
desktop app as the reference for a clean look. The user raised four points:

- **Session actions are bare glyphs.** Delete is a `-` and export is a `↓` on
  every sidebar row, always visible. Rename exists only as an undiscoverable
  double-click.
- **The working mode is in the wrong place.** It sits in the thread header, far
  from the prompt it governs.
- **The Findings button was not understood.** Asked what a click is supposed to
  do, the honest answer exposed three defects (§3).
- **Horizontal dots.** A vertical ⋮ had been asked for once already, and
  horizontal `⋯` controls were still being produced (§4).

## 2. Current state

All paths are under `crates/desktop-shell/` unless stated.

- **Thread header** (`static/index.html`, `.thread-header`). It holds the
  `#thread-repo` eyebrow and the `#thread-session` title. On the right:
  - `#session-mode-select`, placed first there by #20 Task 8;
  - the `#chat-status` badge;
  - `#findings-toggle-btn`;
  - `#session-search-btn`;
  - `#open-vscode-btn`;
  - `#terminal-toggle-btn`.
- **Sidebar session row** (`app.js`, the function that builds
  `.project-session-item`). It builds the open button, plus
  `.project-session-delete` (`-`) and `.project-session-export` (`↓`), which
  call `deleteSessionForProject` and `exportSessionDownload`.
- **Project row menu** (`app.js`, `ensureProjectMenu` and
  `handleProjectMenuAction`). This is already a `.context-menu-popover`:
  - triggered by `.project-menu-btn` with the vertical `.dots-icon`;
  - root panel: Open in ›, Rename, Delete (danger);
  - Open in panel: back, VS Code, Finder.

  This is the component the spec generalises.
- **Overflow buttons using a text `⋯`** (`app.js`). Both are `.btn-icon` with
  `textContent = "⋯"`, opened through `toggleApprovalMenu`:
  - command approval: "More approval options";
  - crash recovery: "More recovery options".
- **Findings** (`app.js`, `renderFindingsPanel`, `fixSelectedFindings`).
  - The toggle shows when the session has any finding.
  - The panel sits between `#chat-log` and the composer.
  - Repair posts `finding_ids` to `/api/findings-repair` and sends the returned
    prompt with `sendChatPrompt({ agentic: true })`.
- **Git status** (`workspace-engine/src/git_service.rs`, `GitService::status`;
  shell `handle_git_status` in `src/lib.rs`).
  - It runs `git status --porcelain=v1` and returns `path`, `raw`, `untracked`
    and `conflicted` per file.
  - It has no branch and no line counts.
  - Every read is audited as `git_status_read`.
- **Turn status** (`app.js`, `setChatStatus`, `startTurnIndicator`,
  `chatCompletionStatus`, `markMessageTurnStop`).
  - The header badge cycles through: Starting, Thinking, Streaming, Generating
    patch, Patch ready, Complete, Incomplete, Tool budget exhausted, Token
    budget exhausted, Stopped, Failed, Loaded, Idle and Desktop API
    unavailable.
  - The chat send already draws an in-bubble turn indicator. The resumes
    after a command, plan or fallback-offer approval, and patch generation, do
    not. For those, the badge is the only progress signal.
- - **Opening things.** `open_in_vscode` and `reveal_in_finder` (`src/lib.rs`)
  validate the folder, then run `open`. There is no route that opens a URL.
  The PTY terminal (`desktop-app/src/main.rs`, `terminal_open`) always starts
  in `terminal_cwd_for_repo(repo)`.

## 3. What the Findings button got wrong

1. **The trigger and the result are far apart.** The button is in the header
   and the panel opens above the composer. Nothing near the click changes.
2. **It can open empty.** The button shows for *any* finding, including
   warnings, stale, dismissed or fixed ones. The panel's default filter is open
   errors only, so "Showing 0 of 4" is reachable in one click.
3. **It sits in the wrong layer.** A finding exists so that someone says "fix
   this", which is a next-turn action. The header holds window tools.

The contextual action in the repo bar fixes all three:
- it appears only when there is something to fix;
- it says how much;
- it sits beside the prompt it will send.

## 4. Why the horizontal dots came back

The earlier request for vertical dots was handled by changing
`.project-menu-btn` to `.dots-icon`. `UI_STYLE_GUIDE.md`, the standing visual
reference later UI specs cite instead of restating it, was not changed:
- §3 still lists `.btn-icon` for "Overflow `⋯`";
- §4 still says "behind `⋯`";
- the specimen page still shows `⋯` in five places.

The approval and recovery overflows were built to the guide and came out
horizontal. Fixing only the controls again would repeat the same failure, so
requirement 2 fixes the guide and adds an anti-pattern.

## 5. Where the design came from

The user asked for a larger target:
- header icons for terminal, changes, browser and files, plus a ⋮;
- a bar showing folder, branch, change count and a contextual action
  (Create PR);
- a folder menu and a branch menu;
- a change count that opens a diff.

It was split because only part of it exists to rearrange:

| Piece | State | Where it goes |
|---|---|---|
| ⋮ menus, header cleanup, mode pill, repo bar, folder menu | Moving existing pieces | This spec |
| Change count → diff view, "changes" icon | No working-tree diff view exists | Next spec in the series |
| Browser panel, files panel | Do not exist | Later specs |
| Branch menu | Branch switching is #36 | #36 |
| Commit / Create PR action | #35 built 2026-10-26; #37 not started | They fill this spec's slot |

The layout was agreed against a clickable mockup on 2026-10-12–13. Two
decisions changed during review:
- **Repo bar position.** First drawn between the header and the chat log, it
  moved into the composer directly above the prompt, at the user's request.
- **Findings.** The user chose to fold findings into the contextual action, over
  a separate findings strip.

The mockup's Open in fly-out became a drill-in panel. That is the behaviour the
existing project menu already has (proposal §3.1).
