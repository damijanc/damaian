# Feature Spec: Branch and Worktree Delivery

Status: Not started
Order: 36 of 37
Roadmap: `docs/ROADMAP/05_phase_5_delivery_workflows.md`, Phase 5, Work
Package 2 (Must). That directory is local-only and not committed, so the
reference is a name rather than a link; this spec is self-contained.
Related spec sections: `ai_coding_assistant_specification.md` section 7.4
(command approval), section 7.8 (risk classification and approval). Related
implementation specs:
[`16_session_checkpoints_and_rewind.md`](16_session_checkpoints_and_rewind.md)
(checkpoint scoping when a worktree is in play),
[`17_durable_task_state_and_crash_recovery.md`](17_durable_task_state_and_crash_recovery.md),
[`20_working_modes.md`](20_working_modes.md),
[`31_permission_profiles.md`](31_permission_profiles.md) and
[`34_repository_config_trust_boundary.md`](34_repository_config_trust_boundary.md)
(Git mutation is a capability repository config cannot grant),
[`35_commit_preparation.md`](35_commit_preparation.md) (produces the commits this
delivers), [`37_pull_request_creation.md`](37_pull_request_creation.md).

**Dependency correction.** The roadmap makes this work package depend on Phase 2
WP4 (Worktree Isolation), which is **Should-tier and outside Phase 2's minimum
releasable slice** — so it has no spec and may never ship. Requirement 1's
"worktree-to-branch identity (Phase 2 WP4)" is therefore not a foundation that
can be assumed. This spec is written so branch delivery is complete and useful
with **no worktree support at all**, and treats worktree integration as a
conditional section (§5.7) that activates only if Phase 2 WP4 lands. Branch work
is the Must-tier half; worktrees are the Should-tier half it must not depend on.

## 1. Motivation

[Spec 35](35_commit_preparation.md) can produce a commit. It commits to whatever
branch the repository is already on — which, for a user who opened Damaian on
`main` and asked for a change, is `main`.

That is the wrong default for the common case and there is currently no
alternative, because `git_service.rs` cannot create a branch, cannot report how
far ahead or behind one is, and cannot tell whether a merge would conflict. It is
141 lines of read-only status and diff.

The work is small and the danger is concentrated in a few operations. Creating a
branch is nearly harmless; deleting one with unmerged commits destroys work;
`git push --force` destroys other people's work. The roadmap's rule — each of
those requires its own action-specific approval, and approving one does not
approve another — is the whole design, and this spec's job is to make that
structural rather than a matter of remembering.

The second half is the distinction between preparation and execution.
"Prepare for merge" that quietly merges would be the single worst behaviour in
this phase. Reporting what a merge *would* do is genuinely useful and completely
safe, and the two must not be reachable by the same code path.

## 2. Current State

- **`GitService` is read-only**: `status` (`git_service.rs:37`), `diff`
  (`git_service.rs:77`), `suggest_commit_message` (`git_service.rs:110`). No
  branch, ref, merge, or worktree operation exists.
- **`GitFileStatus.worktree` is not about `git worktree`.** It is the
  index-versus-worktree status column from `--porcelain=v1`
  (`git_service.rs:12`, parsed at `git_service.rs:127`). The roadmap notes this
  and it is worth repeating: the field name collides with the feature name and
  means something unrelated.
- **Conflict detection exists only for files already conflicted.**
  `parse_porcelain` (`git_service.rs:137`) recognises `AA`, `DD`, `AU`, `UD`,
  `UA`, `DU`, `UU` — the state *after* a conflicted merge. Nothing predicts
  whether a merge would conflict.
- **Git runs via `-C <root>`** (`git_service.rs:38-43`), never `current_dir`,
  which new commands should preserve.
- **No branch or ref information at all**: no current branch, no upstream, no
  ahead/behind, no merge-base.
- **Phase 2 WP4 (Worktree Isolation) is unspecified**, Should-tier, and outside
  Phase 2's minimum slice.
- **Checkpoints are repository-scoped.**
  [Spec 16](16_session_checkpoints_and_rewind.md) §5.1 keys the shadow store on
  `repository_id`, which is `sha256(canonical path)`
  (`indexer.rs:382-385`) — so a worktree at a different path is already a
  different key. §5.7 depends on that.

## 3. Requirements

1. Support a suggested branch name; user-approved branch creation;
   worktree-to-branch identity where worktrees exist; ahead and behind status;
   merge-base display; conflict detection; and preparation for merge or rebase.
2. **Merge, rebase, cherry-pick, force operations, and deletion of dirty
   branches or worktrees each require explicit, action-specific approval.**
   Approving a branch creation does not approve a rebase.
3. Preparation is not execution. "Prepare for merge" reports what a merge would
   do; it does not merge.

## 4. Non-goals

- Pushing or any remote write — [spec 37](37_pull_request_creation.md).
- Committing — [spec 35](35_commit_preparation.md).
- **Performing** merge, rebase, or cherry-pick. §5.5 is explicit: this work
  package implements *preparation and reporting* for those operations and does
  not execute any of them. Requirement 2's approval rule is stated so that a
  later work package implementing execution inherits it, and so nothing here can
  drift into performing one.
- Creating worktrees. That is Phase 2 WP4; §5.7 integrates with worktrees if
  they exist and creates none.
- Conflict resolution, three-way merge UI, or `rerere`.
- Submodules. A repository with submodules is handled to the extent Git's own
  commands handle it, and no submodule-specific behaviour is added.
- Branch protection enforcement. Where a remote exposes protection information
  [spec 37](37_pull_request_creation.md) uses it; locally there is nothing to
  enforce.
- Stash management.

## 5. Design

### 5.1 Read-only branch information first

Every requirement-1 item except branch creation is a read. Those come first
because they are safe, they are what makes the rest legible, and they are useful
on their own:

```rust
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BranchInfo {
    /// None when HEAD is detached.
    pub current: Option<String>,
    pub head_oid: String,
    pub detached: bool,
    /// None when the branch has no upstream configured.
    pub upstream: Option<String>,
    pub ahead: u32,
    pub behind: u32,
    /// Mid-merge, mid-rebase, mid-cherry-pick, or mid-bisect.
    pub in_progress_operation: Option<String>,
    pub conflicted_paths: Vec<String>,
}
```

`detached` and `in_progress_operation` are surfaced rather than treated as edge
cases, because both are states in which most of this work package must refuse
rather than proceed — a branch created from a detached `HEAD` mid-rebase is a
mess to explain afterwards. Refusing with a clear reason is the correct behaviour
and needs the state to be visible to do it.

Ahead and behind come from `git rev-list --left-right --count <upstream>...HEAD`,
merge-base from `git merge-base <base> HEAD`. Both are pure reads.

### 5.2 Suggested branch name

Derived from the task's plan title ([spec 21](21_task_plan_progress_and_budget.md))
or the user's original request: lowercased, non-alphanumerics collapsed to `-`,
truncated, and prefixed per a configurable pattern (default
`damaian/<slug>`).

Two mechanical rules, because a rejected ref name at the moment of creation is a
poor experience:

- **Validate through Git, not a regex.** `git check-ref-format --branch <name>`
  is authoritative about what Git will accept; a hand-written pattern will differ
  from it in some corner and be wrong there.
- **Collision is checked and the name is not silently altered.** If the branch
  exists, the user is told and offered the suffixed alternative — rather than
  creating `damaian/fix-retry-2` and leaving them to discover which branch they
  are on.

The name is always editable. It is a suggestion.

### 5.3 Branch creation

The only mutating operation in this spec, and deliberately the least dangerous
one:

```sh
git -C <root> branch <name> <start-point>          # create, do not switch
git -C <root> symbolic-ref HEAD refs/heads/<name>  # switch, only if asked
```

Creating and switching are separate. Creating a branch changes nothing about the
working tree; switching does, and a switch with uncommitted changes carries them
along or refuses depending on state. So:

- **Creation requires approval** and reports the created ref and its start
  point.
- **Switching is a separate decision** in the same flow, refused outright when
  the working tree is dirty in a way that would conflict, with the specific paths
  named.
- Neither is offered while `in_progress_operation` is set or `HEAD` is detached
  without an explicit start point.

`git branch` fails rather than overwriting an existing ref, so creation is
inherently non-destructive — no `--force` is used, and `-f`/`--force` never
appears in any command this spec constructs.

### 5.4 Conflict detection without attempting a merge

Requirement 1's conflict detection and requirement 3's "reports what a merge
would do" are the same mechanism:

```sh
git -C <root> merge-tree --write-tree --name-only <base> <head>
```

`merge-tree` computes the merge in Git's object store and reports conflicted
paths **without touching the index, the working tree, or `HEAD`**. It is the
correct tool for exactly this requirement, and it makes "preparation is not
execution" a property of the command rather than a discipline.

The prepared report contains: the merge base, commits that would come in, files
that would change, conflicted paths if any, and whether the merge would
fast-forward. Nothing about it can mutate the repository.

Where `merge-tree`'s newer `--write-tree` form is unavailable in the user's Git,
the fallback is to report ahead/behind and merge-base only, stating that conflict
prediction is unavailable — rather than falling back to a real merge with
`--no-commit`, which would touch the index and is exactly the confusion this
avoids.

A rebase preparation reports the same information from the rebase's perspective:
the commits that would be replayed and onto what. It replays nothing.

### 5.5 The dangerous operations are named, not implemented

Requirement 2 lists merge, rebase, cherry-pick, force operations, and deletion of
dirty branches or worktrees. This spec implements **none of them** (§4), and says
so in the UI: the prepared report ends with what the user would run, or a pointer
to a future action, not a button that does it.

That is a deliberate scope choice rather than an omission. Each of those
operations needs its own approval, its own preview, its own unknown-outcome
handling, and its own recovery story, and bundling five of them into the same
work package as branch creation is how one of them ends up with a shared
approval. The requirement's rule — approving one does not approve another — is
recorded here so that whoever implements them inherits it, and the approval
machinery is structured for it:

```rust
/// One variant per operation. There is no `GitMutation::Other` and no
/// variant that covers two operations, so an approval can never be reused
/// across them.
pub enum GitMutation {
    CreateBranch { name: String, start_point: String },
    SwitchBranch { name: String },
    DeleteBranch { name: String, unmerged_commits: u32 },
}
```

An approval is granted for a specific `GitMutation` value, not for a category,
and is consumed once. A later work package adding `Merge` or `Rebase` adds a
variant and gets a distinct approval by construction.

### 5.6 Branch deletion

Included because it is the destructive operation most likely to be wanted
immediately after this ships — cleaning up a `damaian/…` branch — and leaving it
out would push users to the terminal for the one case where Damaian's knowledge
of what is unmerged is useful.

Before deleting, compute what would be lost:
`git -C <root> rev-list --count <branch> --not --remotes --not <default-branch>`
plus the subject lines of those commits.

- **A fully merged branch** deletes with ordinary approval naming the ref.
- **A branch with unmerged commits** requires a **distinct** confirmation that
  names the count and lists the commit subjects. Not a second click on the same
  card: a separate confirmation whose text is about losing those specific
  commits.
- **The current branch is never deleted.** Git refuses this anyway; refusing
  earlier with a clearer message is better than surfacing Git's error.
- `git branch -D` is used only after the unmerged confirmation; `git branch -d`
  otherwise, so Git's own safety check remains the backstop when the
  confirmation logic is wrong.

That last point matters: the confirmation is Damaian's protection, and `-d` is
Git's. Using `-D` unconditionally would remove the backstop precisely where a
bug in the unmerged calculation would be most costly.

### 5.7 Worktrees, conditional on Phase 2 WP4

**This section activates only if Phase 2 WP4 ships.** Nothing above depends on
it.

If worktrees exist, requirement 1's worktree-to-branch identity is:

- `git -C <root> worktree list --porcelain` reports each worktree's path, `HEAD`,
  and branch. That mapping is the identity — Damaian records which session uses
  which worktree, and the branch information in §5.1 is read from the worktree the
  session is using rather than the source repository.
- **Checkpoint isolation follows for free.**
  [Spec 16](16_session_checkpoints_and_rewind.md) §5.1 keys the shadow store on
  `repository_id`, which is `sha256(canonical path)` (`indexer.rs:382-385`), and
  a worktree has a different path. So a checkpoint created in a worktree restores
  into that worktree and cannot write into the source repository. The roadmap
  flags this interaction as the most likely place for the two systems to corrupt
  each other; the existing path-keyed identity is what prevents it, and the test
  asserts it rather than assuming it.
- **Deleting a worktree with uncommitted changes** takes the §5.6 treatment: a
  distinct confirmation naming the dirty paths, and `git worktree remove` without
  `--force` unless that confirmation was given.
- **An externally deleted worktree** is detected on next use — the recorded path
  no longer appears in `worktree list` — and reported, not crashed on.

If Phase 2 WP4 does not ship, sessions run in the source repository, branch
creation and switching work as in §5.3, and none of the above is reachable. The
acceptance criteria for this section are marked conditional.

### 5.8 Approval and policy

Every mutation here is a Git mutation and therefore a capability:

- Code mode only ([spec 20](20_working_modes.md)); refused in Ask, Plan, and
  Review.
- Permitted only under a profile allowing Git mutation
  ([spec 31](31_permission_profiles.md)), which repository config cannot grant
  ([spec 34](34_repository_config_trust_boundary.md)).
- No `Allow Always` for any of them.
- Each mutation is bracketed by
  [spec 17](17_durable_task_state_and_crash_recovery.md) §5.3 action markers with
  `sideEffecting: true`. Ref updates are individually atomic, so recovery is a
  read: compare the ref against the start marker's expectation. Branch creation
  interrupted mid-flight either created the ref or did not, and the check says
  which — no automatic retry.
- Audited with the operation, refs involved, and outcome.

### 5.9 Documentation

`docs/USER_GUIDE.md`: branch suggestion and creation, what the merge preparation
report tells you and that it changes nothing, and why deleting a branch with
unmerged commits asks twice. `docs/TROUBLESHOOTING.md`: what a detached `HEAD` or
mid-rebase state prevents and why, what to do when conflict prediction is
unavailable on an older Git, and where branch operations appear in the audit log.

## 6. Acceptance Criteria

- Branch creation requires approval and reports the created ref and its start
  point.
- Creating and switching are separate decisions; creation alone leaves `HEAD`
  unchanged.
- A switch that would be refused by Git for a dirty working tree is refused
  earlier, naming the paths.
- Branch creation and switching are refused while a merge, rebase, cherry-pick,
  or bisect is in progress, and from a detached `HEAD` with no explicit start
  point.
- A suggested name is validated with `git check-ref-format --branch`, and a
  colliding name is reported rather than silently suffixed.
- No command constructed by this work package contains `-f` or `--force`.
- A conflicting merge is detected and reported **without being attempted** —
  asserted by checking that the index, working tree, and `HEAD` are byte-identical
  before and after the preparation.
- The merge preparation report names the merge base, incoming commits, changed
  files, conflicted paths, and whether it would fast-forward.
- On a Git without `merge-tree --write-tree`, conflict prediction reports as
  unavailable and no merge is attempted as a fallback.
- No merge, rebase, or cherry-pick is executable from this work package.
- Deleting a branch with unmerged commits requires a distinct confirmation naming
  the count and the commit subjects, and uses `git branch -d` in every other
  case so Git's own check remains the backstop.
- The current branch cannot be deleted.
- An approval granted for one `GitMutation` value cannot be reused for another —
  asserted by test.
- Each mutation is bracketed by side-effecting action markers, and an interrupted
  creation is resolved by comparing the ref, never by retrying.
- Branch operations are refused in Ask, Plan, and Review modes, and under a
  profile denying Git mutation; repository config cannot enable them.
- **Conditional on Phase 2 WP4**: a session's worktree-to-branch mapping is
  reported; a checkpoint restore in a worktree session touches only that
  worktree and never the source repository; deleting a dirty worktree requires a
  distinct confirmation naming the dirty paths; an externally deleted worktree is
  reported rather than crashed on.
- The five quality-gate commands from `AGENTS.md` pass, and the
  [spec 18](18_local_evaluation_harness.md) baseline shows no regression.

## 7. Implementation Notes

To be completed during implementation. Record:

- The minimum Git version `merge-tree --write-tree` required in practice, and
  how often the fallback path was taken. If most users hit the fallback,
  conflict prediction is effectively absent and that should be known rather than
  assumed working.
- Whether Phase 2 WP4 had shipped, and therefore whether §5.7 was implemented or
  left dormant. If it was left dormant, say so explicitly so a later reader does
  not assume worktree support exists.
- Any repository state that had to be refused rather than handled — detached
  `HEAD`, mid-rebase, submodules — and the message shown for each.
