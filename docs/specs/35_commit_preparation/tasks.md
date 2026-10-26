# Commit Preparation — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Implements:** [`proposal.md`](proposal.md) in full · corrections and the
pre-implementation state in [`context.md`](context.md)
**Started:** 2026-10-26 — **Done:** 2026-10-26

## Progress

| Task | State | Notes |
|---|---|---|
| 1 · Folder, links, plan | Done 2026-10-26 | Converted the single file to `proposal.md`, added `context.md` (eighteen corrections, decisions D1–D13) and this plan, updated every link that named the old path (README, #36, #37, #38, #52, #57, #58), and set `In progress` in the proposal, the README current-state bullet, the README implementation-order row and this header. `npm run specs:check` passed: 66 specs, dependency lines and status records agree. Fast checks passed: `scripts/quality-gate.sh fmt node lint typos deny`; `npm ci` was needed first because the worktree had no `node_modules`. |
| 2 · The `allow_git_commits` capability | Done 2026-10-26 | Added the sixth capability key beside `allow_file_edits`: `Config` default `true`, `ConfigOverlay` parse/serialise, an explicit capability classification, a restrict-only merge case, `ProfileCapabilities.allow_git_commits`, `ReadOnly` turns it off, the effective-policy label "Git commits allowed", and `Config::profile_capabilities`. The weakening case in `tests/permission_profiles.rs` proves a repository can only refuse it; `each_built_in_profile_resolves_to_its_table_row` now expects Read-only `false`. Scoped `cargo nextest -p workspace-engine --test permission_profiles` passed (74 passed, 1 skipped); clippy clean; fast checks clean after `cargo fmt`. |
| 3 · Durable acceptance detail and the commit engine | Done 2026-10-26 | `PatchFileDecision` gained `applied_hashes` and `accepted_hunks` (both `#[serde(default)]`); `apply_stored_patch` fills them from `PatchApplyResult::applied` and `hunk_selection`, reject paths leave them empty. `SessionStore` gained `append_commit_created` and `read_change_timeline`, which walks applied decisions and commits in log order. New `workspace-engine/src/commit.rs`: candidate set from that timeline plus checkpoint origins, redacted per-path diffs, deterministic message, verification summary, and `prepare`/`commit` through a private `GIT_INDEX_FILE` with `read-tree`/`hash-object`/`update-index`/`write-tree`/`ls-tree` verification/`commit-tree`/`update-ref <new> <old>`; the record is saved under `<data_dir>/commits/`. `GitService::changes` lists untracked files with `-uall` and `-z`, filtered by the same restricted-path rule. Hooks and the secret scan are implemented in the same module; their tests land in Tasks 4 and 5. Nine tests in `tests/commit_preparation.rs` pass (accepted subset, hunk-level, hand edit, command effect, index untouched, HEAD moved, detached/merge, mode+profile gate, invalid selection); `--test foundation` passed (171). Scoped clippy and the fast checks are clean. Deviations: `prepare` saves a record under the data dir (Damaian state, not the repository); the tree is verified against a full expected map, not only the selected paths; `GitService::changes` is new rather than widening `status`, because `status` collapses untracked directories. **Review fixes:** a rename now carries both halves (`GitFileStatus.renamed_from`/`copied_from`, `CommitCandidate.renamed_from`/`renamed_to`/`copied_from`); the source deletion is synthesized for `R` but not for `C`, and `commit_rename_incomplete` refuses a half-selection. The committed mode now comes from the filesystem (symlink `120000`, executable `100755`, else `100644`, with `core.fileMode=false` falling back to HEAD), and the no-change test compares mode as well as blob id. Tests `a_staged_rename_is_prepared_as_a_pair_and_committed_as_one`, `a_rename_record_keeps_its_source_and_a_copy_record_does_not`, `a_copy_record_carries_no_rename_source`, `a_chmod_only_change_is_a_candidate_and_commits_the_mode` and `replacing_a_file_with_a_symlink_commits_mode_120000`. At close: 37 `commit_preparation` tests. |
| 4 · Git hooks: detect, report, run, never bypass | Done 2026-10-26 | Detection honours `core.hooksPath` (relative to the root) and otherwise `git rev-parse --git-path hooks`; the report names path, existence, execute bit, size, interpreter, whether it runs, and why not. `prepare-commit-msg`, `commit-msg` and `post-commit` run in Git order through `CommandRunner::run_at_with_env`, which adds `GIT_INDEX_FILE`, `GIT_DIR` and `GIT_WORK_TREE` to the existing timeout/truncation/redaction/PID path. A rewrite of the approved message is a typed refusal with the final text and no commit; a non-zero or timed-out pre-commit/prepare-commit-msg/commit-msg aborts; a failing post-commit is a warning after the ref moved. A hook report changed between prepare and commit is refused. Tests (10 new): report metadata including `core.hooksPath`, failing pre-commit, commit-msg rewrite then a second approval, prepare-commit-msg rewrite, post-commit warning, private index seen by `git diff --cached`, timeout, truncation plus redaction of a seeded fake AWS key, no `--no-verify` code path, and PID registration while a hook sleeps. All 19 `commit_preparation` tests pass. Deviation from the plan’s wording: `prepare-commit-msg` is run as well as detected, because not running it would silently defeat a hook Git itself would run; recorded in context D4. **Review fix:** after the hooks, `write-tree` is recomputed on the private index; a changed tree adds a warning naming the hooks and says the commit contains the reviewed bytes while the hook's staged changes remain uncommitted. A hook that leaves the index unreadable is a warning, not a failed commit. Test `a_hook_that_stages_a_fix_keeps_it_out_of_the_commit_and_says_so`. |
| 5 · Exact bytes, the secret check and the per-commit override | Done 2026-10-26 | `commit` recomputes each selected path’s Git blob id and mode and refuses `commit_content_changed` when either differs from the record, before composing anything; restoring the reviewed bytes lets the same preparation commit. The secret scan runs on the bytes that would become blobs, reports categories/counts through `GeneratedSecretWarning` (now `Serialize`/`Deserialize`), treats a file over `max_file_bytes` as `unscanned` but still hashes it so an accepted large file is not mislabelled as hand-edited, and requires the separate `allow_generated_secrets` request field. The override writes `commit_secret_override` with categories/counts and no text, and `commit_created` records `generatedSecretOverride`; the prepared record stores only the redacted diff. Five tests: content changed then the same preparation commits the restored bytes, seeded fake AWS key warning with no text in the serialized preparation, override refused then accepted with a clean audit, unscannable file, and a record that contains neither the key nor an unredacted copy. All 24 `commit_preparation` tests pass. **Review fix:** `a_chmod_after_preparation_is_refused_as_content_changed` covers a mode change after the preview; the freshness check already compared mode, and the mode itself now comes from the working tree (Task 3). |
| 6 · Message drafting with the deterministic fallback | Done 2026-10-26 | `POST /api/commit-message` loads the prepared record, takes the selected paths (or the default selection), and calls `draft_commit_message_with`. The prompt carries the task prompt, the latest plan’s step titles, the selected paths and the **redacted** diffs (capped at 20k characters). The model answer is parsed tolerantly (fences, `Subject:`, `Body:`) into a subject and body; a missing adapter, a thrown error, an empty answer or an unparsable one returns `record.message` — the existing `GitService::suggest_commit_message` template — with `source:"fallback"` and a notice. Five tests: model subject/body, empty answer fallback, no adapter fallback, the request contains the redacted diff and plan titles and not the seeded key, and fenced/prefixed parsing. The dialog’s *Draft with model* button is Task 8; nothing calls the model on prepare, so opening the dialog is free. |
| 7 · Durable action markers, audit and crash inspection | Done 2026-10-26 | Each attempt creates a synthetic `Commit prepared changes` task in `RunningTool`, starts a `commit` marker with `sideEffecting: true` before `read-tree`, and records the selected paths and task id on the record. The prepared commit is written to the record **before** `update-ref`, so a crash in that window is inspectable; the same record’s expected old and prepared commit let `Inspect` decide whether HEAD is the commit, still the old value, or somewhere else, without resolving anything. A known outcome sets the task terminal and finishes the marker; a rewritten message or failed hook leaves the record usable so the user can approve again. `GitService::head_commit` is the full-SHA read. `commit_created` audits SHA, branch, path/hunk counts, message length, hook outcomes and the override flag, and the audit carries no content. `desktop-shell/src/recovery.rs` adds the `commit` object and the record’s paths to the recovered JSON; `app.js`’s Inspect shows the comparison. Tests: marker pair with `sideEffecting:true` and `outcome:"ok"`, dangling commit marker → `UnknownExternalOutcome` with no resume, audit counts/override with no seeded key anywhere, and the recovery payload’s `expectedOld`/`preparedCommit`/`landed`. 27 commit-preparation tests and 14 recovery tests pass. |
| 8 · Shell routes and the *Commit…* dialog | Done 2026-10-26 | Four routes: `GET /api/commit-status` (counts + refusal), `GET /api/commit-preparation` (the full record as JSON), `POST /api/commit` (requires `approval=commit`, maps `CommitError` to `{error, code, message}` with hook output or the rewritten message) and the Task 6 `POST /api/commit-message`. The repo bar fetches status beside `/api/git-status` when a session is open, in parallel; `commitAction` appends to `repoActionProviders` and shows when there is a candidate. The dialog lists every candidate with its label and a per-file redacted diff, states that the diff is redacted and the commit is not, requires the secret override when the real bytes are flagged, shows the hook report and “never skips them”, shows the check evidence and any refusal, prefills the deterministic message, offers *Draft with model*, and submits the typed approval with the selected paths. A rewritten message replaces the textarea and asks for approval again; hook output is shown; HEAD/content/hook changes disable the dialog and say to prepare again; success toasts the short SHA and subject and refreshes the bar. `api()` now attaches the parsed payload to errors so the dialog branches on `code`, not prose. Tests (5 route tests + 5 message tests): status/preparation JSON, missing approval → 400 with no ref, Ask mode → 409, a real commit through the route with the SHA and content, and a rewritten message → 409 with the text and no ref; all 193 shell lib tests pass. **Manual UI walk not run:** the web UI is bootstrapped through Tauri (`damaian_desktop_bootstrap`) and no browser harness was available in this session, so the dialog’s interaction was verified by reading, `node --check` and the route tests rather than by clicking it. |
| 9 · Docs, acceptance walk, close-out | Done 2026-10-26 (CI outstanding) | `proposal.md` §7 records what shipped, the private-index behaviour across normal/unborn/merge/rebase/detached states, the honest limits of the secret-check measurement, `core.hooksPath`, seven deviations and a criterion-to-test table (37 engine tests). `USER_GUIDE.md` gains a Commits section and updates the repo-bar action; `TROUBLESHOOTING.md` adds the `commits/` data files and a refusal/unknown-outcome section; `SECURITY.md` adds the Git Commits guarantees. README current state, history and #35 row are updated and #36 is promoted; #36 and #37 dependency lines now say #35 is built; the CHANGELOG `Unreleased` commit clause is gone. A review of the finished branch found and fixed three composition defects (rename pair, hook-staged changes, mode from the working tree); `proposal.md` §7.6 and `context.md` §2.16–§2.18/D11–D13 record them, and the owning task rows name their tests. One manual gap is stated plainly: the dialog was not walked in a browser because the web UI bootstraps through Tauri and no harness was available. Local checks: `scripts/quality-gate.sh fmt node lint typos deny` clean; scoped clippy for `workspace-engine` and `desktop-shell` clean; `cargo nextest -p workspace-engine --test commit_preparation --test permission_profiles --test crash_recovery` 152 passed, 3 skipped; `-p desktop-shell --lib` 193 passed, 5 skipped; deterministic eval tier all 28 scenarios pass; `npm run specs:check` passed. The seven-check gate is left to CI after the user pushes. |

**Goal:** A Code-mode session can commit exactly the changes Damaian applied and
the user accepted, through a private index that never touches the user's staging
area, with the repository's real hooks run rather than bypassed, and with the
bytes committed proven to be the bytes reviewed even though the displayed diff
is redacted.

**Architecture:** A new `workspace-engine` module, `commit.rs`, computes the
candidate set from the session log's `patch_file_decided` and `commit_created`
events, stores a prepared record under `<data_dir>/commits/`, and performs the
commit through `read-tree`/`update-index`/`write-tree`/`commit-tree`/
`update-ref` with `GIT_INDEX_FILE` pointing at a private file. Git hooks run
through `CommandRunner` with that index in their environment. The desktop shell
adds `GET /api/commit-status`, `GET /api/commit-preparation`, `POST /api/commit`
and `POST /api/commit-message`, and one contextual-action provider plus one
dialog in `app.js`. No new dependency.

**Tech Stack:** Rust 2024, the existing `serde`/`serde_json`, the existing
session log and audit log, vanilla JavaScript. No framework, no bundler.

## Global Constraints

Every task's requirements implicitly include this section.

- **Read [`context.md`](context.md) in full before Task 1.** Its §2 corrections
  override the proposal where they disagree, and §3's D1–D13 are the decisions
  the tasks implement. Do not re-derive them from `proposal.md` alone.
- **Do not edit `crates/workspace-engine/src/chat.rs` or `build_model_prompt`.**
  Spec #55 owns that file in a parallel worktree. If a task appears to need it,
  stop and report rather than breaking the pairing. Message drafting lives in
  the desktop shell (D9), and the commit is a session-level action, not a model
  tool.
- **The user's index and working tree are never staging targets.** No `git add`,
  no `git commit`, no write to `.git/index`. Every staging operation sets
  `GIT_INDEX_FILE` to a private path under `<data_dir>/commits/`. A test that
  passes while `.git/index` changes is a failing test.
- **`--no-verify` is not acceptable anywhere.** Hooks are detected and run. There
  is no parameter, config key or environment variable that skips one.
- **The secret check scans the composed bytes, never the redacted diff.** It
  reports categories and counts, never matched text. The override is per commit
  and audited.
- **A commit is a capability, not a convenience.** Code mode only; the resolved
  profile must permit `allow_git_commits`; repository config may turn it off and
  may never turn it on. No persistent approval.
- **Approvals are typed.** A commit approval is its own request field and audit
  event. A command approval, a plan approval or an answer to a question never
  substitutes for it.
- **Repository configuration is untrusted.** A new key is classified explicitly
  in `ConfigOverlay`'s exhaustive destructuring, as a `Capability` in
  `config_key_kind`, and gets a weakening case in
  `tests/permission_profiles.rs`. No wildcard handling.
- **Fixtures:** `enable_index_watcher: false`; the data directory sits **beside**
  the temporary repository, not inside it (context §2.15); real commits happen
  only in a temporary repository. `DAMAIAN_DATA_DIR` for any manual run.
- **Never operate on the user's checkout or another worktree.** Never push.
- **Scoped checks per task; the full gate runs on CI only** (AGENTS.md since
  2026-10-15). Each task runs `scripts/quality-gate.sh fmt node lint typos deny`,
  `cargo clippy -p <crate> --all-targets --locked -- -D warnings` for every crate
  it touches, and the named tests. Never `--workspace` locally.
- **Tests first, and falsify every load-bearing test.** Write the named failing
  test, confirm it fails, implement, then break what the test guards, confirm it
  fails, restore, and record the mutation in the Progress row. This repository
  has a history of tests that passed without testing anything.
- **Commit locally after each task** with a short descriptive subject about the
  effect, not the process. The user pushes.

## File Structure

| File | Change |
|---|---|
| `crates/workspace-engine/src/commit.rs` | **New.** Candidate computation, prepared record, private-index composition, hooks, secret check, commit, typed errors (Tasks 3–7) |
| `crates/workspace-engine/src/lib.rs` | Re-export the commit types (Task 3) |
| `crates/workspace-engine/src/workspace_engine.rs` | Build and hold `CommitEngine` (Task 3) |
| `crates/workspace-engine/src/config.rs` | `allow_git_commits`: field, overlay, parse, serialise, classification, policy text, capabilities (Task 2) |
| `crates/workspace-engine/src/profile.rs` | Read-only profile turns it off; carry the field through profile split/import (Task 2) |
| `crates/workspace-engine/src/effective_policy.rs` | Rule label (Task 2) |
| `crates/workspace-engine/src/mode.rs` | No `ToolAction` row (a commit is not offered to the model); the gate lives in `commit.rs` (Task 2) |
| `crates/workspace-engine/src/session.rs` | `PatchFileDecision` gains applied hashes and accepted hunks (Task 3) |
| `crates/workspace-engine/src/edit.rs` | Persist what `apply_patch` already computed (Task 3) |
| `crates/workspace-engine/src/command_runner.rs` | An environment-aware entry point for hooks (Task 4) |
| `crates/workspace-engine/src/verification.rs` | Only if a public summary is missing; prefer `VerificationReport::build` (Task 3) |
| `crates/workspace-engine/src/git_service.rs` | `suggest_commit_message` stays as the fallback; add no model call (Task 6) |
| `crates/workspace-engine/tests/commit_preparation.rs` | **New.** Engine-level acceptance tests (Tasks 3–7) |
| `crates/workspace-engine/tests/permission_profiles.rs` | The capability's weakening case (Task 2) |
| `crates/desktop-shell/src/lib.rs` | Routes, JSON, the model-draft call (Tasks 6, 8) |
| `crates/desktop-shell/src/recovery.rs` | Commit record in the recovery payload (Task 7) |
| `crates/desktop-shell/static/app.js` | The contextual action, the dialog, the typed-error handling (Task 8) |
| `crates/desktop-shell/static/style.css` | Dialog presentation (Task 8) |
| `docs/USER_GUIDE.md`, `docs/TROUBLESHOOTING.md`, `SECURITY.md` | Task 9 |
| `docs/specs/README.md`, `CHANGELOG.md`, `proposal.md`, this file | Task 9 |

## Order and parallelism

```
1 ──> 2 ──> 3 ──> 4 ──> 5 ──> 6 ──> 7 ──> 8 ──> 9
```

Sequential. Task 2 defines the gate Task 3 checks. Task 3 defines the record and
composition Tasks 4–8 use. Task 4 must land before Task 5's tests can seed a
secret-bearing hook. Tasks 6–7 need Task 3's types. Task 8 needs all of them.
No task edits `chat.rs`.

---

## Task 1: Folder, links, plan

**Requirements:** AGENTS.md's "Spec layout" and "Keep spec and task records
truthful". **Context:** §1, §2.

**Files:** `docs/specs/35_commit_preparation/`, `docs/specs/README.md`,
`docs/specs/36_branch_and_worktree_delivery.md`,
`docs/specs/37_pull_request_creation.md`, `docs/specs/38_subagent_model.md`,
`docs/specs/52_mcp_server_mode.md`,
`docs/specs/57_worker_sessions/proposal.md`,
`docs/specs/58_header_repo_bar_and_menus/proposal.md`.

- [x] **Step 1:** `git mv docs/specs/35_commit_preparation.md
  docs/specs/35_commit_preparation/proposal.md`; keep the decision text and
  fix every relative link for the extra directory level.
- [x] **Step 2:** Write `context.md` with motivation, the pre-implementation
  state, the corrections (context §2.1–§2.18) and the decisions (D1–D13).
- [x] **Step 3:** Write this `tasks.md`.
- [x] **Step 4:** Update every link that named
  `docs/specs/35_commit_preparation.md`, and add #35 to the README's folder
  list. `grep -rn "35_commit_preparation.md"` must return no live link.
- [x] **Step 5:** Set `Status: In progress` in the proposal, the README
  current-state bullet, the README implementation-order row, and this header.
- [x] **Step 6:** `npm run specs:check` and read the output. Record the result
  in the Progress row and commit: `Plan commit preparation as a folder spec`.

## Task 2: The `allow_git_commits` capability

**Requirements:** 5, 6 (the gate), §5.8. **Context:** §2.9, D7, D8.

**Files:** `config.rs`, `profile.rs`, `effective_policy.rs`,
`tests/permission_profiles.rs`, `tests/commit_preparation.rs` (the gate test
lands in Task 3; this task covers the key).

- [x] **Step 1: Write the failing tests** in `tests/permission_profiles.rs`:
  - add `case("allow_git_commits", "allow_git_commits=false\n",
    "allow_git_commits=true\n", Reported("allow_git_commits"))` to
    `weakening_cases()`;
  - add the key to nothing else: the existing
    `every_overlay_field_is_classified_exactly_once` and
    `the_preference_keys_are_exactly_spec_34s_free_keys` must still pass because
    classification is explicit.
- [x] **Step 2: Confirm they fail** (the key does not parse, so the weakening
  case cannot be constructed).
- [x] **Step 3: Implement.**
  - `Config`: `pub allow_git_commits: bool`, default `true`.
  - `ConfigOverlay`: `pub allow_git_commits: Option<bool>`, parsed by `set`,
    emitted by `to_policy_text`, carried in the exhaustive destructuring.
  - `apply_overlay_scoped`: add it to the restrict-only capability loop with
    restrictive value `false`.
  - `config_key_kind`: `allow_git_commits => Capability`.
  - `profile.rs`: `ProfileCapabilities.allow_git_commits`; `Config::
    profile_capabilities()` fills it; `ReadOnly` sets `Some(false)`; carry the
    key through the profile split and import base.
  - `effective_policy.rs`: label `"Git commits allowed"`.
  - `to_policy_text`: one line beside the other capability keys.
- [x] **Step 4: Falsify.** Classify it `Preference` → the capability-cover test
  fails. Omit it from the restrict-only loop → the repository-scope case fails
  (a repository turns it on). Remove it from `ProfileCapabilities` → the matrix
  test fails to compile.
- [x] **Step 5: Scoped checks.**

  ```bash
  scripts/quality-gate.sh fmt node lint typos deny
  CARGO_TARGET_DIR="$CARGO_TARGET_DIR" cargo clippy -p workspace-engine --all-targets --locked -- -D warnings
  CARGO_TARGET_DIR="$CARGO_TARGET_DIR" cargo nextest run -p workspace-engine --test permission_profiles
  ```

- [x] **Step 6: Update the Progress row** and commit:
  `Make committing a capability a profile can deny`.

## Task 3: Durable acceptance detail and the commit engine

**Requirements:** 1, 3, 4, 6, 7 (the SHA), §5.1–§5.3. **Context:** §2.6–§2.8,
D1–D3.

**Files:** `session.rs`, `edit.rs`, `commit.rs` (new), `workspace_engine.rs`,
`lib.rs`, `tests/commit_preparation.rs` (new).

- [x] **Step 1: Write the failing tests** in
  `crates/workspace-engine/tests/commit_preparation.rs` with a fixture whose
  data directory is `base/data` and repository is `base/repo`; `git init`,
  identity, `gpgsign=false`, `gc.auto=0`:
  1. `a_commit_contains_only_the_accepted_files` — five files changed, three
     accepted through `apply_stored_patch`, then `prepare` and `commit`;
     `git ls-tree -r --name-only HEAD` is exactly the three, and each blob's
     content is the accepted content.
  2. `a_hunk_level_acceptance_commits_only_the_accepted_hunks` — a patch with
     two hunks, accept one; the committed file equals the partial apply's bytes,
     and the rejected hunk's line is absent.
  3. `a_hand_edited_accepted_file_is_shown_labelled_and_unselected` — apply,
     then edit the file; the candidate has the "edited" label and
     `selected == false`; `commit` with only default selection commits nothing
     (or the other accepted file).
  4. `a_command_changed_file_is_shown_labelled_and_unselected` — a checkpoint
     manifest with origin `Command` for an unaccepted path; the candidate is
     labelled and unselected.
  5. `the_users_index_and_working_tree_are_unchanged_by_a_commit` — stage an
     unrelated file first; capture `.git/index` bytes and the status; commit;
     both are identical, the staged file is not in the commit, and it is still
     staged afterwards.
  6. `head_moving_between_prepare_and_commit_fails_with_nothing_changed` — after
     `prepare`, make another commit in the fixture; `commit` returns
     `CommitError::HeadMoved`; HEAD, the index and the worktree are unchanged.
  7. `a_detached_head_and_a_merge_in_progress_are_refused` — `prepare` reports a
     refusal for each; `commit` refuses and writes nothing.
  8. `the_commit_tree_is_verified_before_the_ref_moves` — a unit test over the
     composition helper: a deliberately wrong expected tree aborts before
     `commit-tree` (no new ref).
- [x] **Step 2: Confirm they fail** (the module and types do not exist).
- [x] **Step 3: Implement the durable detail.**
  - `PatchFileDecision` gains `#[serde(default)] applied_hashes:
    BTreeMap<String, String>` and `#[serde(default)] accepted_hunks:
    BTreeMap<String, Vec<String>>`.
  - `record_patch_file_decision` takes the applied hashes and accepted hunks;
    `apply_stored_patch` passes `result.applied` and, per path, `hunk_selection`
    or all of the file's hunk ids; reject paths pass empty maps.
  - Keep old logs loadable: both fields default to empty.
- [x] **Step 4: Implement the commit engine.**
  - Types: `CommitCandidate { path, status, source, labels, selected_by_default,
    blob_oid, mode, size, unscanned, diff }`; `CommitPreparation`;
    `CommitStatus`; `CommitRequest`; `CommitOutcome`; `CommitError`;
    `CommitRecord`.
  - `CommitEngine::status(root, session_id)`: session mode + profile gate, git
    state refusals, candidates, counts.
  - `CommitEngine::prepare(root, session_id)`: `status` plus per-candidate
    redacted diff (reuse `GitService::diff_in`; fall back to `diff_file` for an
    untracked path, and `[Binary file]` above `max_file_bytes` or on a NUL in
    the first 8 KiB), the hook report (Task 4 may stub it to empty here), the
    secret warnings (Task 5 may return empty here), the deterministic message
    from `GitService::suggest_commit_message`, and a saved record.
  - `CommitEngine::commit(request)`: load the record; verify session, mode,
    profile, HEAD, selected paths, and each selected candidate's current blob
    identity and mode; then compose through the private index:
    `read-tree HEAD` (or `--empty`), `hash-object -w` (or `--stdin` for a
    symlink), `update-index -z --index-info`, `write-tree`, verify with
    `ls-tree -r -z`, `commit-tree` with `-F <message file>` and `-p HEAD`,
    re-check HEAD, `update-ref refs/heads/<branch> <new> <old>`, read back and
    assert the new SHA.
  - Move blobs only at commit time; `prepare` runs `hash-object` **without**
    `-w`.
  - Record the commit in `commit_created` for Task 7; at this task record it in
    the audit log.
  - Add `CommitEngine` to `WorkspaceEngine` and re-export the public types from
    `lib.rs`.
- [x] **Step 5: Falsify.** Use the user's index instead of the private one → test
  5 fails. Drop the hash comparison → test 3 commits after a hand edit. Skip the
  tree verification → the verification test's negative case passes when it
  should refuse. Compare HEAD only before composition → test 6's CAS still
  passes but the pre-check removal is caught by a log assertion; restore.
- [x] **Step 6: Scoped checks.**

  ```bash
  scripts/quality-gate.sh fmt node lint typos deny
  CARGO_TARGET_DIR="$CARGO_TARGET_DIR" cargo clippy -p workspace-engine --all-targets --locked -- -D warnings
  CARGO_TARGET_DIR="$CARGO_TARGET_DIR" cargo nextest run -p workspace-engine --test commit_preparation --test foundation
  ```

- [x] **Step 7: Update the Progress row** and commit:
  `Commit exactly the accepted changes through a private index`.

## Task 4: Git hooks: detect, report, run, never bypass

**Requirements:** 2, 4 (hook half), §5.5. **Context:** §2.11–§2.12, D4–D5.

**Files:** `commit.rs`, `command_runner.rs`, `tests/commit_preparation.rs`.

- [x] **Step 1: Write the failing tests.**
  1. `hooks_are_reported_with_path_executable_size_and_interpreter` — write
     `.git/hooks/pre-commit` with `#!/bin/sh`; the report names it, executable,
     its size and `#!/bin/sh`; a non-executable `commit-msg` is reported and
     marked as not run.
  2. `a_failing_pre_commit_aborts_and_reports_its_output` — a hook exits 1 and
     prints `no commit today`; `commit` returns `CommitError::HookFailed`, no ref
     moved, and the output is in the error.
  3. `a_commit_msg_hook_that_rewrites_the_message_is_shown_before_the_commit` —
     the hook appends a trailer; the first commit returns
     `CommitError::MessageRewritten` with the trailer, HEAD unchanged; retrying
     with that text commits and the message contains it.
  4. `a_failing_post_commit_hook_is_a_warning_and_the_commit_stays` — a
     `post-commit` exit 1; the outcome has the SHA and a warning; HEAD moved.
  5. `hooks_run_with_the_private_index` — a `pre-commit` that writes
     `git diff --cached --name-only` to a file; the file names the selected path
     and the user's staged unrelated file is absent.
  6. `a_hanging_hook_times_out` — `command_timeout_secs = 1`, a sleeping hook;
     `CommitError::HookFailed` with termination `timed_out`.
  7. `hook_output_is_truncated_and_redacted` — a hook prints a seeded fake
     token and more than `max_command_output_bytes`; the error's output has no
     token and is bounded.
  8. `there_is_no_bypass` — `include_str!("../src/commit.rs")` and
     `include_str!("../src/command_runner.rs")` contain no `--no-verify`, and no
     public hook-running function takes a skip flag.
  9. `prepare_commit_msg_runs_too` — its rewrite is treated like commit-msg.
- [x] **Step 2: Confirm they fail.**
- [x] **Step 3: Implement.**
  - `CommandRunner::run_at_with_env` (or equivalent) with `.envs(...)`; `run` and
    `run_at` delegate with `&[]`. No behaviour change for existing callers.
  - Hook directory: `git config --get core.hooksPath` (relative to the
    repository root, absolute as given), else `git rev-parse --git-path hooks`.
  - Report the four hooks; a hook is run only when it exists, is a file and has
    an execute bit. Skip non-executable with a stated reason.
  - Run in Git's order with `GIT_INDEX_FILE`, `GIT_DIR` (`--absolute-git-dir`)
    and `GIT_WORK_TREE` (`--show-toplevel`), `prepare-commit-msg` and
    `commit-msg` against a message file in the private directory.
  - `HookFailed` and `MessageRewritten` carry the runner's already redacted,
    truncated output.
  - Register the hook's PID through the runner's existing `ProcessKind::Command`
    registration.
- [x] **Step 4: Falsify.** Run the hook before `read-tree` → test 5's file is
  empty/the user's staging appears. Drop the executable check → test 1's
  non-executable case runs. Ignore the rewrite → test 3 commits on the first
  attempt. Remove the timeout → test 6 hangs (use a nextest timeout).
- [x] **Step 5: Scoped checks.**

  ```bash
  scripts/quality-gate.sh fmt node lint typos deny
  CARGO_TARGET_DIR="$CARGO_TARGET_DIR" cargo clippy -p workspace-engine --all-targets --locked -- -D warnings
  CARGO_TARGET_DIR="$CARGO_TARGET_DIR" cargo nextest run -p workspace-engine --test commit_preparation --test command_lifecycle
  ```

- [x] **Step 6: Update the Progress row** and commit:
  `Run a repository's Git hooks for a commit instead of skipping them`.

## Task 5: Exact bytes, the secret check and the per-commit override

**Requirements:** 1, 4 (the approval half), §5.4. **Context:** §1, D2, D6.

**Files:** `commit.rs`, `tests/commit_preparation.rs`.

- [x] **Step 1: Write the failing tests.**
  1. `the_approved_content_is_the_committed_content` — prepare, change a selected
     file, commit → `CommitError::ContentChanged`, no ref moved; restore and
     commit succeeds with the reviewed bytes.
  2. `a_seeded_secret_warns_with_categories_and_counts_and_no_text` — seed a
     fake token in an accepted file; the preparation's warning has the
     category/count, and the serialised preparation contains neither the token
     nor `[REDACTED]` in the warning fields.
  3. `a_secret_needs_a_separate_per_commit_override` — commit without the
     override → `CommitError::Refused`/`SecretsFound`, no ref; with it, the
     commit succeeds and the audit has `commit_secret_override` with counts and
     no text; `commit_created` has `generatedSecretOverride=true`.
  4. `an_unscannable_file_is_treated_as_a_secret_warning` — a file over
     `max_file_bytes`; the warning category is `unscanned`, and it needs the
     same override.
  5. `the_recorded_diff_is_redacted_and_the_record_holds_no_unredacted_copy` —
     a secret in the diff; the candidate's `diff` contains `[REDACTED]`, and the
     prepared record on disk does not contain the secret. (The record stores the
     redacted diff.)
- [x] **Step 2: Confirm they fail.**
- [x] **Step 3: Implement.**
  - Scan each selected candidate's composed bytes with `SecretScanner::scan`;
    aggregate categories/counts into `GeneratedSecretWarning`. Files over
    `max_file_bytes` are `unscanned`.
  - `commit` scans only the selection; `allow_generated_secrets` is a separate
    request field, required when warnings exist. Audit the override with
    categories/counts.
  - The record already stores the blob identity; enforce equality for every
    selected path before composing, and refuse with `ContentChanged` otherwise.
  - Never store unredacted content or matched text in the record or audit.
- [x] **Step 4: Falsify.** Remove the equality check → test 1 commits the changed
  bytes. Scan the diff instead of the composed bytes → the seeded secret in a
  hand-edited region the diff redacts is not flagged (construct the test so the
  scanner sees only the raw bytes). Put matched text in the warning → test 2
  fails.
- [x] **Step 5: Scoped checks.**

  ```bash
  scripts/quality-gate.sh fmt node lint typos deny
  CARGO_TARGET_DIR="$CARGO_TARGET_DIR" cargo clippy -p workspace-engine --all-targets --locked -- -D warnings
  CARGO_TARGET_DIR="$CARGO_TARGET_DIR" cargo nextest run -p workspace-engine --test commit_preparation
  ```

- [x] **Step 6: Update the Progress row** and commit:
  `Check the bytes a commit will write, not the redacted diff`.

## Task 6: Message drafting with the deterministic fallback

**Requirements:** 6, §5.6. **Context:** §2.4, §2.10, D9.

**Files:** `crates/desktop-shell/src/lib.rs` (the draft routine and its tests),
`tests` inline; `git_service.rs` only if the fallback needs a signature change
(it should not); `commit.rs` for the recorded summary.

- [x] **Step 1: Write the failing tests** in `desktop-shell/src/lib.rs`:
  - `a_model_draft_is_shown_and_can_be_edited_before_commit` — a
    `MockModelAdapter` returns a subject and body; the draft response carries
    them; an edited message is what `commit` receives (the engine test covers
    the commit side; the shell test asserts the request field).
  - `a_failed_model_draft_falls_back_to_the_template` — the adapter returns an
    error or an empty string; the response's `source` is `fallback` and the text
    equals `"chore: …"`.
  - `no_configured_model_falls_back_without_a_call` — the adapter closure returns
    the no-key error; no request is made.
  - `the_model_sees_the_redacted_diff_and_the_plan_titles` — the recorded
    request contains `[REDACTED]` for a seeded secret and the plan step titles;
    the raw secret is absent.
- [x] **Step 2: Confirm they fail.**
- [x] **Step 3: Implement.**
  - `POST /api/commit-message` takes `repo`, `session_id`, `prepare_id`; loads
    the prepared record, builds the prompt from the redacted diffs and the
    latest task plan's step titles, calls `chat_model_adapter`, parses the first
    line as the subject, falls back on any failure or empty result.
  - The response is `{ subject, body, message, source: "model"|"fallback" }`.
    `message` is what the textarea receives (`subject` + blank line + `body`).
  - The deterministic fallback is `GitService::suggest_commit_message` with the
    task's summary and the selected paths.
  - No new model call happens on prepare; the dialog opens with the fallback and
    the user presses *Draft with model*.
- [x] **Step 4: Falsify.** Send an unredacted diff → the redaction assertion
  fails. Return `source: "model"` on an empty response → the fallback test
  fails. Drop the plan titles → the request assertion fails.
- [x] **Step 5: Scoped checks.**

  ```bash
  scripts/quality-gate.sh fmt node lint typos deny
  CARGO_TARGET_DIR="$CARGO_TARGET_DIR" cargo clippy -p desktop-shell --all-targets --locked -- -D warnings
  CARGO_TARGET_DIR="$CARGO_TARGET_DIR" cargo nextest run -p desktop-shell --lib
  ```

- [x] **Step 6: Update the Progress row** and commit:
  `Draft a commit message without making the commit depend on a model`.

## Task 7: Durable action markers, audit and crash inspection

**Requirements:** 3, 4 (state half), §5.7–§5.8. **Context:** §2.14, D8.

**Files:** `commit.rs`, `crates/desktop-shell/src/recovery.rs`,
`tests/commit_preparation.rs`, `tests/crash_recovery.rs`.

- [x] **Step 1: Write the failing tests.**
  1. `a_commit_is_bracketed_by_a_side_effecting_marker` — a successful commit's
     session log has `action_started` with `action:"commit"` and
     `sideEffecting:true`, then `action_finished` with `outcome:"ok"`; the finish
     carries the SHA or a `commit_created` event beside it does.
  2. `a_crash_between_the_markers_is_an_unknown_outcome` — start the marker by
     hand, leave it dangling, `classify_session` returns
     `UnknownExternalOutcome`; `resume` is not permitted.
  3. `the_commit_is_audited_with_counts_and_no_secret_text` — `commit_created`
     has path count, hunk count, message length, SHA, hook outcomes and
     `generatedSecretOverride`; no seeded token appears anywhere in the audit.
  4. `recovery_reports_the_prepared_commit_and_the_current_head` — the shell's
     recovered JSON for a dangling `commit` has the record's paths, expected old
     and prepared commit; `Inspect` can state whether HEAD moved to it.
- [x] **Step 2: Confirm they fail.**
- [x] **Step 3: Implement.**
  - At commit: create a synthetic task ("Commit prepared changes") in
    `RunningTool`, start `commit` with `sideEffecting: true`, and on a known
    outcome set the task terminal and finish the marker with `ok`/`failed`. A
    crash before the final write leaves the task non-terminal with a dangling
    marker, which #17's classifier already turns into
    `unknown_external_outcome`; recovery must not re-commit.
  - Persist `CommitRecord` with `expected_old`, `prepared_commit`, `head_after`,
    state, paths, hook outcomes and override flag.
  - Audit `commit_prepared`, `commit_refused`, `commit_hook_ran`,
    `commit_secret_override` and `commit_created`; counts and codes, never
    content.
  - `desktop-shell/src/recovery.rs`: for `action == "commit"`, load the record,
    list its selected paths, and add a `commit` object to the recovered JSON
    with `expectedOld`, `preparedCommit`, `headNow`, `landed`. `app.js`'s
    `createRecoveryDetail` shows it; the comparison is information, not an
    automatic resolution.
- [x] **Step 4: Falsify.** Start the marker after the ref moves → the crash test
  classifies `Interrupted`. Record the bare message in the audit → the secret
  text assertion fails (use a message containing a fake token). Remove the task
  status write → the dangling task is auto-resumable.
- [x] **Step 5: Scoped checks.**

  ```bash
  scripts/quality-gate.sh fmt node lint typos deny
  CARGO_TARGET_DIR="$CARGO_TARGET_DIR" cargo clippy -p workspace-engine --all-targets --locked -- -D warnings
  CARGO_TARGET_DIR="$CARGO_TARGET_DIR" cargo clippy -p desktop-shell --all-targets --locked -- -D warnings
  CARGO_TARGET_DIR="$CARGO_TARGET_DIR" cargo nextest run -p workspace-engine --test commit_preparation --test crash_recovery
  CARGO_TARGET_DIR="$CARGO_TARGET_DIR" cargo nextest run -p desktop-shell --lib
  ```

- [x] **Step 6: Update the Progress row** and commit:
  `Record a commit as a side-effecting action recovery can inspect`.

## Task 8: Shell routes and the *Commit…* dialog

**Requirements:** 1–6 where user-visible, §5.1, §5.8, §5.9. **Context:** §2.13,
D7, D10.

**Files:** `crates/desktop-shell/src/lib.rs`, `static/app.js`, `static/style.css`,
`static/index.html` only if a new element is unavoidable (it is not: the slot
exists).

- [x] **Step 1: Write the failing shell tests** (inline in `lib.rs`):
  - `commit_status_counts_the_uncommitted_accepted_changes` — a temporary repo,
    an applied patch, `GET /api/commit-status` → `candidateCount > 0`,
    `selectedCount > 0`, `canCommit: true`.
  - `a_commit_request_needs_the_commit_approval` — missing or wrong `approval`
    → 4xx with `approval_required`, no ref moved.
  - `a_commit_is_refused_outside_code_mode` — the session is Ask → 4xx with
    `AccessDenied`/`approval_required` and no ref moved.
  - `a_commit_is_refused_when_the_profile_denies_it` — `allow_git_commits=false`
    → 4xx naming the key.
  - `a_commit_refusal_carries_a_code_and_the_rewritten_message` — a
    `commit-msg` rewrite → JSON has `code:"commit_message_rewritten"` and
    `message` equals the rewritten text.
  - `the_commit_preparation_json_names_the_redaction_and_the_hooks` — the
    payload has `diffRedacted: true`, the hook rows, and every candidate's
    `selectedByDefault`.
- [x] **Step 2: Confirm they fail.**
- [x] **Step 3: Implement the routes.**
  - `GET /api/commit-status?repo=&session=` → counts and the refusal reason.
  - `GET /api/commit-preparation?repo=&session=` → the full preparation.
  - `POST /api/commit` → requires `approval=commit`, `prepare_id`, `session_id`,
    `paths` (one per line), `message`, `allow_generated_secrets`; maps
    `CommitError` to `{error, code, message}` with the right status.
  - `POST /api/commit-message` (Task 6).
  - `GET /api/git-status` stays as it is; `refreshRepoBar` additionally fetches
    `/api/commit-status` when a session is open, in parallel.
- [x] **Step 4: Implement the UI.**
  - A `commitAction()` provider after the findings providers: visible when
    `repo()` and a session are open and `candidateCount > 0`; label `Commit…`;
    tone neutral; `run` opens the dialog.
  - The dialog (`showCommitDialog`): message textarea (fallback prefilled),
    *Draft with model*, per-file checkbox rows with path, status and label, a
    per-file diff disclosure (redacted), the standing statement "The diff is
    redacted for display; the commit contains the real file content.", the hook
    report with "Damaian runs these hooks and never skips them.", the verification
    evidence, branch/HEAD, and any refusal reason. Secret warnings show
    categories/counts and a required override checkbox. The primary button says
    `Commit N files` and is disabled with no selection, with a refusal, or while
    a warning lacks the override.
  - On `commit_message_rewritten`, keep the dialog open, replace the message,
    and say the hook rewrote it. On `commit_hook_failed`, show the hook name and
    its output. On `head_moved` / `content_changed`, say nothing was committed
    and refresh. On success, close, toast the short SHA and message, refresh the
    repo bar and the session.
  - `api()` attaches the parsed payload to thrown errors (`error.payload`) so
    the dialog can branch on `code` without parsing prose.
  - `createRecoveryDetail` gains the commit row from Task 7.
  - Style with existing tokens/classes (`.app-dialog`, `.app-dialog-wide`,
    `.btn-sm`, `.btn-primary`, `.btn-danger`); add only `.app-dialog-commit`
    and the file/diff rows. Follow `docs/UI_STYLE_GUIDE.md`.
- [x] **Step 5: Check by hand** in the real web UI on a scratch port and data
  dir: the action appears, the dialog lists accepted and unaccepted files,
  toggling changes the count, the hook report renders, a failing hook shows its
  output, a rewritten message round-trips, and a successful commit shows the
  SHA and clears the action. Record the walk (or state plainly which parts were
  not walked) in the Progress row.
- [x] **Step 6: Scoped checks.**

  ```bash
  scripts/quality-gate.sh fmt node lint typos deny
  CARGO_TARGET_DIR="$CARGO_TARGET_DIR" cargo clippy -p desktop-shell --all-targets --locked -- -D warnings
  CARGO_TARGET_DIR="$CARGO_TARGET_DIR" cargo nextest run -p desktop-shell --lib
  node --check crates/desktop-shell/static/app.js
  npm run lint:web
  ```

- [x] **Step 7: Update the Progress row** and commit:
  `Offer Commit… in the repo bar with its own approval`.

## Task 9: Docs, acceptance walk, close-out

**Requirements:** §5.9, §6, §7. **Files:** `docs/USER_GUIDE.md`,
`docs/TROUBLESHOOTING.md`, `SECURITY.md`, `proposal.md` §7,
`docs/specs/README.md`, `CHANGELOG.md`, `docs/specs/36_branch_and_worktree_delivery.md`,
`docs/specs/37_pull_request_creation.md`, this file.

- [x] **Step 1: Walk every acceptance criterion in §6** against the finished
  tree and record a criterion-to-test table in `proposal.md` §7, naming which
  criteria are manual.
- [x] **Step 2: Fill `proposal.md` §7's implementation notes:** the private-index
  behaviour across normal, merge, rebase and detached states; which states are
  refused and why; how often the secret check fired; `core.hooksPath` handling.
- [x] **Step 3: Documentation.**
  - `USER_GUIDE.md`: what is included and why hand-edited/command files are
    shown unselected; the hook report; that the diff is redacted while the
    commit is not; that Damaian never skips hooks; Code mode and the profile
    gate; that a hook-rewritten message needs a second approval.
  - `TROUBLESHOOTING.md`: `HEAD` moved; reading a hook failure; whether an
    interrupted commit landed (compare HEAD with the prepared SHA); where the
    commit is audited; a message rewritten by a hook.
  - `SECURITY.md`: the first write to the repository, the private index, the
    approval, the typed commit approval, the secret scan and its override, the
    capability, and that repository config can only deny.
- [x] **Step 4: Close-out.** README current-state paragraph, #35's row, and
  "What to build next" (#35 leaves the ready set, #36 is promoted). Flip #36's
  `Depends on:` line for #35 to built, and update #37's prose where it now
  correctly says #35 is built. Remove #35's `Unreleased` line from
  `CHANGELOG.md`. Mark Done in all four places (proposal header, README row,
  this Progress table, this header's `**Done:**`). Say plainly "CI outstanding".
- [x] **Step 5: Scoped checks and `npm run specs:check`.**

  ```bash
  scripts/quality-gate.sh fmt node lint typos deny
  CARGO_TARGET_DIR="$CARGO_TARGET_DIR" cargo clippy -p workspace-engine -p desktop-shell --all-targets --locked -- -D warnings
  CARGO_TARGET_DIR="$CARGO_TARGET_DIR" cargo nextest run -p workspace-engine --test commit_preparation --test permission_profiles --test crash_recovery
  CARGO_TARGET_DIR="$CARGO_TARGET_DIR" cargo nextest run -p desktop-shell --lib
  npm run specs:check
  ```

- [x] **Step 6: Commit:** `Close spec 35: commit the accepted changes`.
