# Feature Spec: Header, Repo Bar and Menus

Status: In progress. Specified 2026-10-13; Task 1 of 7 done 2026-10-13 (see `tasks.md`).
Order: 58 of 58
Also in this spec: [`context.md`](context.md) (motivation, current state with
file references, where the design came from), [`tasks.md`](tasks.md) (execution
order and progress).
Reference: [`../../UI_STYLE_GUIDE.md`](../../UI_STYLE_GUIDE.md). This spec adds
the shared ⋮ menu and the repo bar to the guide, then applies them.
Depends on: [#20](../20_working_modes/proposal.md) (the mode control it
moves), built; [#22](../22_findings_model_and_panel/proposal.md) (the findings
panel it re-homes and the repair path it calls), built;
[#43](../43_chrome_density_and_hierarchy/proposal.md) (the thread header it
rearranges), built; [#53](../53_session_search_and_export/proposal.md) (the
search and export controls it moves), built.
Everything else named below is a cross-reference, not a prerequisite.
Related implementation specs:
[`../35_commit_preparation.md`](../35_commit_preparation.md) and
[`../37_pull_request_creation.md`](../37_pull_request_creation.md) (they fill
the contextual-action slot this spec creates; not built here),
[`../36_branch_and_worktree_delivery.md`](../36_branch_and_worktree_delivery.md)
(owns the branch menu; not built here).

**Not a roadmap graduation.** Like #41–#44, this comes from usability feedback
on the desktop shell, given on 2026-10-12 with Claude Code's desktop app as the
reference. It was agreed against a clickable mockup before it was written. The
user asked for a larger layout: header icons for changes, browser and files, a
diff view, a branch menu and a commit/PR action. That layout was split into
this spec, which rearranges what already exists, plus follow-ups that each need
new capability (§2).

## 1. Requirements

1. Every "more actions" control in the app is a **vertical** ⋮ drawn with
   `.dots-icon`, never a text glyph. That includes the two overflow buttons that
   still render a horizontal `⋯` today: command approval (`app.js`, "More
   approval options") and crash recovery ("More recovery options"). Session,
   header and folder menus open one menu component, today's project-row menu
   generalised. No horizontal ⋯ and no bare glyph buttons (`-`, `↓`) remain.
2. **The style guide is fixed at the source.** A vertical ⋮ was already
   requested once. Only the project row was changed, and the guide kept
   prescribing `⋯`, so later controls were built from the guide and came out
   horizontal. These all specify the vertical `.dots-icon`:
   - `UI_STYLE_GUIDE.md` §3 (the `.btn-icon` row) and §4 (Overflow);
   - every overflow specimen in `docs/ui-style-guide.html`;
   - §8, which gains an anti-pattern: a horizontal `⋯` or a text-glyph overflow
     trigger.
3. Each sidebar session row has a ⋮ menu: **Rename**, **Export as Markdown**,
   a divider, **Delete** (danger). The `-` and `↓` row buttons are removed.
   Double-click rename keeps working.
4. The ⋮ is visible on the active row and on hover or focus-within for other
   rows. Every row, including those whose ⋮ is hidden, is reachable by
   keyboard.
5. The thread header holds only the session title, the terminal toggle and a
   header ⋮ menu. The mode select, the Findings button, the VS Code button, the
   search button and the `#chat-status` badge leave the header. The header
   shows no turn state at all.
6. The header ⋮ menu has an **Open in** section first (VS Code, Finder,
   Terminal), then Rename session and Export as Markdown, then Delete session
   (danger). Each item calls the same function its old control called.
7. **Turn state lives in the conversation, not in chrome.** Every state
   `#chat-status` shows today moves to where the user is already looking
   (§3.2):
   - **Running.** Every path that runs a turn shows the existing in-bubble
     turn indicator (`startTurnIndicator`): phase label, elapsed time, and Stop
     where the path supports stopping. These paths are the chat send, a
     resumed turn after a command, plan or fallback-offer approval, and patch
     generation. Today only the chat send uses it.
   - **Ended other than complete.** Each end state is marked on that turn's
     message: Stopped, Failed, Tool budget exhausted, Token budget exhausted,
     and Incomplete. Incomplete has no mark today. Complete adds no mark.
   - **Patch ready.** The patch review card already says this.
   - **Desktop API unavailable.** This is a persistent error banner above the
     conversation, because it is the app failing, not a turn.
   - **Starting, Loaded, Idle.** These are dropped. They describe nothing the
     user can act on.
8. Session search opens from an icon in the sidebar header, next to the
   new-session `+`. The panel and its behaviour are unchanged.
9. A **repo bar** sits inside the composer, directly above the prompt box. On
   the left: a folder button, then the branch name in muted monospace. On the
   right: the change count `+N −M`, then the contextual-action slot.
10. The folder button opens the folder menu: **Show in Finder**, **Open
   repository on GitHub**, divider, **Copy path**, **Change folder…**, divider,
   **Open in terminal**. The GitHub item appears only when the server reports a
   GitHub URL.
11. `GET /api/git-status` additionally returns:
    - `branch`: the branch name, or `null` on a detached HEAD;
    - `head`: the short SHA;
    - `insertions` and `deletions`: working tree against `HEAD`, with untracked
      text files counted as insertions;
    - `githubUrl`: a string or `null`.

    Existing fields are unchanged.
12. `githubUrl` is built only from the `origin` remote, only when its host is
    exactly `github.com`, and always as `https://github.com/<owner>/<repo>`
    from the parsed owner and repository name. The raw remote string is never
    returned or opened, because the remote is repository-controlled input.
13. A new `POST /api/open-github` takes `repo`, derives the URL server-side the
    same way, and opens it with the system browser. The client never sends a
    URL.
14. In a folder that is not a git repository, the branch, the change count and
    the GitHub item are hidden, and the folder menu still works. A detached HEAD
    shows the short SHA in place of the branch.
15. The repo bar refreshes on session load, after every finished turn, after a
    patch apply or rollback, after a rewind, and on window focus. Focus
    refreshes are at least 5 s apart. There is no timer polling.
16. The contextual-action slot shows **Fix N errors** (danger tone) when the
    session has open error findings. With no errors but open warnings it shows
    **Fix N warnings** (warning tone). Otherwise it is hidden. Its main button
    repairs every open finding of that severity through the existing
    `/api/findings-repair`. It is disabled while a turn runs.
17. The action's chevron opens the existing findings list as a popover anchored
    to the bar, opening upward, with its filters, checkboxes, Dismiss and **Fix
    selected** unchanged. The `#findings-toggle-btn` and the inline
    `#findings-panel` position are removed.
18. The working mode is a pill in the composer action row, after `+` attach and
    before the model trigger. It keeps #20's behaviour exactly: it posts to
    `/api/session-mode`, snaps back on failure, is disabled with no session
    open, and shows no confirmation.
19. Every menu and popover closes on Escape (returning focus to its trigger) and
    on an outside click. Arrow keys move between items, and danger items are
    always last. Delete keeps its existing confirmation.

## 2. Non-goals

- **Changes, browser and files header icons.** The diff view is the next spec
  in this series (§3.6). Browser and files panels are later specs. Showing an
  icon for a feature that does not exist would be dead UI.
- **Clicking the change count.** In this spec it is display-only. The diff
  spec makes it open the diff.
- **Branch menu, branch switching, commit and PR actions.** These belong to
  #36, #35 and #37. This spec only reserves the slot and its priority rule
  (§3.5).
- **Changing what findings are, how repair prompts are built, or the mode
  permission matrix.** This spec moves controls, not semantics.
- **A new folder picker.** Change folder… calls the existing one.

## 3. Design

### 3.1 One menu component

The project-row menu (`ensureProjectMenu`, `app.js`) already provides the
building blocks:
- a `.context-menu-popover` with drill-in panels (root → Open in → back);
- a `.context-menu-row-danger` style;
- the vertical `.dots-icon`.

Generalise it into a factory that takes a list of sections (rows, dividers,
drill-in panels) and an action handler. The project, session-row, header and
folder menus all become instances of it. The factory owns:

- keyboard handling: arrows, Home/End, Escape returns focus to the trigger;
- outside-click close and `aria-expanded`;
- placement: below the trigger, flipping above when there is no room, which
  the repo bar always needs.

**Open in stays a drill-in panel**, as the project menu does today, not the
fly-out shown in the mockup. One behaviour for every menu matters more than
the fly-out, and the drill-in already exists and has been verified.

### 3.2 Header and turn state

```
[Session title]                                                 [>_] [⋮]
```

The title keeps its ellipsis. The terminal toggle keeps its id, so the existing
listener and `aria-pressed` handling are untouched. The header ⋮ menu's
**Open in → Terminal** opens the terminal panel, which the folder menu's
**Open in terminal** also does: `setTerminalOpen(true)`. The PTY already
starts at the repo root through `terminal_cwd_for_repo`.

**Why turn state left the header.** The mockup first put a small "● Working…"
next to the title. In review it went unnoticed: during a turn the user is
looking at the reply and the prompt, not the header. The conversation already
has the right element. `startTurnIndicator` (`app.js`) draws a row inside the
assistant bubble with a phase label ("Waiting for model", a tool's own label,
"Streaming", "round 2/8"), an elapsed clock and a Stop button. It finishes as
"Stopped by you" or removes itself.

Before this spec, the header badge was the *only* progress signal for these
paths:
- `createCommandApprovalPreview`'s resume;
- `createPlanReview`'s resume;
- `createFallbackOffer`'s resume;
- `proposePatchFromChat`.

Removing the badge without giving those paths the indicator would leave
those turns with no visible progress. Each of them gets `startTurnIndicator`
on the message it streams into. Stop is offered only where that path already
has a working cancel. Otherwise the indicator is passed no stop handler and
draws no button.

End states are marked on the message:

| Badge today | After |
|---|---|
| Starting, Thinking, Streaming, Generating patch | Turn indicator phase label |
| Patch ready | The patch review card (unchanged) |
| Complete | Nothing: the turn indicator removes itself |
| Incomplete | **New** quiet mark on the message: "Stopped before finishing" |
| Tool / Token budget exhausted | Existing `markMessageToolBudgetExhausted` / `markMessageTokenBudgetExhausted` |
| Stopped | Existing "Stopped by you" row |
| Failed | Existing `renderChatMessageError` in the bubble, plus the toast |
| Desktop API unavailable | **New** persistent error banner above `#chat-log` |
| Loaded, Idle | Dropped |

### 3.3 Repo bar

```
[damaian-client ▸menu] main                     +12,882 −416  [Fix 3 errors|▾]
┌──────────────────────────────────────────────────────────────────────────┐
│ Ask a question or request a file change                                  │
│ [+] [Code ▾] [⚡ model ▾] [effort ▾]                                  [↑] │
└──────────────────────────────────────────────────────────────────────────┘
```

Styling:
- A rounded strip on a slightly darker surface than `--bg`, one control-height
  tall, the same width as the prompt box.
- Counts use monospace, insertions in `--ok`, deletions in `--danger`.
- The bar replaces the `#thread-repo` eyebrow and the "N changed" text that
  `setRepoState` writes into it today.

### 3.4 Git summary

`GitService` gains a `summary(root)` method returning branch, head, insertions,
deletions and the parsed GitHub owner/repo. It is audited as
`git_status_read`, as `status` is.

- **Insertions and deletions** come from `git diff --numstat HEAD`, with
  binary files (`-\t-`) skipped.
- **Untracked files** are counted by line, skipping files over 1 MiB and files
  containing a NUL byte in the first 8 KiB.
- **A repository with no commits** has no `HEAD`. Count every file as an
  insertion and return `branch` from `symbolic-ref`.
- **Remote parsing** accepts these forms of the `origin` URL:
  `https://github.com/o/r(.git)`, `git@github.com:o/r(.git)` and
  `ssh://git@github.com/o/r(.git)`.
  - Owner and repo must match `[A-Za-z0-9_.-]+`.
  - Anything else, including other hosts, extra path segments and userinfo
    other than `git@`, yields `None`.

`/api/open-github` runs `open <url>` exactly as `reveal_in_finder` runs
`open <path>`. It goes through `validate_working_folder` first.

### 3.5 The contextual-action slot

This is one slot with a priority list. This spec implements the first entry
only:

1. Open error findings → *Fix N errors*
2. Open warning findings → *Fix N warnings*
3. *(#35)* Uncommitted accepted changes → *Commit…*
4. *(#37)* Branch ahead of its base → *Create PR*
5. Nothing → hidden

The slot is a split button: a main action plus an optional chevron menu.
#35 and #37 add entries to this list rather than adding new controls. Both
specs get a cross-reference line in this change.

### 3.6 Follow-ups this spec reserves room for

- **Diff view:** a header "changes" icon and a clickable change count both open
  a working-tree diff. The next spec number at the time it is written.
- **Branch menu:** inside #36.
- **Browser and files panels:** separate specs, unnumbered until written.

### 3.7 What gets deleted

From `index.html`:
- `#session-mode-select`
- `#chat-status`, plus `setChatStatus` and its callers; §3.2 gives each
  state's new home
- `#findings-toggle-btn`
- `#session-search-btn`, which moves to the sidebar
- `#open-vscode-btn`
- `#thread-repo`

From `app.js`:
- the session row's `.project-session-delete` and `.project-session-export`
  buttons.

From `style.css`:
- the rules only those elements used.

Grep after removal. No id may be referenced by `app.js` once its element is
gone.

## 4. Acceptance criteria

1. No horizontal `⋯`, no text-glyph overflow trigger, and no `-` or `↓` button
   remains in the shell UI. `grep -n '⋯' crates/desktop-shell/static/` finds
   nothing. The approval and recovery overflow buttons render `.dots-icon`.
   All four ⋮ menus (project, session row, header, folder) open the same
   component.
2. A session can be renamed, exported and deleted from both its sidebar ⋮ and
   the header ⋮. Delete asks for confirmation.
3. The header contains exactly: title, terminal toggle, ⋮. `setChatStatus`
   no longer exists. A turn resumed after approving a command, a plan and a
   fallback offer, and a patch generation, each shows the in-bubble turn
   indicator while running. A turn ending Incomplete, Failed, Stopped or
   budget-exhausted carries a mark on its message after a reload as well as
   live, or says in `tasks.md` which marks are live-only today and why.
4. Session search opens from the sidebar and finds sessions as before.
5. On this repository, the repo bar shows the folder, `main`, and insertion and
   deletion counts that match `git diff --numstat HEAD` plus untracked lines.
6. On a non-git folder, the bar shows the folder only and its menu works. On a
   detached HEAD it shows the short SHA.
7. Unit tests cover `githubUrl` for the three accepted remote forms. They also
   cover rejection of a non-GitHub host, of `github.com.evil.example`, of an
   extra path segment, and of a missing `origin`.
8. Open repository on GitHub opens the reconstructed URL. A request that sends
   a `url` field is ignored. A test asserts the route reads only `repo`.
9. With open error findings, the bar shows *Fix N errors*. Clicking it starts
   one repair turn covering exactly those findings, and the button is disabled
   while that turn runs. With only warnings it shows *Fix N warnings*. With
   none, the slot is empty.
10. The chevron opens the findings list upward. Filters, Dismiss and Fix
    selected behave as in #22.
11. The mode pill switches modes, survives a reload, and snaps back when the
    server rejects the change.
12. Every menu supports arrows and Escape, and Escape returns focus to the
    trigger. Verified by keyboard alone.
13. `UI_STYLE_GUIDE.md` and `docs/ui-style-guide.html` contain no horizontal
    `⋯`. Their overflow guidance and specimens use the vertical `.dots-icon`.
    §8 lists the horizontal glyph as an anti-pattern. The guide documents the
    ⋮ menu and the repo bar. `USER_GUIDE.md`
    describes export, findings and mode in their new places.
14. The seven-check quality gate passes.
