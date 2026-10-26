# Commit Preparation — Context

This file records why the work exists and what the code looked like when the
spec became a folder (2026-10-26). [`proposal.md`](proposal.md) is the decision;
this is the starting point and the corrections planning found. Every line
reference was checked against the tree on 2026-10-26.

## 1. Why now

Damaian has never written to a user's Git repository. It applies changes
selectively — per file and per hunk (#04) — and verifies them (#23), so it knows
exactly what the user accepted. That knowledge currently dies at the working
tree: the user stages and commits by hand, re-deciding what to include with none
of the information Damaian has.

The feature is also the first place where Damaian's redaction policy and its
write path meet in a way that can hurt the user. `GitService::diff` redacts its
output before returning it. That is right for a preview shown to a model or a
person, and wrong as the sole basis for a commit approval: the preview says
`[REDACTED]`, the commit writes the real bytes, and the user approved a diff that
hid the thing that matters. §5.4 of the proposal is the rule that closes this.

## 2. Pre-implementation state

`crates/workspace-engine/src/git_service.rs` is **1,381 lines**, not the 141 the
proposal's §1 says. It grew through #58 (repo bar summary), #60 (worktree reads,
diff options, read caps) and #64 (path restrictions). It remains read-only; there
is no commit, add, branch or ref mutation anywhere in the crate.

| What | Where today |
|---|---|
| `status` / `status_in` | `git_service.rs:319`, `:325` |
| `summary` (repo bar) | `git_service.rs:388` |
| `diff` / `diff_in` | `git_service.rs:463`, `:493` |
| Redaction of diff output | `git_service.rs:587` (`self.scanner.redact(raw_diff.as_ref()).text`) |
| `suggest_commit_message` | `git_service.rs:948` |
| `parse_porcelain` | `git_service.rs:1015` |
| `apply_patch` signature | `patch_engine.rs:418` |
| `preview_generated_secrets` | `patch_engine.rs:387` |
| Private `prepare_files` | `patch_engine.rs:284` |
| `reconstruct_content` | `diff.rs:94` |
| Patch-file decisions | `session.rs:71` (`PatchFileDecision`), appended by `edit.rs:888` |
| Checkpoint manifests and origins | `checkpoint.rs:50`, `:137` |
| Git mutation capability | **does not exist** |
| Git hook detection | **does not exist** |
| A commit SHA | **no code path produces one** |

### 2.1 Corrections to the proposal's state

1. **`GitService` is 1,381 lines, not 141**, and its read surface is wider than
   `status`/`diff`/`suggest_commit_message`: `summary`, `diff_in` with
   `DiffOptions`, `worktrees`, `list_worktrees`, `log_in`, read caps and
   path-policy filtering. None of it mutates.
2. **The redaction line moved** from `git_service.rs:106–107` to `:587`, and the
   diff is now capped after redaction (`cap_diff`, `:646`) for #60. The trap is
   unchanged: a diff shown for approval can contain `[REDACTED]` while the file
   on disk contains the real value.
3. **`status` already hides restricted paths** (`porcelain_entry_is_restricted`,
   `:695`; `path_is_restricted`, `:681`) and caps its output. Commit candidates
   therefore cannot include a `.env`-style path that `PathPolicy` restricts. That
   is deliberate: a restricted path is one Damaian may not read, and a commit
   would have to read its bytes to write a blob.
4. **`suggest_commit_message` is still a template, not a generator**
   (`git_service.rs:948–957`): `chore: <summary>`, `chore: <path> <summary>` for
   one file, or `chore: update workspace changes`. It consults no model and never
   sees the diff. Requirement 6 still means "extend, do not replace".
5. **`apply_patch` already gates generated secrets** and returns
   `PatchApplyResult { applied: Vec<PatchedFile>, warnings, hook_warnings, … }`
   (`patch_engine.rs:59`, `:418`). `preview_generated_secrets` returns
   `GeneratedSecretWarning { path, categories, count }` with no matched text
   (`patch_engine.rs:126`, `:387`). §5.4 can reuse both shapes rather than invent
   a second warning type.
6. **The durable record of acceptance is the session log, not the patch file.**
   `PatchFileDecision { task_id, patch_id, paths, applied }` is appended as
   `patch_file_decided` by `edit.rs:888` after every apply or reject. The stored
   patch's `status` is never rewritten. So "the per-file distinction is already
   tracked" is true, but the place to read it is `SessionStore::
   read_all_patch_file_decisions`, not `PatchStore`.
7. **A decision does not record the bytes Damaian wrote.** It has paths and a
   boolean. To detect "Damaian changed it, then the user edited it" — and to
   refuse to commit bytes the user never saw — the decision needs the applied
   hash per path. `PatchApplyResult::applied` already carries exactly that
   (`PatchedFile { path, applied_hash }`), so the fix is to persist what is
   already computed.
8. **A decision does not record which hunks were accepted.** `apply_patch` takes
   `hunk_selection` and reconstructs the accepted content, but the decision only
   names the file. For §5.8's "hunk count" audit and for showing what was
   selected, persist the accepted hunk ids beside the applied hash.
9. **Git mutation is not a capability today.** `ProfileCapabilities`
   (`profile.rs:165`) has five keys: `allow_file_edits`, `command_access`,
   `allow_browser_diagnostics`, `allow_mutating_mcp_tools`, `mcp_enabled`. There
   is no key a profile could use to deny committing. §5.8's "only under a profile
   that permits Git mutation" requires a sixth capability key, classified
   restrict-only for repository config.
10. **There is no model adapter in the engine.** `ChatOrchestrator` receives one
    per call; `WorkspaceEngine` holds none (`workspace_engine.rs:39`). The
    desktop shell resolves the provider key and builds the adapter
    (`chat_model_adapter`, `lib.rs`). Message drafting therefore belongs in the
    shell, calling `suggest_commit_message` for the deterministic fallback
    rather than moving model plumbing into the engine.
11. **`hooks.rs` is not Git hooks.** It is #32's dispatcher for Damaian-configured
    hooks with a verdict protocol (`deny`/`request_approval`/`warn`/`allow`). A
    repository's `.git/hooks/pre-commit` is a different thing: an arbitrary local
    program Git itself would run. It must be enumerated and executed separately,
    and must never be routed through #32's verdict machinery (which could turn a
    refusal into an `allow`).
12. **`CommandRunner` has no environment injection.** It builds the shell command
    at `command_runner.rs:194`/`:207` and spawns at `:254`. A Git hook needs
    `GIT_INDEX_FILE` pointing at the private index, plus `GIT_DIR` and
    `GIT_WORK_TREE`, or a hook that inspects `git diff --cached` sees the user's
    index rather than the commit being prepared.
13. **The repo-bar slot is already a provider array.** `app.js`'s
    `repoActionProviders` has placeholder comments for #35 and #37, and
    `renderRepoAction()` shows the first non-null entry. Adding *Commit…* is one
    provider, not a new control.
14. **Recovery already classifies a dangling side-effecting marker.** The shell's
    recovery sweep (`crates/desktop-shell/src/recovery.rs`) reads the marker's
    action and, for `apply_patch`, loads the stored patch to list files. A
    `commit` action needs its own record read to list files and offer the
    HEAD-comparison inspection; the classifier itself needs no new state.
15. **The application data directory may sit inside a repository.** Test
    fixtures conventionally use `repo.join(".damaian")`. A commit test that reads
    `git status` would then see the fixture's own data directory as untracked
    candidate files. Commit fixtures put the data directory beside the
    repository, not in it.
16. **`GitService::changes` dropped a rename's source path.** `git status
    --porcelain=v1 -z -uall` reports `R  target\0source\0`; the first version of
    `changes` kept only `target` and called it "the one a commit names". The
    preparation therefore showed an addition, the tree kept `source` from
    `HEAD`, and the commit was a copy the reviewer never saw. Both halves are
    now carried: `GitFileStatus` has `renamed_from`/`copied_from`, the target and
    the synthesized source deletion move together, and a copy produces no
    deletion.
17. **A hook that staged its own fix was silently discarded.** The tree is
    composed and verified before the hooks run, and `commit-tree` uses that
    tree. A `pre-commit` that reformats and `git add`s (lint-staged) changed the
    private index, but the commit contained the pre-hook bytes and nothing said
    so. The private tree is now compared after the hooks; a difference is a
    warning, and the commit keeps the reviewed bytes.
18. **The committed mode came from `HEAD`.** `blob_info` took the mode from
    `head_entry`, so a chmod-only change (same blob id) was dropped as
    unchanged, and a file replaced by a symlink was written as `100644` with the
    link target as ordinary content. The mode now comes from the filesystem,
    with `core.fileMode=false` falling back to `HEAD`.

## 3. Decisions the implementation takes from the proposal

These are the points later tasks must not re-derive from the proposal alone.

- **D1 — The candidate set is the session's uncommitted accepted changes.** Walk
  the session log in order: `patch_file_decided` with `applied: true` adds its
  paths; `commit_created` removes its paths. The remaining accepted paths are
  the candidates selected by default, and only while their current bytes still
  match the applied hash. Everything else `git status` reports is shown but
  unselected, labelled by what the session knows: a checkpoint path with origin
  `command`, a checkpoint path with origin `patch` that no longer matches what
  Damaian left, or a path the session never touched.
- **D2 — Commit the bytes that are on disk now, after proving they are the bytes
  the user reviewed.** The draft diff is redacted; the commit is not. The
  prepared record stores each candidate's exact blob identity (Git object id) and
  mode. The commit recomputes them and refuses if any selected path changed.
  There is no window in which the preview and the committed bytes differ.
- **D3 — The private index is the only index involved.** `GIT_INDEX_FILE` points
  at `<data_dir>/commits/<prepare-id>/index`; `read-tree HEAD`, `update-index`,
  `write-tree` and `commit-tree` all run against it. The user's `.git/index` and
  working tree are never read for staging or written. The tree is verified with
  `ls-tree -r` before `commit-tree`, so a bug in composition cannot commit a
  different set.
- **D4 — Hooks run for real, in Git's order, with the private index.** Git's
  plumbing does not run hooks, so `pre-commit`, `prepare-commit-msg`,
  `commit-msg` and `post-commit` are executed explicitly through
  `CommandRunner`'s timeout, truncation, redaction and PID registration. There is
  no `--no-verify` and no parameter that skips one. `post-commit` failure is a
  warning after a real commit.
- **D5 — A message rewritten by a hook is shown before the commit exists.** If
  `prepare-commit-msg` or `commit-msg` changes the approved message, the commit
  refuses with the final text and nothing is created. The user reviews and
  approves again. The second run is deterministic and commits.
- **D6 — The secret check scans composed content, not the diff.** Scan the bytes
  that would become blobs. Report categories and counts only, reuse
  `GeneratedSecretWarning`'s shape, require a separate per-commit override, and
  audit the override. Files too large to scan are treated as unscanned and also
  need the override.
- **D7 — Committing is a typed, per-commit approval.** It has its own request
  field (`approval=commit`), its own audit event, and no "always". A question or
  a command approval never satisfies it. It is available only in Code mode and
  only when the resolved profile capability `allow_git_commits` is true.
- **D8 — A commit is a side-effecting action under #17.** It gets a synthetic
  task in `RunningTool`, a `commit` marker with `sideEffecting: true`, and a
  durable record. A crash between the marker pair classifies as
  `unknown_external_outcome`; recovery does not re-commit. `Inspect` can compare
  HEAD with the record's expected old and prepared commit.
- **D9 — The deterministic message is the fallback, never a blocker.** The shell
  may draft with a model using the redacted diff and the plan; on no key, a
  refusal, an empty answer or a parse failure it uses
  `GitService::suggest_commit_message`. The user's edit is what is committed.
- **D10 — Refusals are typed values.** `CommitError` carries a code the shell
  returns beside the message, so the UI can prefill a rewritten message, show a
  hook's output, or explain that HEAD moved, instead of parsing prose.
- **D11 — A rename is one change in two paths.** The target carries
  `renamed_from`, the source deletion carries `renamed_to`, and selecting one
  without the other is refused with `commit_rename_incomplete`. A copy carries
  `copied_from` and no deletion. `Git status`'s worktree column is handled too
  (` R`, `RM`, `RD` as well as `R `).
- **D12 — A hook's staged work is reported, never silently included or
  dropped.** The commit keeps the reviewed tree. After the hooks, the private
  index is written again; a changed tree adds a warning naming the hook and
  saying its changes remain uncommitted. An unreadable index is a warning, not
  a failed commit.
- **D13 — The mode is the working tree's.** `120000` for a symlink, `100755`
  when executable, else `100644`; `core.fileMode=false` falls back to `HEAD`'s
  mode. The no-change test compares mode and blob id, and the commit freshness
  check compares both.

## 4. What this is not

No branch creation (#36), no push or PR (#37), no amend/reset/rebase/cherry-pick,
no signing, no configured identity, no interactive staging over arbitrary
working-tree content, and no bypass for hooks. The proposal's §4 is the
authority.
