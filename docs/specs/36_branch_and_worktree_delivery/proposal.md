# Feature Spec: Branch and Worktree Delivery

Status: Done 2026-10-28; review fixes landed after that close-out and before
merge. The full seven-check gate has **not** run for this branch yet; scoped
checks passed locally (see `tasks.md` and §7).
Order: 36 of 37
Plan: `docs/PLAN/05_phase_5_delivery_workflows.md`, Phase 5, Work
Package 2 (Must). That directory is local-only and not committed, so the
reference is a name rather than a link; this spec is self-contained.
Also in this spec: [`context.md`](context.md) (pre-implementation state, the
corrections planning found, and the decisions the tasks implement) and
[`tasks.md`](tasks.md) (execution order and progress).
Depends on: [#35](../35_commit_preparation/proposal.md) (commit preparation) — built
(Done 2026-10-26); Phase 2 WP4 (worktree isolation, optional) — **unspecified**,
and §5.7 is written so branch delivery works without it.
Everything else named below is a cross-reference, not a prerequisite.
Related spec sections: `ai_coding_assistant_specification.md` section 7.4
(command approval), section 7.8 (risk classification and approval). Related
implementation specs:
[`16_session_checkpoints_and_rewind.md`](../16_session_checkpoints_and_rewind.md)
(checkpoint scoping when a worktree is in play),
[`17_durable_task_state_and_crash_recovery/proposal.md`](../17_durable_task_state_and_crash_recovery/proposal.md),
[`20_working_modes/proposal.md`](../20_working_modes/proposal.md),
[`31_permission_profiles/proposal.md`](../31_permission_profiles/proposal.md) and
[`34_repository_config_trust_boundary.md`](../34_repository_config_trust_boundary.md)
(Git mutation is a capability repository config cannot grant),
[`35_commit_preparation/proposal.md`](../35_commit_preparation/proposal.md) (produces the commits this
delivers), [`37_pull_request_creation.md`](../37_pull_request_creation.md).

**Dependency correction.** The roadmap makes this work package depend on Phase 2
WP4 (Worktree Isolation), which is **Should-tier and outside Phase 2's minimum
releasable slice** — so it has no spec and may never ship. Requirement 1's
"worktree-to-branch identity (Phase 2 WP4)" is therefore not a foundation that
can be assumed. This spec is written so branch delivery is complete and useful
with **no worktree support at all**, and treats worktree integration as a
conditional section (§5.7) that activates only if Phase 2 WP4 lands. Branch work
is the Must-tier half; worktrees are the Should-tier half it must not depend on.

## 1. Motivation

[Spec 35](../35_commit_preparation/proposal.md) can produce a commit. It commits to whatever
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
  [Spec 16](../16_session_checkpoints_and_rewind.md) §5.1 keys the shadow store on
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

- Pushing or any remote write — [spec 37](../37_pull_request_creation.md).
- Committing — [spec 35](../35_commit_preparation/proposal.md).
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
  [spec 37](../37_pull_request_creation.md) uses it; locally there is nothing to
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

Derived from the task's plan title ([spec 21](../21_task_plan_progress_and_budget/proposal.md))
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
  [Spec 16](../16_session_checkpoints_and_rewind.md) §5.1 keys the shadow store on
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

- Code mode only ([spec 20](../20_working_modes/proposal.md)); refused in Ask, Plan, and
  Review.
- Permitted only under a profile allowing Git mutation
  ([spec 31](../31_permission_profiles/proposal.md)), which repository config cannot grant
  ([spec 34](../34_repository_config_trust_boundary.md)).
- No `Allow Always` for any of them.
- Each mutation is bracketed by
  [spec 17](../17_durable_task_state_and_crash_recovery/proposal.md) §5.3 action markers with
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
- Every quality-gate command from `AGENTS.md` passes, and the
  [spec 18](../18_local_evaluation_harness/proposal.md) baseline shows no
  regression.

## 7. Implementation Notes

### What shipped

The engine surface is `crates/workspace-engine/src/branch.rs` (naming,
`GitMutation`, the prepared-record store under `<data_dir>/branches/`, and the
mutation and report paths) plus additions to `git_service.rs`: `BranchInfo`,
`branch_info`, a public `merge_base`, `check_branch_name`, `local_branches`,
`branch_exists`, `merge_tree` and `diff_names`. `allow_git_branches` and
`git_branch_prefix` are in `config.rs`, with the capability resolved in
`profile.rs` and labelled in `effective_policy.rs`; `WorkspaceEngine` exposes a
`branch` engine.

The shell surface is five routes in `desktop-shell/src/lib.rs` — branch info
(`repo`, `session`; returns `BranchInfo` plus the local branch names),
suggestion (`repo`, `session_id`, optional `source`), prepare
(`operation=create|switch|delete`), the typed apply (`approval=branch`,
`session_id` bound to the preparing record, the prepared `GitMutation` as JSON,
optional `confirm_unmerged=true`), and merge/rebase preparation (`mode=merge|rebase`,
`base`, a known `session`) — plus a folder-menu *Branch…* dialog in `app.js` and
`style.css`. No route accepts a raw command, and no route runs a merge, rebase or
cherry-pick.

Tests: 41 integration tests in `workspace-engine/tests/branch_delivery.rs`,
four `merge-tree` parser unit tests in `branch.rs`, the capability and config
tests in `permission_profiles.rs`, and eleven route tests in `desktop-shell`.

### Worktrees (§5.7)

Phase 2 WP4 had **not** shipped, so §5.7 stayed dormant and the four worktree
criteria in §6 are conditional. Nothing in this work package creates, moves or
removes a worktree, and §5.7 added no new mutation. What exists elsewhere is not
this spec's work: spec 57 creates worker worktrees and refuses a dirty removal
without a second confirmation naming the affected paths; spec 60's
`GitService::worktrees`/`list_worktrees` already reports each worktree's path,
`HEAD` and branch; and `worktree_access.rs` labels a path Git no longer considers
a valid worktree as prunable. This spec added no externally-deleted-worktree
message of its own, so a later reader must not assume the §5.7 checkpoint and
worktree-deletion criteria were implemented here.

### Repository states refused rather than handled

- **Not a repository:** "This folder is not a Git repository."
- **Unknown session:** "Unknown session. Open a session before changing branches."
- **Not Code mode:** "Branch changes are available in Code mode only. This
  session is in <mode> mode."
- **Capability off:** "The permission profile does not allow Git branch changes
  (allow_git_branches=false). Switching mode will not allow it."
- **Merge, rebase, cherry-pick, revert, sequencer or bisect in progress:** "A Git
  operation is in progress (<operation>). Finish or abort it before creating a
  branch." Switching and deleting have the same sentence with their own verb.
  The state comes from the same `branch_info` read, so all six markers are
  refused, not only a conflicted merge.
- **Detached `HEAD` for creation without a start point:** "HEAD is detached. Give
  an explicit start point to create a branch from."
- **Unborn `HEAD`:** "This repository has no commits yet. Commit something before
  creating a branch."
- **Dirty switch:** "Switching to `<name>` would overwrite uncommitted changes
  in: <paths>. Commit or stash them, or switch after the working tree matches."
- **Existing or invalid name:** "A branch named `<name>` already exists; it was
  not changed. Try `<alternative>` instead." or Git's `check-ref-format` reason.
- **Deleting the current branch or a branch checked out in a worktree:** a named
  refusal before Git runs.
- **Stale approval:** "HEAD moved from … to … since you reviewed this. Nothing
  ran; prepare again." A moved ref has the same shape, per operation.
- **Unmerged deletion without the second confirmation:**
  `branch_unmerged_confirmation_required`, carrying the count (or the unknown
  notice) and the subjects; the record stays `prepared`.
- **A deletion Git would not accept with `-d` although the default-branch count
  is zero:** the branch is merged into `main` but not into its upstream or the
  checked-out branch, so the preparation takes the same
  `branch_unmerged_confirmation_required` path and the dialog names the
  reference Git checks. The record stays `prepared`; `-D` is offered only after
  the second card.

Submodules are not refused as a repository. Branch operations behave as Git's
own commands do, and no submodule-specific behaviour was added, as §4 says.

### Deviations from the proposal

- **`BranchInfo.head_oid` is `Option<String>`** rather than `String`, so an
  unborn branch is representable (D5).
- **Switching runs `git switch --no-guess --no-overwrite-ignore`, not
  `symbolic-ref`.** The proposal's command moves only `HEAD`; it would leave the
  index and working tree behind (D6, context §3.17). `--no-overwrite-ignore` is
  the review fix for a gitignored local file that the target branch tracks:
  Git's default replaces it, and porcelain never reported it to the dirty check.
- **The unmerged count uses one `--not`.** The proposal's literal
  `--not --remotes --not <default>` is wrong: a second `--not` toggles the
  revision walk back to positive and counts the default branch's own history.
  The implementation runs `git rev-list --count <branch> --not --remotes
  <default>` and the matching `git log --format=%s`. A zero there is checked
  again against Git's own `-d` reference — the branch's upstream, or `HEAD` when
  it has none — so a branch merged into `main` but not into the checked-out
  branch takes the confirmation path instead of failing `git branch -d`.
- **Commit subjects are redacted.** Merge, rebase and deletion reports pass each
  subject through `SecretScanner`, just as a suggested name passes its source;
  the audit still records counts, never subjects.
- **`DeleteBranch.unmerged_commits` is `Option<u32>`** (D11), so "the default
  branch could not be resolved" is a state rather than a false zero.
- **`apply` takes `confirm_unmerged`**; only that flag turns `git branch -d` into
  `-D`, and only for a branch the preparation found unmerged or uncountable.
- **The prefix is validated in Rust before Git sees the composed name.** Git
  2.56's `check-ref-format` rejects `--end-of-options`, so the leading-`-`
  protection cannot live in its argv (context §3.14). The composed name still
  goes through `git check-ref-format --branch`.
- **Apply re-checks the dirty-switch paths.** The task asked for the check before
  switching; re-checking at apply is stricter because the tree can change
  between review and approval.
- **The approval names the resolved commit.** `GitMutation::CreateBranch`
  carries the resolved SHA; the preparation and record separately report the
  start point the user typed.
- **The preparation refusal is HTTP 409 with the preparation attached**, rather
  than a 200 carrying a refusal field, so the dialog can show the reason, the
  collision alternative and the state in one answer. The apply route sends the
  prepared mutation back as JSON rather than rebuilding it from form fields, so
  the approval names exactly what was reviewed.
- **`branch-info` requires `session`** and returns the local branch names the
  dialog needs for switching and deletion.
- **A branch checked out in any worktree cannot be deleted**, not only the
  current checkout.
- **`inspect` is engine-level.** A dangling branch action can be classified as
  `landed`, `not_landed` or `other_ref_state`, and no path retries. The desktop
  recovery card does not currently offer a branch-specific **Inspect** row, so
  the durable record and session marker are the evidence a user can read.

### Review fixes after the first Done

These landed after the initial close-out and before merge; they change no
approval, capability or execution boundary. The branch has still had no CI run.

- **Ignored-file switch (2a, confirmed).** The switch argv gained
  `--no-overwrite-ignore`, and `switching_never_overwrites_an_ignored_local_file`
  proves Git refuses and the local bytes survive. Before the flag, a target
  branch could silently replace a gitignored file the user had edited.
- **Merged-branch deletion (2b, confirmed).** `plain_delete_report` asks the
  same reference `git branch -d` asks and is re-checked at apply;
  `a_branch_merged_into_main_but_not_into_head_needs_the_confirmation` proves the
  second card is offered rather than the record failing.
- **One-shot state (2c, confirmed by reading).** The record is saved `applying`
  before `run_user_git`; `a_record_that_is_applying_cannot_be_applied_again`
  proves a non-`prepared` record runs nothing. The ordering has no injectable
  pause, so it is verified by reading, not by timing.
- **Route and naming hardening (2d).** `/api/branch` binds `session_id` to the
  record, `/api/merge-preparation` refuses an unknown session, commit subjects
  are scanned before display, and `git_branch_prefix` may not start with
  `refs/`.
- **Documentation reconciliation (2e).** The concurrent session's edits noted in
  the Task 7 row are present once in `698d731`; no duplicate section or data row
  remained uncommitted.

### Task 6: merge and rebase preparation

- **Observed Git version:** 2.56.0. On it,
  `merge-tree --write-tree --name-only <base> HEAD` produced the D12 shape
  exactly: a tree id on the first line, conflicted paths up to the first blank
  line, informational text after it, exit 0 for a clean merge, and exit 1 for
  conflicts. Git's release notes place the rewritten `merge-tree --write-tree`
  at Git 2.38; only 2.56.0 was available here, so the practical floor is
  documented rather than measured on an older Git.
- **Fallback exercise:** the unsupported-`--write-tree` decision is exercised at
  the parser boundary with synthetic Git output (exit 129 and
  `error: unknown option 'write-tree'`), because the installed Git supports the
  flag; the test therefore does not depend on the installed version. The
  end-to-end unavailable path in `prepare_merge` is exercised with unrelated
  histories and an unresolvable base, where no `merge-tree` process runs at all.
  Honest limitation: no Git without `--write-tree` was available, so a real old
  Git never reached `prepare_merge`; the unit test proves the decision and the
  report, not an old binary.
- **How often the fallback was taken:** zero times on this machine. Every
  preparation with a resolvable merge base in the tests took the
  `merge-tree --write-tree` path; only the synthetic parser tests take the
  unavailable branch.

### Acceptance walk

Every criterion in §6 is listed below. Test names without a path are in
`crates/workspace-engine/tests/branch_delivery.rs`; `branch_route_tests` are in
`crates/desktop-shell/src/lib.rs`; `permission_profiles` tests are in
`crates/workspace-engine/tests/permission_profiles.rs`.

| §6 criterion | Status | Evidence |
|---|---|---|
| Branch creation requires approval and reports the created ref and its start point. | Met | `creating_a_branch_leaves_head_and_the_index_unchanged` asserts `created_ref` and the start OID; `a_branch_change_needs_its_own_branch_approval_not_a_commit_one` proves the approval is required. |
| Creating and switching are separate decisions; creation alone leaves `HEAD` unchanged. | Met | `creating_a_branch_leaves_head_and_the_index_unchanged`; `create_then_switch_moves_head_and_the_working_tree` (two separate records and approvals). |
| A switch refused by Git for a dirty working tree is refused earlier, naming the paths. | Met | `a_dirty_switch_names_the_paths_it_would_overwrite`, which names the path at prepare; `apply` re-checks the same paths. |
| Creation and switching are refused during a merge, rebase, cherry-pick or bisect, and from a detached `HEAD` without an explicit start point. | Met | `a_mid_merge_or_detached_head_refuses_creation_with_a_reason` proves the merge and detached cases. Rebase, cherry-pick, revert, sequencer and bisect use the same `in_progress_operation` gate; its marker parsing is asserted only for a real merge plus the clean/unborn states. |
| A suggested name is validated with `git check-ref-format --branch`; a collision is reported, not silently suffixed. | Met | `git_is_authoritative_about_a_branch_name`; `a_suggestion_uses_the_configured_prefix_and_reports_a_collision` (name unchanged, free `-2` offered). |
| No command this work package constructs contains `-f` or `--force`. | Met | The `no_force_flag` assertion is used by the creation, switch and deletion tests, including `a_merged_branch_deletes_with_d_and_no_force`; `-D` appears only after the unmerged confirmation. |
| A conflicting merge is detected and reported without being attempted; index, working tree and `HEAD` are byte-identical before and after. | Met | `an_overlapping_merge_is_reported_as_a_conflict_without_merging`; `merge_and_rebase_preparation_leave_the_index_head_and_status_identical` compares index bytes, `HEAD` and `status --porcelain=v2` through the route. |
| The merge report names the merge base, incoming commits, changed files, conflicted paths and whether it would fast-forward. | Met | `a_clean_merge_is_planned_without_merging`; `a_fast_forward_merge_is_reported_as_fast_forward`; `merge_and_rebase_preparation_leave_the_index_head_and_status_identical`. |
| On a Git without `merge-tree --write-tree`, prediction reports unavailable and no merge is attempted. | Met with a stated limitation | `interpret_merge_tree` unit tests use synthetic exit 129 / `unknown option 'write-tree'` output; `an_unrelated_or_unresolvable_base_reports_prediction_unavailable` proves the end-to-end unavailable report with no `merge-tree` process. No Git older than 2.38 was available, so the decision is proved at the parser and report boundaries, not on an old binary. |
| No merge, rebase or cherry-pick is executable from this work package. | Met | `no_route_executes_a_merge_rebase_or_cherry_pick` (404 for `/api/merge`, `/api/rebase`, `/api/cherry-pick` and variants, with `HEAD`, status and `.git/MERGE_HEAD` unchanged); the engine has only the two report methods and no merge/rebase/cherry-pick command. |
| Deleting a branch with unmerged commits requires a distinct confirmation naming the count and subjects; `git branch -d` is used in every other case. | Met | `an_unmerged_branch_needs_a_distinct_confirmation` (plain approval 409 with count and subjects, `-D` only after `confirm_unmerged`); `a_merged_branch_deletes_with_d_and_no_force`; `an_unknown_default_branch_still_requires_the_unmerged_confirmation`; `a_branch_merged_into_main_but_not_into_head_needs_the_confirmation` (Git's own `-d` reference takes the confirmation path); `the_delete_route_keeps_the_two_step_confirmation`. |
| The current branch cannot be deleted. | Met | `the_current_branch_and_a_checked_out_branch_cannot_be_deleted` (also refuses a branch checked out in another worktree); `the_delete_route_keeps_the_two_step_confirmation` covers the route. |
| An approval for one `GitMutation` cannot be reused for another. | Met | `an_approval_cannot_be_retargeted_and_is_consumed_once`; `a_branch_change_needs_its_own_branch_approval_not_a_commit_one`. |
| Each mutation is bracketed by side-effecting action markers, and an interrupted creation is resolved by comparing the ref, never by retrying. | Met at the engine level | `a_branch_apply_is_bracketed_by_a_marker_pair_and_audited`; `inspect_reports_a_dangling_record_truthfully`; `a_delete_that_already_landed_is_inspected_not_retried`. The desktop recovery card has no branch-specific **Inspect** row; the record and marker are the evidence. |
| Branch operations are refused in Ask, Plan and Review modes and under a profile denying Git mutation; repository config cannot enable them. | Met | `mode_and_capability_refusals_name_the_reason_at_prepare_and_apply` (also at apply); `ask_mode_is_refused_before_a_record_or_a_ref_exists` through the route; `every_capability_key_resists_weakening_from_repository_scope`, `a_repository_may_deny_git_branches_and_may_not_grant_them`, `each_built_in_profile_resolves_to_its_table_row` and `allow_git_branches_is_a_capability_key_with_a_human_name` in `permission_profiles`. |
| **Conditional on Phase 2 WP4:** worktree-to-branch mapping is reported; a checkpoint restore in a worktree touches only that worktree; deleting a dirty worktree needs a distinct confirmation naming the dirty paths; an externally deleted worktree is reported, not crashed on. | Conditional — not this spec's work | Phase 2 WP4 did not ship, so §5.7 is dormant (D7). Spec 60's `GitService::worktrees`/`list_worktrees` reports the mapping, spec 57 owns dirty-worktree removal, and `worktree_access.rs` reports a prunable worktree. Spec 36 added no worktree mutation and no externally-deleted-worktree message, and does not test a worktree checkpoint restore. |
| Every quality-gate command from `AGENTS.md` passes, and the spec 18 baseline shows no regression. | **Outstanding** | Scoped checks passed locally: `scripts/quality-gate.sh fmt node lint typos deny`; `cargo clippy -p workspace-engine -p desktop-shell --all-targets --locked -- -D warnings`; `cargo nextest run -p workspace-engine --test branch_delivery`; `cargo nextest run -p desktop-shell --lib`; `node --check`; `npm run lint:web`. The workspace clippy and nextest gate is left to CI, and no CI run exists for this branch yet, so the full gate has **not** passed. |

### Manual walk

No manual browser walk was performed. The web UI bootstraps through the Tauri
desktop API token, so it cannot be driven from a plain browser in this
environment. The dialog's route contract is covered by `branch_route_tests`;
its rendering was checked by `node --check` and Biome only. The visual layout and
the two-card delete flow are unverified in a running app until someone opens
*Branch…* there.
