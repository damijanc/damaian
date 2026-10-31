# Feature Spec: Delete a Worker Session

Status: Done 2026-10-31 (specified and built the same day; local checks in §6,
the full gate is left to CI).
Order: 87 of 87. Number 86 is reserved for the edit-tools spec given to a
Damaian worker the same day.
Plan: none. A defect reported in use, like #73–#85: an exception to the rule
in [`README.md`](README.md).
Depends on: [#57](57_worker_sessions/proposal.md), built.

## 1. Motivation

On 2026-10-31 the user tried to delete a finished worker session. Its ⋯ menu
offered **Stop**, **Reveal worktree in Finder**, **Remove worktree** and
**Dismiss from board**, but no way to delete the session.

The worker's worktree had already been removed outside Damaian, with
`git worktree remove`. **Remove worktree** then failed with "does not exist;
only a worker worktree of … can be removed", so the row could not be cleared
at all.

The shell could already delete a worker session. `/api/session-delete` shuts
a live worker down first, the way #57 stops a deleted parent's workers. Only
the menu lacked the entry.

## 2. Requirements

1. A worker's ⋯ menu, on the board and in the sidebar, has **Delete
   session**. The confirmation says that a running worker is stopped first,
   and that its worktree and branch are kept.
2. Deleting a worker session goes through `/api/session-delete` like any
   other session, and the worker drops off the board and out of the sidebar.
3. **Remove worktree** on a worker whose worktree no longer exists reports
   *already removed* instead of refusing. Git's stale record of it is pruned
   (`git worktree prune`), and the branch is kept.
4. This applies only to a path that would sit directly under the source's
   `.damaian/worktrees/`. A missing path anywhere else is still refused as
   before (#57 §5.1).

## 3. Non-goals

- Removing the worktree or branch with the session. Both stay, as when a
  parent is deleted (#57). The user removes the worktree first if they want
  it gone.
- Deleting a worker's branch. That stays the user's to do (#57 §5.1).

## 4. Design

- **`worktree.rs`:**
  - `remove_worker_worktree` first checks `worker_worktree_is_gone`. When
    that is true, it runs `git worktree prune` and returns the new
    `RemoveOutcome::AlreadyGone`.
  - `worker_worktree_is_gone` is true when nothing exists at the path and its
    parent is the source's `.damaian/worktrees/`.
- **`workers.rs`:** the route answers `AlreadyGone` with
  `{"status":"gone"}`. Nothing is audited, because nothing was removed.
- **`app.js`:**
  - `toggleWorkerMenu` has **Delete session**, which calls
    `deleteWorkerSession`. That function confirms, calls
    `deleteSessionForProject` with the confirmation already given, and clears
    the row's dismissal.
  - `removeWorkerWorktree` toasts the *gone* status.

> **Amended 2026-10-31.** In use, **Delete session** on a worker whose
> worktree was gone failed with "Session … does not belong to repository
> <source>". `delete_session` accepted only the session's own repository, and
> a worker's session is keyed by its worktree. It now also accepts the
> worker's source repository (`WorkerRecord::source_repository`), where the
> `session_end` hooks then run. Any other root is still refused. Pinned by
> `a_worker_session_is_deleted_from_its_source_project` (`tests/hooks.rs`).

## 5. Acceptance

- `removing_a_worktree_that_is_already_gone_prunes_it`
  (`tests/worktree.rs`): the outcome is `AlreadyGone`, git no longer lists
  the worktree, and the branch exists.
- `a_missing_path_outside_the_worker_worktrees_is_still_refused`.

## 6. Verification

Local checks, 2026-10-31:
- `scripts/quality-gate.sh fmt node lint typos deny`;
- `cargo clippy -p workspace-engine -p desktop-shell --all-targets --locked -- -D warnings`;
- `cargo nextest run -p workspace-engine --locked --test worktree`;
- `cargo nextest run -p desktop-shell --locked`.

The workspace clippy and test suite are left to CI.
