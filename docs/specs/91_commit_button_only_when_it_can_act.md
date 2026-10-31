# Feature Spec: The Commit Button Only When It Can Act

Status: Done 2026-10-31 (specified and built the same day; local checks in §6,
the full gate is left to CI).
Order: 91 of 91
Plan: none. A defect reported in use, like #73–#90.
Depends on: [#35](35_commit_preparation/proposal.md) and
[#58](58_header_repo_bar_and_menus/proposal.md), built.

## 1. Motivation

On 2026-10-31 the user pressed **Commit…** in the repo bar and "nothing
happens".

- **When no session is open:** `openCommitDialog` returns silently without a
  session, yet `commitAction` offered the button whenever the last commit
  status had candidates.
- **While preparing:** preparing a commit takes seconds here, and the button
  gave no sign of it. A second press opened a second dialog. When the first
  committed, the second could only say "HEAD moved … since you reviewed this
  commit".

## 2. Requirements

1. **Commit…** is shown only while a session is open, and the session has a
   commit candidate.
2. While a commit is being prepared, the button reads **Preparing…** and is
   disabled.
3. Pressing it while a commit dialog is open does nothing more. One dialog at
   a time.

## 3. Non-goals

- How long preparation takes.

## 4. Design

- **`app.js`:**
  - `commitAction` returns nothing without `currentSessionId`.
  - A `commitPreparing` flag sets the label and disables the button through
    `renderRepoAction`.
  - `openCommitDialog` returns early while the flag is set or an
    `.app-dialog-commit` is open. It clears the flag in a `finally`.

## 5. Acceptance

Checked by reading: the change is in the web UI's event handling, which has
no automated tests. `node --check` and the web lint pass.

## 6. Verification

As #88.
