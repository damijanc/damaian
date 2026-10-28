# Branch and Worktree Delivery — Context

This file records what the repository looked like when spec 36 became a folder,
what planning found that the flat proposal no longer describes accurately, and
the decisions the tasks implement. It was written on 2026-10-28 against commit
`aaa3be8` in the `spec-36-branch-delivery` worktree. Every line reference below
was checked in that tree; where the proposal and this file disagree, this file is
the measured state and the proposal's §2 is history.

## 1. Why now

[Spec 35](../35_commit_preparation/proposal.md) can now create a commit, but it
commits to whatever branch is already checked out — usually `main`. A user who
asked for a change therefore gets a commit on `main` or has to leave Damaian to
make a branch first. The missing pieces are small and dangerous in specific
places: branch creation is nearly harmless, deleting a branch with unmerged
commits destroys work, and "prepare for merge" that silently merged would be the
worst possible behaviour in this phase.

The feature is also where a read-only report and a mutation must be provably
different code paths. `git merge-tree --write-tree` computes a merge without
touching the index, the working tree, or `HEAD`, so "preparation is not
execution" becomes a property of the command rather than a discipline.

## 2. Pre-implementation state

`crates/workspace-engine/src/git_service.rs` is **1,566 lines**, not the 141 the
proposal's §1 says. It grew through #58 (repo-bar summary), #60 (worktree reads,
diff options, read caps), #64 (path restrictions) and #35's `changes` read. It
is still read-only: there is no branch, ref, merge, or rebase mutation in
`GitService`.

| What | Where today |
|---|---|
| `GitService::status` / `status_in` | `git_service.rs:326`, `:369` |
| `GitService::changes` (uncapped, `-uall -z`) | `git_service.rs:340` |
| `GitService::summary` (branch, short HEAD, counts, GitHub origin) | `git_service.rs:432`, `:457` |
| `GitService::diff` / `diff_in` | `git_service.rs:507`, `:537` |
| Private `merge_base(base, HEAD)` | `git_service.rs:661` |
| `GitService::worktrees` (`worktree list --porcelain -z`) | `git_service.rs:753` |
| Private `ahead_behind(base, HEAD)` | `git_service.rs:785` |
| `GitService::list_worktrees` | `git_service.rs:836` |
| `GitService::log_in` | `git_service.rs:906` |
| `suggest_commit_message` | `git_service.rs:1047` |
| `read_git` (`--no-optional-locks`, fsmonitor and index-refresh off, `-C <root>`) | `git_service.rs:229` |
| `validate_base` (ref-name checks before git runs) | `git_service.rs:113` |
| Porcelain parsing, including conflicted pairs | `git_service.rs:1119`, `:1159` |
| `GitFileStatus.worktree` (index-vs-worktree column) | `git_service.rs:12`; parsed `:1149`, `:1177` |
| Branch mutation in `GitService` | **does not exist** |
| Branch mutation elsewhere in the crate | `worktree.rs:112` (`create_worker_worktree`), `:178` (`git worktree add -b`) |
| Worker slug and `damaian/` prefix | `worktree.rs:21`, `:26` |
| Dirty worker-worktree removal with a distinct confirmation | `worktree.rs:210` |
| One git mutation runner, with the exact argv rendered for a record | `checkpoint.rs:1134`, `:1158` |
| Commit's private in-progress-operation list | `commit.rs:1195`, `:1232` |
| `SessionMode` | `mode.rs:14` |
| Commit's mode/profile refusal | `commit.rs:1277`, `:1288` |
| `ProfileCapabilities` | `profile.rs:168` |
| `allow_git_commits` field / overlay / classification / merge | `config.rs:312`, `:2973`, `:3134`, `:1485` |
| Effective-policy label for `allow_git_commits` | `effective_policy.rs:169` |
| Action markers | `session.rs:385`, `:1967`, `:2038` |
| Commit's marker pair and audit | `commit.rs:781`, `:799`, `:825`, `:871` |
| Commit record store (`<data_dir>/commits/`) | `commit.rs:1996`, `:2003` |
| Shell commit routes | `desktop-shell/src/lib.rs:306`–`:309` |
| Recovery's dangling `commit` inspection | `desktop-shell/src/recovery.rs:509`, `:808` |
| Repo-bar contextual-action providers | `static/app.js:1418`, `:1429` |
| Branch label in the repo bar (display-only `<span>`) | `static/index.html:251`; rendered at `static/app.js:1314` |
| Folder menu | `static/app.js:1354` |
| Checkpoint identity (`sha256(canonical path)`) | `hash.rs:113`–`:116`; used at `checkpoint.rs:252`, `:686` |

## 3. Corrections to the proposal's state

1. **`GitService` is 1,566 lines, not 141**, and it reads more than
   `status`/`diff`/`suggest_commit_message`. None of it mutates; that part of §1
   is still true for `GitService`.
2. **Branch information is not wholly absent.** `summary` reads the current
   branch and short `HEAD` (`git_service.rs:457`), `main_worktree_branch` reads
   the main worktree's branch (`:773`), `ahead_behind` already computes both
   counts against a base (`:785`), and `merge_base` already resolves a base's
   merge base with `HEAD` (`:661`). What is missing is one typed `BranchInfo`
   with upstream tracking, detached/in-progress/conflicted aggregation, and a
   public merge-base used for display rather than only for `diff_in`.
3. **"Git runs via `-C <root>` (`git_service.rs:38-43`)" is stale.** Reads begin
   at `read_git` (`git_service.rs:229`), which adds `--no-optional-locks`,
   disables fsmonitor, and disables `diff`'s index refresh. Mutations have a
   separate runner, `checkpoint::run_user_git` (`checkpoint.rs:1134`), with
   `user_git_command_line` (`:1158`) rendering the exact argv; `worktree.rs`
   already creates branches and worktrees through it. Branch work should reuse
   that runner rather than add a third git invocation path.
4. **`GitFileStatus.worktree` is still not about `git worktree`.** It is the
   index-versus-worktree status column (`git_service.rs:12`, parsed `:1149` and
   `:1177`). The collision the proposal warns about is real and unchanged.
5. **Conflict detection is still post-hoc.** `parse_porcelain` recognises the
   seven conflicted pairs (`git_service.rs:1159`–`:1185`), which describe a merge
   that has already happened. Nothing predicts a conflicting merge. `git diff
   --diff-filter=U` can list the currently unmerged paths cheaply, which is what
   `BranchInfo.conflicted_paths` should use rather than the uncapped `changes`.
6. **The crate already creates branches.** `create_worker_worktree`
   (`worktree.rs:112`) runs `git worktree add -b damaian/<slug> <path> <commit>`
   (`:178`) through `run_user_git`, and `remove_worker_worktree` (`:210`) already
   refuses a dirty worktree without a distinct confirmation, passing `--force`
   only after it (`:225`–`:229`). So the proposal's "no branch operation exists"
   is true of `GitService` only; spec 57's worker worktrees are a built branch
   and worktree facility with a narrower naming rule.
7. **§5.7 is already partly satisfied by built code.** `GitService::worktrees`
   (`:753`) and `list_worktrees` (`:836`) report each registered worktree's path,
   `HEAD`, branch, changed-file count, and ahead/behind against a base;
   `worktree_access` classifies reachable roots; worker records carry
   `worktree_path` and `branch` (`session.rs`'s worker record type). What §5.7
   still asks for is reporting and an externally-deleted-worktree message, not
   creating worktrees. Phase 2 WP4 remains unspecified as a product feature, so
   the conditional section stays conditional — but a later reader should not
   conclude no worktree support exists.
8. **The checkpoint key citation moved.** `repository_id` is
   `sha256(canonical path)` in `hash.rs:113`–`:116`, and `CheckpointStore` uses
   `repository_id_for_root` at `checkpoint.rs:252` and `:686`. The property §5.7
   relies on — a worktree at a different path is a different key — holds; the
   proposal's `indexer.rs:382-385` no longer points at it.
9. **There is no branch capability.** `ProfileCapabilities` (`profile.rs:168`)
   has six keys: `allow_file_edits`, `command_access`,
   `allow_browser_diagnostics`, `allow_mutating_mcp_tools`, `allow_git_commits`
   and `mcp_enabled`. `allow_git_commits` was added by #35 for commits only.
   Nothing lets a profile or repository refuse branch creation, switching, or
   deletion.
10. **Mode gating is engine-side, not a tool matrix row.** Commit checks
    `SessionMode::Code` in `commit.rs:1288`; `mode.rs`'s `ToolAction` matrix is
    for model tools. Branch operations are user actions, so they need the same
    engine-side check, not a new tool row.
11. **There is no prepared-record or "consumed approval" store for branches.**
    The precedent is `CommitRecord` under `<data_dir>/commits/` (`commit.rs:1996`,
    `:2003`), with `prepare_id`, state, and a record loaded at apply time. An
    approval that can be reused for a different operation needs an equivalent
    durable record whose mutation value is compared, not a flag in the webview.
12. **Action markers exist and are the right bracket.** `SessionStore::
    start_action` (`session.rs:1967`) and `finish_action` (`:2038`) are used by
    commits (`commit.rs:799`, `:825`); the shell's recovery already inspects a
    dangling `commit` action (`recovery.rs:509`, `:808`). A branch record needs
    the same shape and an inspect path that compares refs rather than retrying.
13. **`BranchInfo.head_oid` cannot be a `String`.** An unborn branch has a
    current branch name and no commit. `GitState.head` (`commit.rs:410`) and
    `GitSummary.head` are already `Option`; the branch equivalent must be too, or
    a freshly initialised repository cannot be represented.
14. **`git check-ref-format --branch` is authoritative but not literal.**
    `--branch` expands `@{-N}` and similar syntax. A configured prefix that
    contains `@{` could therefore be interpreted as a previous-branch expression
    instead of validated as text. The prefix must be checked in Rust before the
    composed name reaches git; the composed name is then checked with
    `check-ref-format` as §5.2 requires. Git 2.56's `check-ref-format` also
    rejects `--end-of-options` (it prints usage and exits 129), so the leading-`-`
    protection lives in the Rust prefix check rather than in the git argv.
15. **Branch names can carry a secret.** A suggestion derived from a task title
    or the user's request can contain an API key; slugging preserves alphanumeric
    runs and hyphens, so a key such as `sk-live-...` would become part of a ref
    name and every audit line that records it. The source text must pass through
    `SecretScanner::redact` (`secret_scanner.rs:42`) before slugging, and audit
    entries should record the resulting ref, categories, and counts, never the
    unredacted title.
16. **The repo-bar action slot holds one action.** `repoActionProviders`
    (`app.js:1418`) returns the first non-null action, and Commit… currently
    precedes the #37 placeholder (`:1421`–`:1423`). Adding Branch… as another
    main action would hide Commit… whenever both are available. The branch
    label is a display-only `<span>` (`index.html:251`, `app.js:1314`), and the
    folder menu (`app.js:1354`) is already a place for repository-level
    commands, so branch entry points belong there (or a branch trigger that does
    not consume the action slot), not in the action-provider list ahead of
    Commit….
17. **The proposal's switch command does not switch the working tree.**
    `git symbolic-ref HEAD refs/heads/<name>` moves only `HEAD`. It leaves the
    index and working tree as they were, so unless the target commit is exactly
    the current `HEAD` (the creation-then-switch case), Git reports every
    differing file as a local change and the user is on a branch whose tree they
    do not have. A real switch must run `git switch --no-guess <name>` (or
    `git checkout <name>`), which updates index and working tree and refuses
    when local changes would be overwritten. Creation alone still uses
    `git branch <name> <start>`.
18. **There is no default-branch resolver.** §5.6's unmerged count needs
    `--not <default-branch>`, and nothing in the repository determines a default
    branch today. The decision below records the fallback order.
19. **Config key count tests are explicit.** Adding `git_branch_prefix` changes
    `every_overlay_field_is_classified_exactly_once` and
    `the_preference_keys_are_exactly_spec_34s_free_keys` in
    `tests/permission_profiles.rs`; adding `allow_git_branches` changes the
    capability table and the Read-only profile expectation in the same file.
    These are expected record updates, not incidental test churn.

## 4. Decisions

These are the choices the tasks implement. They are not in the proposal's §5 and
a later session should not re-litigate them without checking the code.

- **D1 — The capability key is `allow_git_branches`.** A separate key from
  `allow_git_commits`, default `true`, classified `Capability`, restrict-only
  from repository config, turned off by the Read-only profile, with the
  effective-policy label "Git branches allowed". Commits and branches are
  separate capabilities because the approval rule is action-specific; renaming
  `allow_git_commits` would break existing user config for no gain.
- **D2 — Branch operations are user actions, not model tools.** They are gated
  in the engine by Code mode plus `allow_git_branches`; no `ToolAction` row is
  added and `chat.rs` is never touched.
- **D3 — Approvals are durable and consumed once.** A prepared `BranchRecord`
  holds the exact `GitMutation` value, the repository, the session, the expected
  ref state, and a state of `prepared`/`applied`/`failed`. `apply` compares the
  approved mutation to the record, refuses a second use, and marks the record
  applied before reporting success. There is no `Allow Always`.
- **D4 — Mutations are bracketed by action markers and audited.** Each attempt
  creates a synthetic task in `RunningTool`, starts a side-effecting action
  (`git_branch_create`, `git_branch_switch`, or `git_branch_delete`), finishes
  it with the outcome, and audits refs, start points, and outcome. Recovery
  inspects by comparing refs; it never retries.
- **D5 — `BranchInfo.head_oid` is `Option<String>`.** `None` on an unborn
  branch; `current` names the branch, `detached` is false, and creation is
  refused until the repository has a commit.
- **D6 — Switching uses `git switch`, not `symbolic-ref`.** Creation is
  `git branch <name> <resolved-start>`; creation-plus-switch and switching an
  existing branch run `git switch --no-guess --no-overwrite-ignore <name>`. The
  start point is resolved with `git rev-parse --verify --end-of-options
  <start>^{commit}` before any command, and reported as the resolved SHA. No
  `-f`/`--force` is ever constructed. `--no-overwrite-ignore` is required: Git's
  default updates ignored files, so a gitignored local file that the target
  branch tracks would be silently replaced, and `git status` would never have
  shown it to D15's dirty check.
- **D7 — §5.7 is reporting-only.** Worktree creation stays with spec 57. This
  spec reads `BranchInfo` from the session's checkout (which may be a worktree),
  reports the mapping via the existing worktree listing, and adds an
  externally-deleted-worktree message where a worker record points at a path git
  no longer lists. It creates, moves, and force-removes no worktree. If a later
  spec implements Phase 2 WP4, this section gains no new mutation.
- **D8 — Branch entry points live in the folder menu.** The repo bar's action
  slot stays Commit…-first; a *Branch…* item in the folder menu opens the dialog.
  The branch label remains display-only unless a later UI spec changes it.
- **D9 — Reuse `checkpoint::run_user_git`.** Branch mutations use the existing
  runner and `user_git_command_line`, so the executed argv and the recorded
  argv cannot drift. No new subprocess helper, and no `CommandRunner` (that path
  is for shell commands with their own approval policy).
- **D10 — Slug from redacted text.** `suggest_branch_name` redacts the source
  with `SecretScanner` before slugging, and never returns or audits the raw
  title. A test seeds a fake key and asserts it is absent from the suggested name
  and the audit line.
- **D11 — Default branch resolution order.** `refs/remotes/origin/HEAD`'s
  symbolic target, then local `main`, then local `master`, then the branch's
  merge base with the current `HEAD`; if none resolves, the unmerged count is
  reported as "could not determine a default branch" and deletion proceeds only
  through the unmerged confirmation path with `-D`, never `-d`. A zero from that
  count is not enough for `-d`: Git checks the branch's upstream, or `HEAD` when
  it has none, so the deletion also asks that question and takes the
  confirmation path when the two disagree (review fix 2b).
- **D12 — Merge preparation parses `merge-tree --write-tree --name-only`.** The
  first line is the resulting tree id; lines after it up to the first blank line
  are conflicted paths; the remainder is informational text and is not shown as
  a claim about the repository. Exit code 1 means conflicts, 0 means clean, and
  any other failure or an unrecognised output shape reports conflict prediction
  as unavailable. The fallback never runs a real merge.
- **D13 — Rebase preparation does not predict conflicts.** It reports the commits
  that would be replayed (`merge-base..HEAD`), the merge base, and the onto ref,
  and states plainly that conflict prediction for a rebase would require
  replaying it. That is honest rather than inventing a merge-tree result that
  does not model sequential replay.
- **D14 — Creation and switching are separate records.** `prepare_create` and
  `prepare_switch` produce different `GitMutation` values, so an approval for
  one cannot apply the other. A create-with-switch flow prepares both and asks
  for both approvals, in that order.
- **D15 — Dirty-switch refusal names paths.** Before switching, compare
  `HEAD..target`'s changed paths with the working tree's changed and untracked
  paths; any intersection is a refusal listing those paths. This is an earlier,
  clearer version of the refusal Git itself would produce, not a replacement for
  Git's check. Ignored files are invisible to that comparison, so D6's
  `--no-overwrite-ignore` is the guard for them.
- **D16 — Review fixes harden the recorded guarantees.** A branch record is
  saved as `applying` before `run_user_git`, so consumption does not depend on
  the shell handling one request at a time. `/api/branch` requires the preparing
  `session_id` and `/api/merge-preparation` requires a known session. Commit
  subjects in merge, rebase and deletion reports pass through `SecretScanner`,
  like suggested names. `git_branch_prefix` may not start with `refs/`, which is
  the full-ref namespace rather than a naming convention. These were found in
  review after the first Done and landed before merge; they change no approval,
  capability or execution boundary.

## 5. What this means for the task order

- Read-only state first (Task 1). It is the input every later task refuses on,
  and it can be tested without touching a ref.
- Naming next (Task 2). It is read-only, it is the only part of §5.2, and the
  creation task needs it.
- The capability key before the engine that consults it (Task 3), following
  spec 35's order.
- Creation and switching together (Task 4): they share the prepared record, the
  approval shape, and the marker/audit path. Deletion is separate (Task 5)
  because its second confirmation is a different flow.
- Merge/rebase preparation (Task 6) is independent of the mutations and is the
  last engine work; it must be provably read-only.
- Shell routes and UI (Task 7) after the engine surface is stable.
- Documentation, acceptance evidence, and close-out (Task 8).

No task edits `crates/workspace-engine/src/chat.rs`. If one appears to need it,
stop and report rather than breaking the parallel `chat.rs` track.

## 6. Open questions and out-of-scope findings

- **Minimum Git for `merge-tree --write-tree`.** The development machine has Git
  2.56.0. The practical minimum and how often the fallback is taken are to be
  measured in Task 6 and recorded in `proposal.md` §7. The fallback must be
  exercised by a test that does not depend on the installed Git version; the
  honest limitation of that substitute belongs in the implementation notes.
- **Push and PR creation are spec 37.** No remote write, credential use, or PR
  API belongs in this spec.
- **Branch-protection information is remote state.** Locally there is nothing to
  enforce; spec 37 reads protection where the remote exposes it.
- **Worktree creation is spec 57.** §5.7 is reporting-only (D7).
- **The spec's `-f`/`--force` acceptance criterion** applies to commands this
  spec constructs. Spec 57's `worktree remove --force` is a different,
  already-built path with its own confirmation and is not reachable from spec
  36's branch commands.
- **`cargo deny check` failed at planning time for a dependency this spec does
  not touch.** On 2026-10-28 it reported RUSTSEC-2026-0414 against
  `pulldown-cmark v0.13.4` (fixed in 0.13.5), reached through
  `workspace-engine`. It was fixed on `main` the same day as its own change
  (`ec9f7ea`, `cargo update -p pulldown-cmark`), and this branch was rebased
  onto that commit, so the check now passes here. Nothing in spec 36's diff
  touches the dependency.
