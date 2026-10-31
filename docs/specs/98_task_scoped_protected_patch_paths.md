# Feature Spec: Task-Scoped Protected Patch Paths

Status: Not started
Order: 98 of 98
Plan: none. A defect reported in use, like #73–#97.
Depends on: [#20](20_working_modes/proposal.md) (Code-mode patch approval),
[#21](21_task_plan_progress_and_budget/proposal.md) (durable task state),
[#23](23_verification_loop/proposal.md) (repair continuations), and
[#57](57_worker_sessions/proposal.md) (worker sessions), all built.

## 1. Motivation

A worker was told directly not to touch two unrelated files. After a failed
check, its repair turn proposed a one-file patch deleting one of them. The user
rejected it, so nothing was written, but the model had already violated an
explicit task constraint by proposing the edit.

Today a phrase such as “do not touch `crates/desktop-shell/src/lib.rs`” is
prompt text only. A rejection is recorded per patch file, but it neither
becomes a durable no-touch constraint nor prevents a later repair from
proposing the same path. That is the wrong boundary for a user who has named
unrelated work that must remain out of scope.

This spec makes an explicit user path exclusion a task-scoped patch capability:
the engine, not the model, refuses a patch that includes it. It must survive
the patch/verification/repair hand-off and a worker restart.

## 2. Requirements

1. A direct user request may establish protected repository-relative paths.
   The initial syntax is deliberately narrow and deterministic: `do not touch`
   or `do not modify`, followed by one or more backtick-delimited paths. The
   desktop and CLI also provide an explicit protected-path control for paths a
   user wants to add after the task starts. Model output, fetched content,
   repository files, and tool output cannot establish a protected path.
2. Each path is canonicalised and must be a file or directory inside the task's
   repository. A directory protects every descendant. Ambiguous prose and an
   invalid, missing, absolute, or outside-repository path do not silently
   create a constraint; the user sees the reason and can use the explicit
   control instead.
3. Protected paths are durable task state. They carry from a patch task to its
   verification, repair, and ordinary plan continuation, including a worker's
   own process. A new user-started task does not inherit them unless the user
   explicitly sets them again.
4. Every patch-producing route enforces the active task's paths before a patch
   is stored or shown: `edit_file`, `propose_patch`, and the legacy edit flow.
   A patch containing any protected file is refused as a whole, names the
   protected paths, and is returned to the model as a failed tool result so it
   can choose an in-scope alternative. It must not create a review card.
5. Applying a stored patch repeats the check against the task's current
   protected paths. This closes the race where a path becomes protected after a
   preview was made or a recovered card is applied after restart. A partial
   apply may still accept selected unprotected files, but it refuses any
   selected protected file without writing it.
6. A user's rejection of one proposed file is not itself a permanent protected
   path. Rejection can concern the patch's content, hunk, or timing rather than
   every future edit to that file. The rejected decision remains durable review
   evidence and the user can explicitly protect the path when that is intended.
7. The protected-path state and every refusal are visible in the session log,
   worker progress, and the desktop session view. The model receives the active
   exclusions in its task context as a reminder, but enforcement does not rely
   on that reminder.
8. Existing command policy remains unchanged. This is a fence for patch and
   edit-tool file targets, not a claim to sandbox arbitrary shell commands. A
   shell command remains separately approval-gated under the command policy;
   its explicit approval is not inferred from a protected-path setting.
9. Secret redaction, path policy, file-hash patch safety, patch approval,
   recovery, and the one-file rejection flow remain intact.

## 3. Non-goals

- Inferring a permanent write restriction from arbitrary natural language,
  model text, fetched pages, or repository instructions. The narrow direct-user
  syntax and explicit control avoid a false sense of protection.
- Turning a rejected patch file into an implicit ban on all future edits.
- Replacing command approval or providing an operating-system sandbox for
  arbitrary shell commands.
- Changing repository-level `restricted_patterns`, which remain a broader
  policy boundary rather than a task's user-owned scope.
- Retrofitting protected paths into historical tasks that did not record them.

## 4. Design

### 4.1 Durable source and scope

Add a `TaskProtectedPaths` session event keyed by the task that the direct user
request starts. It records canonical repository-relative paths, the user-facing
source (`prompt` or explicit control), and its timestamp, but never model or
repository-supplied text. `SessionStore` resolves the active exclusions for a
task by following its `plan_resumed` / verification continuation ancestry; it
does not inherit them merely because another task belongs to the same session.

The direct-request parser accepts only code spans after the exact phrase `do
not touch` or `do not modify`, splitting multiple spans in the same sentence.
The desktop's **Protect paths** affordance and `damaian protect-paths` use the
same canonicalisation and event writer. A successful set or change is rendered
with the active task and included in worker progress. Removing a protection is
an explicit user action, recorded as a new event; a model cannot remove it.

### 4.2 Patch proposal and application fences

`ChatOrchestrator` resolves the active paths before dispatching `edit_file` and
`propose_patch`; the legacy edit route does the same from its supplied task.
`EditOrchestrator` receives the resolved set and rejects any proposed target
that is the protected path itself or its descendant. The guard sits before
`PatchStore::save`, so no pending patch or preview can misleadingly offer the
file.

`apply_stored_patch` resolves the patch's task protections again before
checkpointing or writing. It checks the selected paths, so a partial review can
apply unrelated selected files while refusing an attempt to include a protected
one. The refusal is a known no-write outcome: it leaves no ambiguous action
marker, no patch decision for the protected path, and no verification due.

The error text is concise and path-specific: “Patch would modify protected task
path: …”. Agentic routes return it through the ordinary tool result. The
desktop card marks only the rejected selection and keeps other pending files
reviewable; the worker stays waiting for an answer rather than silently
continuing around the refusal.

### 4.3 Continuation and recovery

`resume_plan`, `verify_due`, repair turns, `continue_task_after_verification`,
and worker control all resolve the same source task's protections. Recovery
reattaches only a card whose remaining files are not now protected; a stale
card containing a newly protected selected file is shown as refused, not as an
actionable approval. The model-context reminder is generated from the durable
resolved set each turn, so compaction or a fresh worker cannot drop it.

## 5. Acceptance Criteria

Engine coverage proves all of the following:

- A direct user request with `do not touch` and backtick paths records the
  canonical file and directory exclusions; malformed or ambiguous wording
  records none and produces a user-visible reason.
- `edit_file`, `propose_patch`, and the legacy edit route refuse a patch that
  targets a protected file or a child of a protected directory, write no patch
  file, and return the path-specific tool result.
- A multi-file patch with one protected target is refused whole at proposal
  time; after an exclusion is added, a stored multi-file patch accepts selected
  unprotected files and refuses a selected protected file without writing it.
- A fresh `SessionStore` and a restarted worker preserve the exclusions through
  an applied patch, `verification_due`, a repair attempt, and the ordinary
  continuation. A newly typed task in the same session has none.
- The model context and worker progress show the exclusions, but a model cannot
  set, clear, or broaden them.
- Existing partial rejection tests still prove that rejecting a file does not
  itself protect that path, and existing patch hash, secret, path-policy, mode,
  and command-approval tests remain green.
- Desktop and CLI tests cover adding, listing, and explicitly removing paths;
  a reloaded desktop session shows the same protected paths and does not offer
  a patch card containing a newly protected selected file.

## 6. Verification

Implementation must run the fast checks, scoped `workspace-engine`,
`desktop-shell`, and `damaian-cli` Clippy and nextest suites, the deterministic
evaluation tier, and `npm run specs:check`. The workspace-wide Clippy and test
suite remain CI-only under `AGENTS.md`.
