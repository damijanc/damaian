# Feature Spec: Worktree Tasks Go to a Worker

Status: Done 2026-10-30 (specified and built the same day; local checks in §7,
the full gate is left to CI).
Order: 76 of 76
Plan: none. A defect reported in use, like #73–#75: an exception to the rule
in [`README.md`](README.md).
Depends on: [#57](57_worker_sessions/proposal.md) (worker sessions) and
[#60](60_worktree_aware_read_tools/proposal.md) (worktree-aware read tools),
both built.

## 1. Motivation

On 2026-10-30 the user gave Damaian an implementation task that began "Work on
a new branch in a new git worktree, not in the main checkout." The session:

1. ran `git worktree add -b brace-glob-search-content .claude/worktrees/brace-glob HEAD`;
2. read every file from that worktree through #60's `worktree` argument;
3. proposed the change with `edit_file` and the path
   `crates/workspace-engine/src/ignore.rs`.

`edit_file` has no `worktree` argument, so the patch applied to the main
checkout, the one place the user had ruled out. The verification checks it
proposed would have run there too.

#60 §4 lists writing to a worktree as a non-goal and says a patch against a
worktree path "is refused as today". Nothing refused it. A path such as
`.claude/worktrees/x/src/a.rs` resolves inside the repository root, so
`edit_file` would have written straight into the worktree, with no
verification, checkpoint or recovery that knows it is a different checkout.

We weighed three options with the user:

1. Give every write path a `worktree` argument.
2. Let a session switch its checkout.
3. Use what exists.

The user chose the third. #57 worker sessions already run a whole session
inside a worktree of their own.

## 2. Requirements

1. `edit_file` and `propose_patch` refuse a path inside another registered
   worktree of the repository. The refusal comes back to the model as a tool
   result, like any refused edit. It names the worktree and says what to do
   next: with Worker sessions on, hand the task to a worker; with them off,
   tell the user the setting is needed, or ask whether to make the change in
   this checkout.
2. A worker's own checkout is not foreign to it. That holds even though the
   worker is a linked worktree inside the main checkout, whose worktree list
   contains both.
3. The two tools' descriptions say that paths are relative to the session's
   checkout, where the change lands, and that a worktree the read tools reach
   is never written.
4. With Worker sessions on, a non-worker session's system prompt adds
   `WORKTREE_TASK_PARAGRAPH` just before #57's handoff paragraph. With them
   off, the prompt is byte-for-byte unchanged (#57 requirement 1), and a
   worker's prompt never carries it.
5. No write path to a worktree is added. Commands are unchanged: a command
   with a `working_directory` inside a worktree still needs approval like any
   other non-read-only command.

## 3. Non-goals

- `worktree` on `edit_file`, `propose_patch`, `run_command` or verification
  checks, and a session switching its checkout. These are the options the
  user did not choose.
- Re-running the task: the user starts it again, now ending in a worker
  handoff.
- The two other defects the same session showed. After the patch was
  applied, the task's remaining plan steps were never resumed. A verification
  with every check skipped recorded `passed`. Both are separate work.

## 4. Design

`worktree_access::foreign_worktrees(git, policy, root)` lists the registered
worktrees, as canonical path and display pairs, excluding the session's own
checkout and any worktree that contains it.

`ChatOrchestrator::refuse_edit_in_worktree` resolves each edit path with
`PathPolicy::resolve_for_write` and refuses the first one under a foreign
worktree with `ClientError::AccessDenied`. If git cannot list worktrees, there
are none to protect and the edit goes ahead. Both patch arms of the agent
loop run this check before `create_patch`.

The paragraph, verbatim:

> Your edits and commands always apply to this session's checkout. The
> `worktree` argument of the read tools only reads. When the user asks for
> work on a new branch or in a separate git worktree, do not create a worktree
> yourself and do not make the change here: hand the task to a worker in a
> ```worker-prompt block, which runs it in a worktree of its own.

## 5. Acceptance Criteria

- From the main checkout every linked worktree is foreign. From a linked
  worktree, neither it nor the main checkout that contains it is foreign, but
  a sibling worktree is.
- `edit_file` on `.claude/worktrees/a/README.md` returns the refusal, stores
  no patch and leaves the file unchanged. `propose_patch` adding a file under
  `.claude/worktrees/b/` is refused the same way.
- `edit_file` on `README.md` in the session's checkout still proposes a patch.
- With handoffs offered, the system prompt ends with the paragraph and then
  #57's. A worker session's prompt is still the base prompt exactly.

## 6. Implementation Notes

The tests are in `workspace-engine/tests/worktree_reads.rs` (spec 76 block)
and in `chat.rs` `worker_handoff_tests`. The pinned-prompt test re-types the
paragraph from §4, so changing the constant means changing this spec.

## 7. Verification

The `workspace-engine` and `eval-harness` suites passed locally (2,223 tests).
The fast checks and scoped clippy are in the change's hand-back. The workspace
clippy and test suite are left to CI.
