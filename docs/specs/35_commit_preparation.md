# Feature Spec: Commit Preparation

Status: Not started
Order: 35 of 37
Roadmap: `docs/ROADMAP/05_phase_5_delivery_workflows.md`, Phase 5, Work
Package 1 (Must). That directory is local-only and not committed, so the
reference is a name rather than a link; this spec is self-contained.
Related spec sections: `ai_coding_assistant_specification.md` section 7.7 (diff
and patch engine), section 7.4 (command approval), section 7.10 (secret
detection). Related implementation specs:
[`04_hunk_level_patch_apply.md`](04_hunk_level_patch_apply.md) (the per-file and
per-hunk acceptance this commits),
[`07_generated_secret_override.md`](07_generated_secret_override.md) (the
warn-and-override mechanism §5.4 reuses),
[`16_session_checkpoints_and_rewind.md`](16_session_checkpoints_and_rewind.md),
[`17_durable_task_state_and_crash_recovery.md`](17_durable_task_state_and_crash_recovery.md)
(a commit is a side-effecting action with an unknown-outcome window),
[`23_verification_loop.md`](23_verification_loop.md) (check evidence),
[`31_permission_profiles.md`](31_permission_profiles.md) and
[`34_repository_config_trust_boundary.md`](34_repository_config_trust_boundary.md)
(Git mutation is a capability), [`36_branch_and_worktree_delivery.md`](36_branch_and_worktree_delivery.md).

## 1. Motivation

Damaian has never written to a user's Git repository. `git_service.rs` is 141
lines and read-only: `status`, `diff`, and `suggest_commit_message`. Every
requirement in this work package is new code, and all of it crosses from reading
the user's repository to writing it — the largest trust boundary in the roadmap.

The value is concrete. Damaian already applies changes selectively — per file and
per hunk ([spec 04](04_hunk_level_patch_apply.md)) — so it knows exactly what the
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
  [spec 34](34_repository_config_trust_boundary.md) §1, and new commands should
  keep the `-C` form.
- **Acceptance is already selective and already plumbed.**
  `PatchEngine::apply_patch` takes `approved_paths: Option<&[String]>` and
  `hunk_selection: Option<&HashMap<String, Vec<String>>>`
  (`patch_engine.rs:376-384`), and `PatchApplyResult` records excluded hunks.
- **A secret-warning mechanism exists to reuse.**
  `PatchEngine::preview_generated_secrets` (`patch_engine.rs:345`) scans the
  content that would be written and returns `GeneratedSecretWarning { path,
  categories, count }` carrying no matched text, and `apply_patch` gates on
  `allow_generated_secrets` — [spec 07](07_generated_secret_override.md)'s
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

- Pushing, or anything remote — [spec 37](37_pull_request_creation.md).
- Branch creation, merge, or rebase — [spec 36](36_branch_and_worktree_delivery.md).
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

So the commit does not use the repository index at all. Damaian builds the tree
explicitly:

```sh
# A private index file; the user's .git/index is never read or written.
GIT_INDEX_FILE=<tmp> git -C <root> read-tree HEAD
GIT_INDEX_FILE=<tmp> git -C <root> update-index --add --cacheinfo <mode>,<oid>,<path>
GIT_INDEX_FILE=<tmp> git -C <root> write-tree
git -C <root> commit-tree <tree> -p HEAD -m <message>
git -C <root> update-ref refs/heads/<branch> <commit> <expected-old>
```

Blobs for hunk-level selections are written with `git hash-object -w` from the
content Damaian composed, which is the same content-composition path
`prepare_files` (`patch_engine.rs:242`) already uses for partial applies.

Three properties this buys, each mapping to requirement 4:

- **The user's index and working tree are untouched.** Nothing to restore if the
  commit fails, because nothing was changed.
- **`update-ref` with an expected old value is a compare-and-swap.** If `HEAD`
  moved between preparation and commit — a concurrent terminal, a rebase — the
  update fails and nothing happens, rather than committing onto a base that no
  longer exists.
- **A partial commit is unreachable.** The commit object either exists and the
  ref moves, or neither.

This is more machinery than `git add` plus `git commit`, and it is the machinery
that makes requirement 4 true rather than approximately true. The
`git stash`-adjacent approach in
[spec 16](16_session_checkpoints_and_rewind.md) §5.1 uses the same
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
  [spec 07](07_generated_secret_override.md) established for generated patches.
  The override is per commit and is audited.
- States plainly in the approval UI that the diff above is redacted for display
  and the commit will contain the real content. A user who sees `[REDACTED]` and
  is told nothing will reasonably assume the redaction is what gets committed.

This is not hypothetical: a repository with a real `.env`-style value in a
tracked file, or a developer's local token pasted into a config file, produces
exactly this situation. The commit is also the step after which the damage
becomes hard to undo — a secret in Git history survives a later deletion, and
[spec 37](37_pull_request_creation.md) may push it.

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

Because hooks are arbitrary local programs, they are executed through the
existing command path with its timeout, output truncation, and `SecretScanner`
redaction, and their PIDs are registered per
[spec 17](17_durable_task_state_and_crash_recovery.md) §5.7 — a hanging
`pre-commit` must not hang the session, and a crash must not leave it running.

### 5.6 Message suggestion

Requirement 6: extend `suggest_commit_message` (`git_service.rs:110`), do not
replace it. Today it is a template that never sees the diff and always prefixes
`chore:`.

The extension keeps that function as the **deterministic fallback** and adds a
model-backed path in front of it: given the accepted diff and the task's plan
([spec 21](21_task_plan_progress_and_budget.md)), draft a subject and body. When
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
[spec 17](17_durable_task_state_and_crash_recovery.md) §5.3's action markers with
`sideEffecting: true`. A crash in between classifies as
`unknown_external_outcome`, and recovery **does not re-commit**. Resolution is
cheap and specific: compare `HEAD` against the expected old value recorded in the
start marker. If it moved to the prepared commit, the commit succeeded; if not,
it did not. That check is offered as the inspection action rather than performed
automatically, per requirement 5 of that spec.

Requirement 3's SHA is recorded in the finish marker and in the audit log, and
surfaced in the completion report.

### 5.8 Approval and policy

A commit is a Git mutation and therefore a capability, not a convenience:

- It requires explicit approval per commit. There is no `Allow Always` for
  committing — [spec 10](10_persistent_command_approval.md)'s persistent
  approval is exact-command for shell commands, and a standing permission to
  commit would remove the review step that requirement 1 exists to guarantee.
- It is available only in Code mode ([spec 20](20_working_modes.md)) and only
  under a profile that permits Git mutation
  ([spec 31](31_permission_profiles.md)). Ask, Plan, and Review cannot commit.
- Repository config cannot grant it
  ([spec 34](34_repository_config_trust_boundary.md)).
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
- The user's staged work is not committed and their index is unchanged, even
  when they had staged content before the commit — asserted with a fixture that
  stages an unrelated file first.
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
- The five quality-gate commands from `AGENTS.md` pass, and the
  [spec 18](18_local_evaluation_harness.md) baseline shows no regression.

## 7. Implementation Notes

To be completed during implementation. Record:

- Whether the private-index approach in §5.3 worked as specified across the
  fixture cases — in particular a repository mid-rebase, mid-merge, or with a
  detached `HEAD`, where `read-tree HEAD` and `-p HEAD` need care. If any state
  had to be refused rather than handled, say which and why refusing is correct.
- How often the §5.4 secret check fired on real content, and whether it produced
  false positives severe enough to make the override routine. A routine override
  is a defeated control, and [spec 07](07_generated_secret_override.md) exists
  because that already happened once.
- Whether `core.hooksPath` was honoured correctly for repositories using a
  shared hooks directory.
