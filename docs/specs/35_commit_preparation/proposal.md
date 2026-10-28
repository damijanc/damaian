# Feature Spec: Commit Preparation

Status: Done (2026-10-26; review fixes 2026-10-28). The full seven-check gate
passed in CI on 2026-10-26 (Quality run 40323902715); scoped checks for the
review fixes passed locally: see `tasks.md`.
Order: 35 of 37
Plan: `docs/PLAN/05_phase_5_delivery_workflows.md`, Phase 5, Work
Package 1 (Must). That directory is local-only and not committed, so the
reference is a name rather than a link; this spec is self-contained.
Depends on: [#16](../16_session_checkpoints_and_rewind.md) (checkpoints) — built;
[#22](../22_findings_model_and_panel/proposal.md) (findings) — built;
[#23](../23_verification_loop/proposal.md) (the verification loop) — built (Done 2026-10-25);
[#31](../31_permission_profiles/proposal.md) (profiles) — built. Everything else
named below is a cross-reference, not a prerequisite.
Also in this spec: [`context.md`](context.md) (pre-implementation state and the
corrections planning found) and [`tasks.md`](tasks.md) (execution order and
progress).
Related spec sections: `ai_coding_assistant_specification.md` section 7.7 (diff
and patch engine), section 7.4 (command approval), section 7.10 (secret
detection). Related implementation specs:
[`58_header_repo_bar_and_menus/proposal.md`](../58_header_repo_bar_and_menus/proposal.md)
(its §3.5 contextual-action slot is where *Commit…* appears in the desktop app),
[`04_hunk_level_patch_apply.md`](../04_hunk_level_patch_apply.md) (the per-file and
per-hunk acceptance this commits),
[`07_generated_secret_override.md`](../07_generated_secret_override.md) (the
warn-and-override mechanism §5.4 reuses),
[`16_session_checkpoints_and_rewind.md`](../16_session_checkpoints_and_rewind.md),
[`17_durable_task_state_and_crash_recovery/proposal.md`](../17_durable_task_state_and_crash_recovery/proposal.md)
(a commit is a side-effecting action with an unknown-outcome window),
[`23_verification_loop/proposal.md`](../23_verification_loop/proposal.md) (check evidence),
[`31_permission_profiles/proposal.md`](../31_permission_profiles/proposal.md) and
[`34_repository_config_trust_boundary.md`](../34_repository_config_trust_boundary.md)
(Git mutation is a capability), [`36_branch_and_worktree_delivery/proposal.md`](../36_branch_and_worktree_delivery/proposal.md).

## 1. Motivation

Damaian has never written to a user's Git repository. When this was written,
`git_service.rs` was 141 lines and read-only: `status`, `diff`, and
`suggest_commit_message`. It is 1,381 lines now, still read-only, because #58 and
#60 widened its read surface (context.md §2.1). Every requirement in this work
package was new code, and all of it crosses from reading the user's repository to
writing it — the largest trust boundary in the roadmap.

The value is concrete. Damaian already applies changes selectively — per file and
per hunk ([spec 04](../04_hunk_level_patch_apply.md)) — so it knows exactly what the
user accepted. Right now that knowledge dies at the working tree: the user is
left to stage and commit by hand, re-deciding what to include with none of the
information Damaian has. Committing exactly the accepted set is a small step
mechanically and removes the step where a rejected change accidentally goes in.

**One risk found while specifying this deserves stating up front, because it
inverts a safety mechanism.** `GitService::diff` redacts its output through
`SecretScanner` before returning it (`git_service.rs:106-107`). That is correct
for showing a diff to a model or a user. It is dangerous as the basis for a
commit approval: the user reviews a diff showing `[REDACTED]` and approves, and
`git commit` then commits the **real** working-tree content, secret included.
Redaction protects what leaves the machine; a commit is the beginning of content
leaving the machine, and the preview that authorised it hid the very thing that
matters. §5.4 handles this.

## 2. Current State

The references below are the ones this decision was written against.
[`context.md`](context.md) §2 has the current ones and the eighteen corrections
planning found; where the two disagree, `context.md` is the measured state and
this section is the history.

- **`GitService` is read-only.** `status` (`git_service.rs:37`), `diff`
  (`git_service.rs:77`), `suggest_commit_message` (`git_service.rs:110`). No
  mutation of any kind exists.
- **`diff` redacts.** `self.scanner.redact(raw_diff.as_ref()).text`
  (`git_service.rs:107`). `status` does not redact — it returns paths.
- **`suggest_commit_message` is a template, not a generator**
  (`git_service.rs:110-119`): it trims the summary and emits
  `chore: <summary>`, or `chore: <path> <summary>` for a single file, or
  `chore: update workspace changes` when the summary is empty. It consults no
  model and never sees the diff.
- **Status parsing already handles the hard cases.** `parse_porcelain`
  (`git_service.rs:122-141`) reads `--porcelain=v1`, resolves renames by taking
  the target of ` -> `, and detects conflicts from the index/worktree pair
  (`AA`, `DD`, `AU`, `UD`, `UA`, `DU`, `UU`), exposing `staged`, `worktree`,
  `untracked`, and `conflicted` per file.
- **Git is invoked with `-C <root>`**, not `current_dir` (`git_service.rs:38-43`,
  `:78-79`), so the program name resolves from `PATH` and the working directory
  is never changed. This avoids the relative-program hazard described in
  [spec 34](../34_repository_config_trust_boundary.md) §1, and new commands should
  keep the `-C` form.
- **Acceptance is already selective and already plumbed.**
  `PatchEngine::apply_patch` takes `approved_paths: Option<&[String]>` and
  `hunk_selection: Option<&HashMap<String, Vec<String>>>`
  (`patch_engine.rs:376-384`), and `PatchApplyResult` records excluded hunks.
- **A secret-warning mechanism exists to reuse.**
  `PatchEngine::preview_generated_secrets` (`patch_engine.rs:345`) scans the
  content that would be written and returns `GeneratedSecretWarning { path,
  categories, count }` carrying no matched text, and `apply_patch` gates on
  `allow_generated_secrets` — [spec 07](../07_generated_secret_override.md)'s
  warn-with-override contract.
- **No Git hook awareness.** Nothing reads `.git/hooks` or reports what a
  `pre-commit` hook might do.
- **No commit exists to report.** There is no code path that produces a SHA.

## 3. Requirements

1. **Never include rejected or unrelated changes.** The commit contains exactly
   what the user accepted, using the per-file and per-hunk distinction
   `patch_engine.rs` already tracks.
2. Repository Git hooks are detected and what they may do is shown before
   committing.
3. The resulting commit SHA is reported.
4. Hook failure and externally changed state leave neither a partial commit nor
   a modified index the user did not ask for.
5. **No amend, reset, or history rewrite** without a separate, explicit,
   action-specific approval.
6. `suggest_commit_message` is extended rather than replaced by a parallel
   generator.

## 4. Non-goals

- Pushing, or anything remote — [spec 37](../37_pull_request_creation.md).
- Branch creation, merge, or rebase — [spec 36](../36_branch_and_worktree_delivery/proposal.md).
- Amend, reset, rebase, cherry-pick, or any history rewrite. Requirement 5
  forbids doing them without dedicated approval; this spec does not implement
  them at all, so the approval flow for them is future work.
- Signing commits, or configuring `user.name`/`user.email`. Damaian commits as
  the repository's existing Git identity and does not set one.
- Conventional-commit enforcement or message linting beyond what a repository's
  own hook does.
- Interactive staging as a general Git UI. Selection is over the accepted change
  set, not arbitrary working-tree content.
- Committing changes Damaian did not make. §5.2 explains why.
- Bypassing hooks. There is deliberately no `--no-verify`.

## 5. Design

### 5.1 The flow

```text
accepted change set → show status and diff → select files or hunks
  → suggest message → detect hooks → run configured pre-commit checks
  → explicit approval → commit → report SHA
```

Each step is separately visible. Only the approval step mutates anything.

### 5.2 Scope: what is even eligible

Requirement 1 says the commit contains exactly what the user accepted. That
requires being explicit about what "accepted" means, because a working tree
contains more than Damaian's changes.

The commit's candidate set is **the paths and hunks Damaian applied and the user
accepted in this task** — known from `apply_patch`'s `approved_paths` and
`hunk_selection` (`patch_engine.rs:376-384`) and recorded per patch. Files the
user changed by hand, and files changed by an approved command, are **shown but
not selected by default**, labelled with why they are present.

Not selected by default, rather than excluded, because a command Damaian ran
legitimately produces committable output — a lockfile update, a generated
schema — and excluding it would force the user out to the terminal. Not selected
by default because the user did not review it as a diff, and requirement 1's
whole point is that the commit is the reviewed set.

A file in both categories — Damaian changed it and then the user edited it — is
labelled as such and not selected by default. Committing an accepted hunk from a
file that has since been hand-edited would commit the hand edit too.

### 5.3 Staging without disturbing the index

Requirement 4 is the awkward one: `git commit` commits the index, and the user's
index is theirs. If they have staged work in progress, staging Damaian's
selection on top and committing would commit their staged work as well, and
resetting afterwards would lose their staging.

So no tree is built from the repository index at all. Damaian builds the tree
explicitly:

```sh
# A private index file; no tree is built from the user's .git/index.
GIT_INDEX_FILE=<tmp> git -C <root> read-tree HEAD
GIT_INDEX_FILE=<tmp> git -C <root> update-index --add --cacheinfo <mode>,<oid>,<path>
GIT_INDEX_FILE=<tmp> git -C <root> write-tree
git -C <root> commit-tree <tree> -p HEAD -m <message>
git -C <root> update-ref refs/heads/<branch> <commit> <expected-old>
# Then the committed paths' entries in the real index are rewritten to the
# new HEAD, only where they still equal the old HEAD's entry.
```

Blobs for hunk-level selections are written with `git hash-object -w` from the
content Damaian composed, which is the same content-composition path
`prepare_files` (`patch_engine.rs:242`) already uses for partial applies.

Four properties this buys, each mapping to requirement 4:

- **A failure leaves the index and working tree untouched.** Everything up to
  `update-ref` reads the repository and writes only Damaian's private state, so
  nothing needs restoring if the commit does not happen.
- **A success leaves the user's staging consistent with the new `HEAD`.** A
  commit that moved the branch while leaving `.git/index` describing the old
  `HEAD` is not harmless: `git status` then shows the commit as a staged
  reversal, and the user's next plain `git commit` stages that reversal —
  reverting Damaian's change and deleting the files it added. So after
  `update-ref` succeeds, each committed path's index entry is rewritten to what
  the commit recorded, exactly as `git commit --only` leaves it. The rewrite is
  conditional: an entry is changed only when it still equals the old `HEAD`'s
  entry for that path. Work the user staged at a different path is untouched,
  and a path the user staged differently is left alone, because that staging is
  theirs. (This was a real data-loss defect found in review; see §7.9, finding
  3a.)
- **`update-ref` with an expected old value is a compare-and-swap.** If `HEAD`
  moved between preparation and commit — a concurrent terminal, a rebase — the
  update fails and nothing happens, rather than committing onto a base that no
  longer exists.
- **A partial commit is unreachable.** The commit object either exists and the
  ref moves, or neither.

Two composition details follow from building the tree from `HEAD` rather than
from the user's index:

- **A rename is one change in two paths.** `git status` reports a rename as the
  target plus the source path; `changes` keeps both. The target carries
  `renamed_from` and the source is a synthesized deletion candidate carrying
  `renamed_to`. Selecting the target without the source deletion is refused
  (`commit_rename_incomplete`), because committing only the target would leave
  the old path behind — a copy the review never showed. A copy record (`C`)
  carries `copied_from` and produces no deletion, because the source stays.
  The dialog moves the pair's checkboxes together; the engine check is the
  backstop.
- **The mode is the working tree's.** A chmod-only change has the same blob id
  as `HEAD`, and a file replaced by a symlink must become a `120000` entry. So
  the mode is `120000` for a symlink, `100755` when executable and `100644`
  otherwise, read from the filesystem (with `core.fileMode=false` falling back to
  `HEAD`'s mode rather than inventing a change Git does not track). The
  "nothing changed" test compares mode as well as blob id, and the commit's
  freshness check compares both.

This is more machinery than `git add` plus `git commit`, and it is the machinery
that makes requirement 4 true rather than approximately true. The
`git stash`-adjacent approach in
[spec 16](../16_session_checkpoints_and_rewind.md) §5.1 uses the same
`GIT_DIR`/index-isolation idea, so the technique is not novel to this spec.

### 5.4 The redacted-preview trap

The risk from §1, stated as a rule: **the diff shown for commit approval is
redacted, and the content committed is not, so the preview alone cannot be the
safety check.**

Before the approval step, Damaian scans the **actual composed content** — the
bytes that will become blobs, not the redacted diff — using the same scanner
call `preview_generated_secrets` makes (`patch_engine.rs:345-360`), and:

- Reports per path the finding categories and counts, never the matched text,
  reusing `GeneratedSecretWarning`'s shape which exists precisely to be safe to
  display.
- Requires an explicit override to proceed, exactly as
  [spec 07](../07_generated_secret_override.md) established for generated patches.
  The override is per commit and is audited.
- States plainly in the approval UI that the diff above is redacted for display
  and the commit will contain the real content. A user who sees `[REDACTED]` and
  is told nothing will reasonably assume the redaction is what gets committed.

This is not hypothetical: a repository with a real `.env`-style value in a
tracked file, or a developer's local token pasted into a config file, produces
exactly this situation. The commit is also the step after which the damage
becomes hard to undo — a secret in Git history survives a later deletion, and
[spec 37](../37_pull_request_creation.md) may push it.

### 5.5 Hooks: detect, describe, run, never bypass

Requirement 2. Before the approval step, enumerate the hooks that a commit would
trigger — `pre-commit`, `prepare-commit-msg`, `commit-msg`, `post-commit` — by
checking `core.hooksPath` and then `.git/hooks`, and report for each: its path,
whether it is executable, its size, and its first-line interpreter.

**The hook body is not summarised or interpreted.** A hook is arbitrary code the
user may not have read, and a confident one-line description of what it does is
exactly the kind of claim that would be wrong at the worst moment. The report
says a hook exists, where it is, and offers to open it — the user reads it, or
accepts that they have not.

Hooks then run for real, because `commit-tree` does **not** run them: the
low-level plumbing in §5.3 bypasses hooks entirely, which would silently defeat a
repository's own protections. So the flow runs them explicitly:

- `pre-commit` runs with the composed content available for inspection, before
  the commit object is created. A non-zero exit **aborts** and its output is
  reported. There is no bypass, no `--no-verify`, and no setting that adds one:
  a user who wants to skip a hook can do it in their own terminal, and Damaian
  offering it would make Damaian the tool of choice for evading a check.
- `commit-msg` runs against the prepared message and may rewrite it; the
  rewritten message is shown before the commit is created.
- `post-commit` runs after, and its failure does not un-commit anything —
  reported as a warning, since the commit has happened and pretending otherwise
  would be worse.
- **A hook that stages its own fix does not get it committed, and the user is
  told.** `pre-commit` commonly reformats files and `git add`s them. The tree was
  composed before the hooks ran, and the commit keeps exactly the bytes the user
  reviewed. After the hooks, Damaian recomputes the private index's tree; if a
  hook changed it, the commit succeeds with the reviewed tree and carries a
  warning naming the hook: its staged changes remain uncommitted in the working
  tree. A hook that leaves the private index unreadable is a warning too, not a
  failed commit. Silently dropping the hook's work, or silently including bytes
  the user never approved, are both worse.

Because hooks are arbitrary local programs, they are executed through the
existing command path with its timeout, output truncation, and `SecretScanner`
redaction, and their PIDs are registered per
[spec 17](../17_durable_task_state_and_crash_recovery/proposal.md) §5.7 — a hanging
`pre-commit` must not hang the session, and a crash must not leave it running.
They run as part of the commit the user approved (`approved: true`), gated by
`allow_git_commits` rather than by a second command approval; whether a profile
that denies commands should refuse to run them is an open decision recorded in
§7.8.

### 5.6 Message suggestion

Requirement 6: extend `suggest_commit_message` (`git_service.rs:110`), do not
replace it. Today it is a template that never sees the diff and always prefixes
`chore:`.

The extension keeps that function as the **deterministic fallback** and adds a
model-backed path in front of it: given the accepted diff and the task's plan
([spec 21](../21_task_plan_progress_and_budget/proposal.md)), draft a subject and body. When
no model is configured, when the call fails, or when the result is empty, the
existing template answers — so a commit is never blocked on a model call.

Two rules on the drafted message:

- **The diff given to the model is the redacted one** (`git_service.rs:107`).
  This is the one place redaction is exactly right, and it is worth noting the
  asymmetry with §5.4: the model sees redacted content, the commit contains real
  content, and the secret check in §5.4 is what covers the gap.
- **The message is always editable before approval**, and the user's edit is
  what is committed. A generated message is a draft, and the commit-message
  field is where a user most reliably notices that the change is not what they
  expected.

### 5.7 A commit is a side-effecting action

`update-ref` either succeeds or does not, but the process can die between
`commit-tree` and `update-ref`, leaving a dangling commit object and no ref move
— harmless, collected by `git gc` — or die after `update-ref` with no record.

So a commit is bracketed by
[spec 17](../17_durable_task_state_and_crash_recovery/proposal.md) §5.3's action markers with
`sideEffecting: true`. A crash in between classifies as
`unknown_external_outcome`, and recovery **does not re-commit**. Resolution is
cheap and specific: compare `HEAD` against the expected old value recorded in the
start marker. If it moved to the prepared commit, the commit succeeded; if not,
it did not. That check is offered as the inspection action rather than performed
automatically, per requirement 5 of that spec.

Requirement 3's SHA is recorded in the finish marker and in the audit log, and
surfaced in the completion report.

`update-ref` is also the one step whose hook is not part of §5.5's flow: Git runs
the repository's `reference-transaction` hook from inside `update-ref`, so it is
not reported, hashed or given a timeout by Damaian. §7.8 records the options.

### 5.8 Approval and policy

A commit is a Git mutation and therefore a capability, not a convenience:

- It requires explicit approval per commit. There is no `Allow Always` for
  committing — [spec 10](../10_persistent_command_approval.md)'s persistent
  approval is exact-command for shell commands, and a standing permission to
  commit would remove the review step that requirement 1 exists to guarantee.
- It is available only in Code mode ([spec 20](../20_working_modes/proposal.md)) and only
  under a profile that permits Git mutation
  ([spec 31](../31_permission_profiles/proposal.md)). Ask, Plan, and Review cannot commit.
- Repository config cannot grant it
  ([spec 34](../34_repository_config_trust_boundary.md)).
- Every commit records path count, hunk count, message length, resulting SHA,
  hook outcomes, and whether a secret override was used, through
  `AuditLog::record`.

### 5.9 Documentation

`docs/USER_GUIDE.md`: what gets included in a commit and why hand-edited files
are shown unselected, what the hook report means, that the displayed diff is
redacted while the commit is not, and that Damaian never bypasses hooks.
`docs/TROUBLESHOOTING.md`: what to do when `update-ref` fails because `HEAD`
moved, how to read a hook failure, how to check whether a commit interrupted by a
crash actually landed, and where the commit appears in the audit log.

## 6. Acceptance Criteria

- A commit created after accepting three of five proposed files contains exactly
  those three — asserted against the commit's tree, not the working tree.
- A commit with hunk-level selection contains exactly the accepted hunks.
- A file the user hand-edited after Damaian's change is shown, labelled, and not
  selected by default.
- A file changed by an approved command is shown, labelled, and not selected by
  default.
- The user's staged work is not committed and stays staged, while the committed
  paths' index entries agree with the new `HEAD`, even when they had staged
  content before the commit — asserted with a fixture that stages an unrelated
  file first, and by a plain `git commit` afterwards that does not revert
  Damaian's commit or delete the files it added.
- A failing `pre-commit` hook aborts the commit and reports the hook's output.
- A `commit-msg` hook that rewrites the message has its result shown before the
  commit is created.
- A failing `post-commit` hook is reported as a warning and does not un-commit.
- There is no code path that passes `--no-verify` or otherwise skips hooks.
- Hooks run with a timeout, their output is truncated and redacted, and their
  PIDs are registered.
- `HEAD` moving between preparation and commit causes the commit to fail with
  nothing changed — asserted by moving `HEAD` between the two steps.
- The repository index and working tree are identical before and after a failed
  commit attempt.
- Composed content containing a seeded fake secret produces a warning with
  categories and counts and **no matched text**, and requires an explicit
  override to commit — the override being audited.
- The approval UI states that the displayed diff is redacted and the commit will
  contain real content.
- A commit is bracketed by side-effecting action markers, and a crash between
  them classifies as `unknown_external_outcome` with no automatic re-commit.
- The resulting SHA is reported and audited.
- `suggest_commit_message` remains the fallback and answers when no model is
  configured or the model call fails.
- The drafted message is editable, and the edited text is what is committed.
- Committing is refused in Ask, Plan, and Review modes, and under a profile that
  denies Git mutation.
- Repository config cannot enable committing.
- No amend, reset, or history-rewriting operation is reachable from this work
  package.
- Every quality-gate command from `AGENTS.md` passes, and the
  [spec 18](../18_local_evaluation_harness/proposal.md) baseline shows no
  regression.

## 7. Implementation Notes

### 7.1 What shipped

A new `workspace-engine/src/commit.rs` module, exposed as
`WorkspaceEngine::commit`, does the work end to end:

- **Candidate set.** The session log's `patch_file_decided` and `commit_created`
  events are walked in order to answer "which accepted changes are not committed
  yet". An accepted path is selected by default only while its current SHA-256
  equals the `applied_hash` the apply recorded. Everything else `git status`
  reports is labelled and unselected: a hand edit after Damaian's change, a
  command effect from the session's checkpoint (`CheckpointOrigin::Command`), a
  legacy patch with no decision, an untracked or external path, or an unresolved
  conflict. `GitService::changes` uses `-uall -z` so an untracked directory
  becomes files, and the same restricted-path filter as every other git read.
  A rename keeps both halves: the target has `renamed_from`, the source is a
  deletion candidate with `renamed_to`, and the pair is selected and refused
  together. A copy (`C`) has `copied_from` and produces no deletion.
- **Private index.** Every tree is built with `GIT_INDEX_FILE` pointing at
  `<data_dir>/commits/<prepare-id>/index`: `read-tree HEAD` (or `--empty` for an
  unborn branch), `hash-object -w`, `update-index -z --index-info`,
  `write-tree`, then `ls-tree -r -z` compared against the full expected map
  (HEAD's tree with the selected additions, modifications and deletions). Only
  then `commit-tree`, a final HEAD check, and
  `update-ref refs/heads/<branch> <commit> <expected-old>`. No tree is built
  from `.git/index`; after the ref moves, the committed paths' entries in the
  real index are rewritten to the new `HEAD` where they still equal the old
  `HEAD`'s entry, as `git commit --only` leaves them. The tree check compares
  the whole tree,
  not only the selected paths, so a composition bug cannot commit an extra
  entry. The mode comes from the working tree (`120000`, `100755`, `100644`),
  so a chmod-only change and a file-to-symlink change are commit as what is
  there; `core.fileMode=false` falls back to HEAD's mode. The "no change" test
  and the commit's freshness check both compare mode and blob id.
- **Exact approval.** The prepared record stores each candidate's Git blob id
  and mode; `commit` recomputes both and refuses `commit_content_changed` on a
  mismatch. The record also stores the expected old `HEAD` and the branch, and
  refuses when either moved. The approval therefore covers the bytes, not the
  redacted preview.
- **Secrets.** The scanner runs on the committed blob, not the working-tree
  file and not the redacted diff: the blob is hashed into a throwaway object
  directory and read back, so a clean or EOL filter cannot make the scanned
  bytes differ from the committed bytes, and an unapproved secret never reaches
  `.git/objects`. Findings become `GeneratedSecretWarning` (path, categories,
  count), a blob over `max_file_bytes` is `unscanned` and needs the override
  too, and `allow_generated_secrets` is a separate request field. The override
  writes `commit_secret_override` with categories and counts; the prepared
  record keeps only the redacted diff.
- **Hooks.** The report honours `core.hooksPath` (relative to the root) and
  otherwise `git rev-parse --git-path hooks`. `pre-commit`,
  `prepare-commit-msg`, `commit-msg` and `post-commit` run in Git's order through
  `CommandRunner::run_at_with_env`, which adds `GIT_INDEX_FILE`, `GIT_DIR` and
  `GIT_WORK_TREE` to the existing timeout, truncation, redaction and PID
  registration. A rewrite of the approved message is a typed refusal with the
  final text and no commit; a failing or timed-out pre-commit, prepare-commit-msg
  or commit-msg aborts; a failing post-commit is a warning after the ref moved.
  There is no `--no-verify` and no parameter that skips a hook. After the
  hooks, the private index's tree is recomputed; if a hook changed it, the
  commit keeps the reviewed tree and a warning names the hook and says its
  staged changes remain uncommitted.
- **Durable state.** Each attempt creates a synthetic task in `RunningTool` and
  a `commit` marker with `sideEffecting: true`. The prepared commit is written to
  the record before `update-ref`, so a crash in that window is inspectable:
  recovery reports the expected old, the prepared commit and the current HEAD,
  and never re-commits. Audit events are `commit_prepared`, `commit_refused`,
  `commit_secret_override` and `commit_created`, with SHA, branch, path/hunk
  counts, message length, hook outcomes and the override flag — never content.
- **Approval and policy.** `allow_git_commits` is the sixth profile capability
  key, classified `Capability` and restrict-only from repository config; the
  Read-only profile turns it off. Committing is refused outside Code mode. The
  shell route requires `approval=commit`; there is no Allow Always.
- **Message.** `GitService::suggest_commit_message` remains the deterministic
  answer, used by `prepare`. `POST /api/commit-message` may draft with a model
  from the redacted diff and the latest plan's step titles; no adapter, a
  refusal, an empty answer or an unparsable one falls back to the template. The
  textarea is the user's, and the edited text is committed. Before comparing a
  hook's rewrite with the approved text, both are cleaned the way `git commit`
  cleans a message (`git stripspace --strip-comments`), because `commit-tree -F`
  applies no cleanup: a hook that appends a `#` comment must not look like a
  rewrite on every attempt.
- **Desktop.** *Commit…* is one entry in #58's contextual-action provider list,
  visible while the session has a candidate. Its dialog shows the per-file
  labels and redacted diffs, the standing redaction statement, the hook report,
  the check evidence, any refusal, the secret override and the message.
  `POST /api/commit` maps `CommitError` to a code the dialog branches on.

### 7.2 Private-index behaviour across states

The approach worked as specified for the normal, unborn, merge, rebase and
detached cases, in the sense that each has a definite answer:

- **Normal branch:** supported; the tree is built from `HEAD` and the ref moves
  with a compare-and-swap.
- **Unborn branch (`HEAD` absent):** supported. The index starts empty
  (`read-tree --empty`), the tree has no parent (`commit-tree` without `-p`), and
  `update-ref` uses an empty old value, which requires the ref not to exist.
- **Detached `HEAD`:** **refused.** `update-ref refs/heads/<branch>` has no
  branch to move; naming `HEAD` directly is a different operation with different
  recovery semantics. Refusing with "check out a branch" is correct rather than
  committing onto a detached checkout.
- **Merge, cherry-pick, revert, rebase or sequencer in progress:**
  **refused.** `read-tree HEAD` and `-p HEAD` would need the merge's second
  parent (or the sequencer's state) to produce the commit Git itself would
  produce. Approximating that is exactly where a partial or wrong commit hides;
  refusing names the marker (`MERGE_HEAD`, `rebase-merge`, …) instead.
- **Unresolved conflicts:** **refused** before composing, because the working
  tree contains conflict markers and the private index does not resolve them.
- **A branch checked out in another worktree:** **refused**, because updating it
  would move a checkout this session is not in.
- **No committer identity:** **refused** with a message to set `user.name` and
  `user.email`; Damaian does not set them.

No test created a real merge or rebase commit; the refusal path is asserted for a
detached HEAD and a `MERGE_HEAD` marker.

### 7.3 The secret check in practice

The deterministic eval tier and the scoped test suite do not exercise a real
user repository, so there is **no measurement of how often the check fires on
real content**. What is measured:

- The seeded fake AWS key in a generated file is flagged with
  `aws_access_key`, count 1, and no matched text in the prepared JSON.
- A file over `max_file_bytes` is `unscanned` and needs the same override,
  rather than being silently committed.
- The override is refused without the separate field, lands with it, is audited
  with categories and counts, and the audit and record contain no key text.

There is therefore no evidence that the override is routine; there is also no
evidence from real use. That is the honest position, and the acceptance table
below marks the live frequency as unmeasured.

### 7.4 `core.hooksPath`

Honoured correctly: a repository with `core.hooksPath=.githooks` reports that
source and the `.githooks/pre-commit` path, and runs it. The default directory is
resolved with `git rev-parse --git-path hooks`, so a linked worktree uses the
hooks Git itself would. A non-executable hook is reported as one Git would skip
and is not run.

### 7.5 Deviations from the written design

1. **`prepare-commit-msg` is run, not only detected.** `commit-tree` skips all
   hooks. Running only `pre-commit`, `commit-msg` and `post-commit` would silently
   defeat a hook Git runs, so the implementation runs all four in Git's order.
   Its rewrite is treated exactly like `commit-msg`'s.
2. **A rewritten message is a second approval, not a silent edit.** The first
   attempt refuses with the final text and creates no commit; the user reviews
   it and approves again. The record stays usable, so the retry does not need a
   new preparation.
3. **The model draft lives in the desktop shell.** The engine holds no model
   adapter; the shell already resolves the provider key and builds one. The
   deterministic template stays in `GitService`.
4. **Each attempt gets a synthetic task.** #17's classifier is task-scoped, and a
   commit usually happens after the turn that proposed the changes has ended, so
   a dedicated `Commit prepared changes` task is what makes a crash visible. It
   makes no model call and carries no plan.
5. **The candidate set is session-wide, not task-wide.** "This task" in §5.2 is
   read as "this session's uncommitted accepted changes": a user who accepts
   files across two turns expects both, and `commit_created` removes them from
   the pending set. The labels still distinguish each path's origin.
6. **No CLI command.** The specification's flow is the desktop shell; the CLI
   has no interactive staging. This is recorded rather than assumed.
7. **The commit route is not streamed.** A commit is short by comparison with a
   turn; hooks are bounded by `command_timeout_secs`, and the dialog shows the
   outcome.

### 7.6 Composition corrections after review

Three defects found while reviewing the finished branch were fixed in place,
and each is covered by a test:

1. **A staged rename was committed as a copy.** `GitService::changes` dropped
   the source record of `R  target\0source\0`, so the tree kept the old path and
   the review never showed the deletion. `GitFileStatus` now carries
   `renamed_from`/`copied_from`; the parser keeps both halves; the preparation
   synthesizes the source deletion for a rename (but not for a copy); the
   dialog moves the pair's checkboxes together; and the engine refuses a
   half-selected rename with `commit_rename_incomplete`. The committed tree is
   now the target only.
2. **A hook that staged a fix had it silently dropped.** The tree is composed
   and verified before the hooks run, and `commit-tree` uses it. After the
   hooks, `write-tree` is recomputed on the private index; a difference is a
   warning naming the hook, and the commit keeps the reviewed bytes. The hook's
   edit stays in the working tree. A hook that leaves the index unreadable is
   also a warning, not a failed commit.
3. **The committed mode came from HEAD.** A chmod-only change was dropped by the
   "nothing changed" check (same blob id), and a file replaced by a symlink was
   written with HEAD's `100644` and the target's bytes as content. The mode now
   comes from the filesystem — `120000` for a symlink, `100755` when
   executable, else `100644` — with `core.fileMode=false` falling back to HEAD's
   mode. The no-change check compares mode as well as blob id; the commit's
   freshness check already did.

### 7.7 Acceptance

Every criterion in §6, with the test or the reason it is not automated. Tests
named without a path are in
`crates/workspace-engine/tests/commit_preparation.rs`; the shell tests are in
`crates/desktop-shell/src/lib.rs` and `src/recovery.rs`.

| Criterion | Evidence |
|---|---|
| Three of five accepted files, asserted against the tree | `a_commit_contains_only_the_accepted_files` |
| Hunk-level acceptance contains exactly the accepted hunks | `a_hunk_level_acceptance_commits_only_the_accepted_hunks` |
| A hand-edited accepted file is shown, labelled, unselected | `a_hand_edited_accepted_file_is_shown_labelled_and_unselected` |
| A command-changed file is shown, labelled, unselected | `a_command_changed_file_is_shown_labelled_and_unselected` |
| Staged work is not committed, stays staged, and the committed entries match the new HEAD | `a_commit_updates_the_committed_paths_in_the_users_index` |
| A plain `git commit` after Damaian's does not revert it or delete its files | `a_plain_commit_after_a_damaian_commit_does_not_revert_it` |
| A failing pre-commit aborts and reports its output | `a_failing_pre_commit_aborts_and_reports_its_output` |
| A commit-msg rewrite is shown before the commit exists | `a_commit_msg_hook_that_rewrites_the_message_is_shown_before_the_commit`, `a_prepare_commit_msg_hook_is_run_and_its_rewrite_is_shown` |
| A failing post-commit is a warning and does not un-commit | `a_failing_post_commit_hook_is_a_warning_and_the_commit_stays` |
| No code path passes `--no-verify` | `there_is_no_hook_bypass` |
| Hooks time out; output is truncated and redacted; PIDs registered | `a_hanging_hook_times_out`, `hook_output_is_truncated_and_redacted`, `a_running_hook_is_registered_with_the_process_registry` |
| HEAD moving fails with nothing changed | `head_moving_between_prepare_and_commit_fails_with_nothing_changed` |
| An accepted deletion commits the file away | `an_accepted_deletion_is_selected_and_commits_the_file_away` |
| A submodule or nested repository is shown and never selected | `a_nested_repository_is_shown_and_can_never_be_selected` |
| A hook edited after the report refuses the commit | `a_same_size_hook_edit_after_the_report_is_refused` |
| A staged rename is prepared as a pair and committed as one | `a_staged_rename_is_prepared_as_a_pair_and_committed_as_one` |
| A copy record produces no deletion | `a_rename_record_keeps_its_source_and_a_copy_record_does_not`, `a_copy_record_carries_no_rename_source` |
| A hook that stages a fix keeps it out of the commit and warns | `a_hook_that_stages_a_fix_keeps_it_out_of_the_commit_and_says_so` |
| A chmod-only change is a candidate and commits `100755` | `a_chmod_only_change_is_a_candidate_and_commits_the_mode` |
| A file replaced by a symlink commits `120000` | `replacing_a_file_with_a_symlink_commits_mode_120000` |
| A chmod after preparation is refused | `a_chmod_after_preparation_is_refused_as_content_changed` |
| Index and working tree identical after a failed attempt | `head_moving_between_prepare_and_commit_fails_with_nothing_changed` |
| A post-ref failure is a warning and the commit is still recorded | `a_staged_copy_that_cannot_be_updated_is_a_warning_after_the_commit` |
| A preparation id cannot traverse out of `commits/` | `a_prepare_id_cannot_escape_the_commit_state_directory` |
| A message hook's comment line is cleanup, not a rewrite | `a_commit_msg_hook_that_only_adds_comments_does_not_block_the_commit` |
| The secret scan reads the committed blob, not the working-tree file | `the_secret_scan_reads_the_blob_that_will_be_committed` |
| Secret warnings have categories/counts and no text; override audited | `a_seeded_secret_warns_with_categories_and_counts_and_no_text`, `a_secret_needs_a_separate_per_commit_override`, `the_recorded_diff_is_redacted_and_the_record_holds_no_secret` |
| The approval UI says the diff is redacted and the commit is not | the dialog's `.commit-redaction` line; **manual, not walked** |
| Action markers and a crash classify as unknown with no re-commit | `a_commit_is_bracketed_by_a_side_effecting_marker`, `a_crash_between_the_markers_is_an_unknown_outcome`, `an_interrupted_commit_reports_the_prepared_commit_and_the_current_head` |
| SHA reported and audited | `the_commit_is_audited_with_counts_and_no_secret_text`, `a_commit_route_creates_the_commit_and_reports_the_sha` |
| `suggest_commit_message` answers when no model is configured or the call fails | `a_failed_or_empty_model_draft_falls_back_to_the_template`, `no_configured_model_falls_back_without_a_call` |
| The drafted message is editable and the edit is committed | `the_commit_is_audited_with_counts_and_no_secret_text` asserts `git log` is the edited text |
| Committing is refused in Ask, Plan and Review and under a profile that denies it | `committing_is_refused_outside_code_mode_and_without_the_profile_capability` (all three modes), `a_commit_is_refused_in_ask_mode` at the route |
| Repository config cannot enable committing | `every_capability_key_resists_weakening_from_repository_scope` |
| No amend, reset or history rewrite | `the_commit_module_has_no_history_rewriting_operation` |
| Every quality-gate command passes and the baseline shows no regression | The full seven-check gate passed in CI on 2026-10-26 (Quality run 40323902715); locally `scripts/quality-gate.sh fmt node lint typos deny`, scoped clippy for `workspace-engine` and `desktop-shell`, `cargo nextest -p workspace-engine --test commit_preparation` (37), `--test permission_profiles` (74 passed, 1 skipped), `--test foundation` (171), `-p desktop-shell --lib` (193 passed, 5 skipped), and the deterministic eval tier |

The one manual gap is the dialog walk: the web UI bootstraps through Tauri
(`damaian_desktop_bootstrap`), and no browser harness was available in this
session. The JS is `node --check`ed and Biome-linted, and every route it calls
is covered by a test; the interaction itself was read, not clicked.

### 7.8 Open decisions

Two review findings are choices for the owner, not defects this change silently
decided. Both concern how repository hooks interact with the capability model.
They are recorded here rather than resolved in code.

1. **`update-ref` runs the repository's `reference-transaction` hook.**
   `update-ref` is the only step that mutates the user's repository, and Git runs
   `reference-transaction` (and its `prepared`/`committed` phases) for it. That
   hook is not part of `GIT_COMMIT_HOOKS`: it is not reported in the hook report,
   its content hash is not stored or compared, and it has no timeout of our own —
   only whatever `git update-ref` itself gives it, which is none. A repository
   therefore has an unreviewed code path that runs at the moment the ref moves.
   The options:
   - **Keep as is and document it.** `update-ref` is the operation Git provides;
     any local `git commit` runs the same hook. The risk is the same one the user
     already accepts in their terminal, and adding a hook report for
     `reference-transaction` would not make it reviewable in time to matter
     (it fires while the ref moves).
   - **Report it like the commit hooks.** Add `reference-transaction` to the
     report with its path and hash, refuse when it changes between preparation
     and commit, and document that it is not run by us but by Git. This closes
     the "unhashed" gap but cannot add a timeout, because the process is Git's
     own child.
   - **Refuse when one exists.** Treat a present `reference-transaction` hook as
     `commit_not_supported` so the user commits from a terminal. Safe, and
     hostile to the repositories that use it legitimately (they are used for
     audit and for locking refs).
   The current implementation is the first option and does not mention the hook
   anywhere the user sees. Whichever option is chosen, the user guide must say
   what actually runs.

2. **Hooks run with `approved: true` even when the profile denies commands.**
   `run_one_hook` passes `CommandRunOptions { approved: true, approved_by:
   Some("git_hook"), … }` so a `pre-commit` hook never raises a command-approval
   card during a commit the user already approved. That is deliberate — a hook
   is part of the operation being approved, and asking again mid-commit for a
   command the user did not write is noise — but it means a profile that denies
   shell commands still executes arbitrary repository code when the user commits.
   The profile gate is `allow_git_commits`; command approval is not consulted.
   The options:
   - **Keep as is with the rationale stated.** Approving a commit is approving
     what a commit does, hooks included; the hook report is shown before the
     approval, and the profile's commit capability is the gate. Document it in
     `SECURITY.md` and the user guide.
   - **Refuse to run hooks when command execution is denied.** If the resolved
     profile turns commands off (`allow_commands=false`, or every command would
     need an approval that a hook cannot obtain), refuse the commit with a
     typed refusal naming the hook and the profile. Safer, and it makes a
     commit impossible in such profiles even when `allow_git_commits` is on;
     repositories with hooks then need a profile that allows them.
   The current implementation is the first option, and it is not stated at the
   point of approval.

### 7.9 Review fixes

A review of the shipped spec found the following. Each entry says what was
confirmed or disproved, and what changed. Finding numbers are the review's.

1. **3a — the user's real index was never updated (confirmed, data loss).**
   Only `update-ref` moved the branch, so `.git/index` still held the old `HEAD`
   at every committed path. After the commit, `git status` showed the commit as
   a staged reversal, and the user's next plain `git commit` committed that
   reversal: Damaian's change reverted and its new files deleted. Confirmed by
   writing the follow-up-commit test first and watching it fail against the old
   code. §5.3's "nothing to restore" sentence was wrong in the same way — it was
   true only for the failure path, not for the committed paths after success.
   The fix rewrites each committed path's real-index entry to the new `HEAD`,
   only where that entry still equals the old `HEAD`'s entry, as `git commit
   --only` leaves the index; a path the user staged differently is untouched.
   Tests: `a_commit_updates_the_committed_paths_in_the_users_index`,
   `a_plain_commit_after_a_damaian_commit_does_not_revert_it`. The old
   `the_users_index_and_working_tree_are_unchanged_by_a_commit` asserted the
   broken state (identical index bytes), so it was replaced rather than kept.
   `USER_GUIDE.md`, `SECURITY.md`, the module doc and §5.3/§6/§7.1 now describe
   the real behaviour.
2. **3b — `prepare_id` was not validated (confirmed).** It flowed into
   `data_dir/commits/<id>.json` and into `private_dir`, which does
   `remove_dir_all`; `../` could therefore read a planted record and delete a
   directory outside `commits/`. Confirmed by planting a valid record at
   `data_dir/victim.json`, creating `data_dir/victim/keep.txt`, and calling
   `commit` with `../victim`: the old code deleted `victim/` and committed.
   `validate_prepare_id` now accepts only `commit_` followed by
   ASCII-alphanumeric/underscore, is enforced in `commit`, `load_record`,
   `save_record` and `private_dir`, and a bad id is `commit_not_prepared`.
   Test: `a_prepare_id_cannot_escape_the_commit_state_directory`.
3. **3c — a post-`update-ref` error marked a real commit as failed
   (confirmed).** `rev-parse HEAD` and `run_post_commit(...)?` propagated a
   `CommitError` after the ref had moved. The caller then set the record to
   `failed`, finished the marker as failed, and skipped `commit_created` and the
   Committed session event. Confirmed with a test that holds `.git/index.lock`:
   with the post-ref steps fatal, `commit` returned `Engine` while HEAD had
   already moved. From `update-ref` onward every step is now a warning on the
   outcome; success is recorded first. Test:
   `a_staged_copy_that_cannot_be_updated_is_a_warning_after_the_commit`
   (asserts the new HEAD, the warning, `state: created` and the audit entry).
   Forcing `rev-parse HEAD` itself to fail deterministically would need a race,
   so its warning path is covered by reading it and by the shared post-ref
   structure, not by its own test.
4. **3d — `reference-transaction` is unreviewed and unbounded** — recorded as an
   open decision in §7.8; no code change.
5. **3e — hooks run with `approved: true` under a command-denying profile** —
   recorded as an open decision in §7.8; no code change.
6. **3f — message cleanup made a hook-added `#` line a permanent rewrite
   (confirmed).** `commit-tree -F` writes the file verbatim, so a `commit-msg`
   hook appending a comment always differed from the approved text: every
   attempt returned `MessageRewritten` and the commit could never be made.
   Confirmed by the comment-hook test failing with exactly that error. Both texts
   are now cleaned with `git stripspace --strip-comments` before comparison, the
   cleaned text is written back to the message file, and an all-comment message
   is refused as empty. A hook that appends real text still requires a second
   approval, as before. Test:
   `a_commit_msg_hook_that_only_adds_comments_does_not_block_the_commit`.
7. **3g — the scan read the file, the commit wrote the filtered blob
   (confirmed).** `hash-object -- <path>` applies clean and EOL filters, so a
   repository with a clean filter could commit bytes the scanner never saw.
   Confirmed with a filter that turns `PLACEHOLDER` into a seeded AWS key: the
   old code committed it with no warning. The blob is now hashed into a
   throwaway object directory and read back (`hash-object -w`,
   `cat-file -s`/`blob`), so the scanned bytes are the committed bytes and an
   unapproved secret never reaches `.git/objects`; a filter-expanded blob over
   `max_file_bytes` is `unscanned`. Test:
   `the_secret_scan_reads_the_blob_that_will_be_committed`.
8. **3h — message spacing and a `GET` with side effects.** The doubled spaces
   from dropped line continuations in five messages (branch changed, hooks
   changed, HEAD read-back, both rename refusals) are fixed. `GET
   /api/commit-preparation` wrote a record and an audit entry; the route is now
   `POST /api/commit-preparation`, with the dialog, the handler doc and the shell
   route tests updated. No known nit was left from this finding.

Checks for this batch: `scripts/quality-gate.sh fmt node lint typos deny`;
`cargo clippy -p workspace-engine --all-targets --locked -- -D warnings`;
`cargo clippy -p desktop-shell --all-targets --locked -- -D warnings`;
`cargo nextest run -p workspace-engine --test commit_preparation` (42);
`cargo nextest run -p desktop-shell --lib` (the commit and recovery tests);
`node --check crates/desktop-shell/static/app.js`; `npm run lint:web`. The
workspace-wide clippy and tests were left to CI.
