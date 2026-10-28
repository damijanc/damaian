# Branch and Worktree Delivery — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use
> superpowers:subagent-driven-development (recommended) or
> superpowers:executing-plans to implement this plan task-by-task. Steps use
> checkbox (`- [ ]`) syntax for tracking.

**Implements:** [`proposal.md`](proposal.md) in full, with the corrections and
decisions in [`context.md`](context.md). Where the two disagree, `context.md`
wins.
**Started:** 2026-10-28 — **Done:** 2026-10-28

## Progress

| Task | State | Notes |
|---|---|---|
| 1 · Read-only branch information | Done 2026-10-28 | Added `BranchInfo` (camelCase serde) and `GitService::branch_info` in `git_service.rs`: current branch, full `HEAD`, detached, upstream via `rev-parse @{upstream}`, ahead/behind through the existing helper, in-progress operation from Git-resolved marker paths including `BISECT_LOG`, and unmerged paths via `git diff --diff-filter=U -z` filtered by the restricted-path rule. `merge_base` is now public and validates `base` before git runs. `head_oid` is `Option<String>` because an unborn branch has no commit (context §3.13/D5). Audit is `git_branch_read` with counts and no restricted path names. New `tests/branch_delivery.rs` has 10 tests: upstream 0/0 → 1/0 → 0/1 → 1/1, detached, unborn, real conflicted merge, clean state, merge-base and option-shaped base refusal, byte-identical index/HEAD/status across reads, restricted conflicting `.env` omitted, non-repository refusal, and camelCase JSON. Falsified by swapping ahead/behind (test failed) and by skipping the `MERGE_HEAD` scan (test failed), then restoring. Scoped checks: `cargo fmt --all`; `cargo clippy -p workspace-engine --all-targets --locked -- -D warnings` clean; `cargo nextest -p workspace-engine --test branch_delivery --test worktree_reads` 58 passed. No `chat.rs` change. |
| 2 · Suggested branch name | Done 2026-10-28 | Added `git_branch_prefix` (`Config` default `damaian/`, `ConfigOverlay` field, parse/serialise, `ConfigKeyKind::Preference`, repository-scope `preference()` merge, policy text, profile split refuses it) and the config-key record updates. New `branch.rs` has `slugify_branch_title` (lowercase ASCII, separator runs, 50-char cap, `change` fallback), `validate_branch_prefix` (empty allowed; rejects `@{`, `..`, `//`, leading `-`/`/`, control/special characters, `.lock` components, trailing `.`), `BranchSuggestion`, and `suggest_branch_name`, which redirects the source through `SecretScanner::redact` before slugging (D10). `GitService` gained `check_branch_name` (authoritative `git check-ref-format --branch`) and `local_branches` (one `for-each-ref`); a collision keeps the name unchanged and offers the first free `-2`/`-3` alternative. Six new branch tests: slug bounds/unicode, default prefix, collision and alternative, empty/invalid prefixes including `feature@{1}/`, Git-authoritative valid/invalid names, and a seeded AWS key absent from the name. One new permission test proves a repository may choose the prefix and a profile may not. Falsified by removing the redaction call: the secret test failed with `damaian/rotate-akiaiosfodnn7example-before-release`, then restored. Scoped checks: `cargo fmt --all`; `cargo clippy -p workspace-engine --all-targets --locked -- -D warnings` clean; `cargo nextest -p workspace-engine --test branch_delivery --test permission_profiles --test foundation` 268 passed (1 skipped); fast gate `fmt node typos` clean. `cargo deny check` fails on RUSTSEC-2026-0414 in `pulldown-cmark` 0.13.4, a dependency this task does not touch (context §6). Discovery recorded in context §3.14: Git 2.56's `check-ref-format` rejects `--end-of-options`, so leading-`-` refusal stays in Rust. No `chat.rs` change. |
| 3 · The `allow_git_branches` capability | Done 2026-10-28 | Added `allow_git_branches` through the same four-file pattern as `allow_git_commits`: `Config` default `true`, `ConfigOverlay` field with parse/serialise, `ConfigKeyKind::Capability`, restrict-only in `apply_overlay_scoped`, `Config::to_policy_text`, `ProfileCapabilities.allow_git_branches` and `Config::profile_capabilities`, the Read-only overlay turns it off, `split_profile_keys` carries it, and `rule_label` gives it "Git branches allowed". Tests in `tests/permission_profiles.rs`: the weakening case (repository `true` over user `false` refused as restrict-only), a new two-load `a_repository_may_deny_git_branches_and_may_not_grant_them`, `allow_git_branches_is_a_capability_key_with_a_human_name`, default `true`, the Read-only row in `each_built_in_profile_resolves_to_its_table_row` now expects false (all other built-ins true), and `EVERY_PROFILE_KEY` carries the new key. Falsified by passing `restrictive = true` for the new key in `apply_overlay_scoped`: `every_capability_key_resists_weakening_from_repository_scope` failed with "repository value was not refused as allow_git_branches; got []" and the denial test failed at `!denied.allow_git_branches`; restored and both pass. Scoped checks: `cargo fmt --all`; `cargo clippy -p workspace-engine --all-targets --locked -- -D warnings` clean; `cargo nextest -p workspace-engine --test permission_profiles --test branch_delivery --test foundation` 270 passed (1 skipped); `cargo nextest -p workspace-engine --lib` 579 passed (5 skipped). No behaviour is wired, per the task and D1: no branch mutation entry point exists yet (Task 4 adds `prepare_*`/`apply`), and the only existing branch-creating path, spec 57's `worktree.rs` `git worktree add -b`, is its own `enable_worker_sessions` facility, not this capability. Task 4 must gate every `prepare_*` on `config.profile_capabilities().allow_git_branches` and Code mode. Observation, not changed: `EVERY_PROFILE_KEY` never gained `allow_git_commits` in spec 35; only the new key was added here. The repeated `rev-parse --git-path` calls noted in review were left alone as instructed. No `chat.rs` change. |
| 4 · Branch creation and switching | Done 2026-10-28 | Added `GitMutation` (`CreateBranch`/`SwitchBranch`, internally tagged camelCase, no catch-all), `BranchPreparation`, `BranchRecord`, `BranchOutcome`, `BranchInspection`, `BranchError` (`code()`/`message()`), and `BranchEngine` in `branch.rs`; `WorkspaceEngine` gains `branch` built with config, audit log, scanner, path policy, session store and `GitService`. `prepare_create(name, start_point, session_id)` and `prepare_switch(name, session_id)` refuse a non-repository, unknown session, non-Code mode, `allow_git_branches == false`, any in-progress operation, detached `HEAD` without an explicit start, unborn `HEAD`, invalid/option-shaped names and start points, a collision, and a missing switch target; every refusal is a reason in the preparation plus `git_branch_refused`. Creation resolves the start with `rev-parse --verify -q --end-of-options <start>^{commit}` (default `HEAD`) and records the resolved SHA in the mutation; `git branch <name> <sha>` leaves `HEAD`/index/tree untouched. Switching resolves `refs/heads/<name>`, intersects `HEAD..target` changed paths with `git.changes`' changed/untracked paths (restricted paths filtered by `PathPolicy`), names the paths in the refusal and re-checks at apply; the command is `git switch --no-guess <name>`. `apply(prepare_id, approved_mutation, approved_by)` requires the exact mutation, a `prepared` record, and re-checks Code mode, `allow_git_branches`, in-progress state, `HEAD`/ref expectations; it creates a synthetic `RunningTool` task, starts `start_action(..., true)` with `git_branch_create`/`git_branch_switch`, runs through `checkpoint::run_user_git`, records the exact `user_git_command_line`, finishes the marker, marks the record `applied` (or `failed`) and audits `git_branch_created`/`git_branch_switched` with refs, start point, `approvedBy`, head before/after and outcome. `inspect` compares `show-ref --verify` with the record and reports `landed`/`not_landed`/`other_ref_state` without retrying; a creation left `prepared` but already at its resolved start reports `landed` (crash-after-command is a read, not a guess). Ten new tests in `tests/branch_delivery.rs`: creation byte-identical index/HEAD/status with the ref at the start point; create-then-switch and switching an existing branch move HEAD and files; Ask/Plan/Review and `allow_git_branches=false` refused at prepare and again at apply (record survives a policy refusal); mid-merge and detached (refused without a start point, allowed with one); collision/invalid/option-shaped/unresolved; dirty switch names paths at prepare and at apply; retargeted approval and second apply refused; dangling record inspect; marker pair + audit; no `-f`/`--force`. Falsified by adding `-f` to creation (no-force test failed), disabling the existing-ref refusal (collision test failed) and skipping the `prepared` check (consumption test failed), then restoring. Scoped checks: `cargo fmt --all`; `cargo clippy -p workspace-engine --all-targets --locked -- -D warnings` clean; `cargo nextest run -p workspace-engine --test branch_delivery --test permission_profiles` 109 passed, 1 skipped. Deviations: the mutation carries the resolved SHA while the preparation/record report the original start point and SHA; apply re-checks dirty-switch paths (stricter than the task's "before switching"); `BranchEngine::suggest` is a read-only convenience so the configured prefix and redaction have one entry point for Task 7; `finish_preparation` has a targeted `#[allow(clippy::too_many_arguments)]` as `commit.rs` does; `show-ref` failure is treated as "ref absent" and Git's own `git branch` refusal remains the backstop. No `chat.rs`, `git_service.rs` or spec 57 `worktree.rs` change; Task 5 starts from `BranchRecord`/`apply`/`inspect` and must add `DeleteBranch` to every exhaustive match (`name`, `action`, `audited_verb`, `reference`, `finish_preparation`'s `expected_ref_oid`, `inspect`'s `expected_oid`, apply's expected-state/dirty branches). |
| 5 · Branch deletion with the unmerged confirmation | Done 2026-10-28 | Added `GitMutation::DeleteBranch { name, unmerged_commits: Option<u32> }` and extended every Task 4 match: `name`, `action` (`git_branch_delete`), `audited_verb` (`git_branch_deleted`), `reference`, `finish_preparation`'s `expected_ref_oid`, `inspect`'s `expected_oid`/outcome logic, and `mutation_args`. `prepare_delete(name, session_id)` refuses a non-repository, unknown session, non-Code mode, `allow_git_branches == false`, an in-progress operation, an invalid/option-shaped name, the current branch, a branch checked out in any worktree, and a missing branch; every refusal is a reason plus `git_branch_refused` and leaves no record. D11 resolves the default branch from `refs/remotes/origin/HEAD`, then local `main`, then local `master`, then the branch's merge base with `HEAD`; when none resolves the count is `None` and deletion always takes the unmerged confirmation path. Fully merged deletion runs `git branch -d`: the plan's literal `--not --remotes --not <default>` is wrong because a second `--not` toggles back to positive revisions and counted the default branch's own history, so the implementation runs `git rev-list --count <branch> --not --remotes <default>` and the matching `git log --format=%s` (verified against a probe repository). `apply(prepare_id, approved_mutation, approved_by, confirm_unmerged)` re-checks the capability, mode, in-progress state, unchanged `HEAD`, the ref still at the reviewed commit, and that no worktree now has the branch checked out. A plain approval for an unmerged or unknown-count branch is refused with code `branch_unmerged_confirmation_required`, the count (or "could not determine the default branch") and the subjects, and leaves the record `prepared`; only `confirm_unmerged = true` runs `git branch -D`, still through `checkpoint::run_user_git` with a `git_branch_delete` marker pair. The success audit records the ref, `unmergedCommits` (`"unknown"` when unresolved) and `confirmedUnmerged` (what was actually used, so a merged branch with the flag still uses `-d` and records false), never subjects or bodies. `inspect` treats an absent ref as `landed` for a deletion, a ref still at the reviewed commit as `not_landed`, and any other commit as `other_ref_state`; a delete that already happened is therefore classifiable without retrying. Six new tests in `tests/branch_delivery.rs`: merged delete uses `-d` with byte-identical HEAD and audit fields; unmerged delete refuses the plain approval with count and subjects, deletes only with `-D`, omits a commit body from the audit, and consumes the record; an orphan branch with no resolvable default reports unknown and still requires confirmation; the current branch and a branch checked out in another worktree are refused at prepare, and the worktree case again at apply; a missing branch, mid-merge, Ask/Plan/Review and `allow_git_branches=false` are refused before git, and a policy refusal at apply does not consume the record; a delete that already landed inspects as `landed` and a later apply refuses without running git. Falsified by making the merged case use `-D` (`a_merged_branch_deletes_with_d_and_no_force` failed at "must use -d"), by ignoring `confirm_unmerged` (the unmerged test failed because the plain approval returned `Ok`), and by disabling the current-branch check (the checkout test failed on the refusal text; the worktree scan would still have blocked it), then restoring. Scoped checks: `cargo fmt --all`; `cargo clippy -p workspace-engine --all-targets --locked -- -D warnings` clean; `cargo nextest run -p workspace-engine --test branch_delivery --test permission_profiles` 115 passed, 1 skipped. Deviations: `unmerged_commits` is `Option<u32>` rather than the proposal's `u32` so D11's unknown state is representable and never looks merged; `apply` gained `confirm_unmerged` (Task 4 call sites pass false); `BranchPreparation` and `BranchRecord` carry `unmerged_subjects` (the record field is serde-default for existing records); a branch checked out in any worktree is refused, not only this checkout; a successful deletion keeps `expected_ref_oid` so inspect can tell a re-created ref from the removed one. No `chat.rs` change. Task 6 starts from the now-stable `GitMutation` and `apply` surface; merge/rebase preparation is read-only and needs no deletion path, and `default_branch_rev`/`unmerged_report` are private to deletion. |
| 6 · Merge and rebase preparation without executing either | Done 2026-10-28 | Added `MergePreparation` and `RebasePreparation` (camelCase serde) and `BranchEngine::prepare_merge(base, session_id)` / `prepare_rebase(onto, session_id)`, re-exported from `lib.rs`. `prepare_merge` validates `base` through the public `merge_base` read (an option-shaped base is refused before git), resolves the merge base, lists `HEAD..<base>` as `<abbrev> <subject>`, records `ahead`/`behind` counts (`<base>..HEAD` / `HEAD..<base>`), computes fast-forward with `merge-base --is-ancestor HEAD <base>`, and runs `git merge-tree --write-tree --name-only --end-of-options <base> HEAD` through a new `GitService::merge_tree` that returns the exit status rather than treating exit 1 as an error. The parser is D12 exactly: the first line is the tree id (accepted only as 40 or 64 hex characters, so the id can never look like an option), lines up to the first blank line are conflicted paths, the rest is informational; exit 0 is clean, exit 1 conflicts, and any other status or contradictory shape is unavailable with Git's stderr. Changed files come from a new `GitService::diff_names("HEAD", tree)` that uses `read_git`'s `diff.autoRefreshIndex=false`, so reading the diff cannot refresh the index. Restricted paths are omitted from changed and conflicted lists. A merge base that does not resolve (unrelated histories or a missing ref) short-circuits to `problem` and `conflict_prediction_available: false` without running merge-tree; there is no real-merge fallback. `prepare_rebase` reports onto and its OID, the merge base, and `<merge-base>..HEAD` as commits to replay, and always gives D13's reason (it would require replaying); it runs no rebase. Audit is `git_merge_prepared` (base, merge base, incoming/changed/conflicted counts, availability, outcome) and `git_rebase_prepared` (onto, onto OID, merge base, replay count, availability false, outcome), never subjects or bodies. Five integration tests in `tests/branch_delivery.rs`: clean non-overlapping merge (incoming commit, changed file, exit 0, audit, no body), overlapping merge (exit 1 and `conflict.txt`), fast-forward (`fast_forward: true`), unrelated histories and a missing ref (prediction unavailable, merge-tree not run, option-shaped base refused before git), and a rebase plan (two replayed commits, onto OID, D13 reason, audit without subjects); each compares index bytes, `HEAD`, `git status --porcelain=v2` and `.git/MERGE_HEAD` before and after. Four unit tests in `branch.rs` cover the D12 parser, including synthetic unsupported-`--write-tree` output. Observed Git 2.56.0; the unsupported path was exercised at the parser boundary with exit 129 output because the installed Git supports the flag, while the end-to-end unavailable path uses unrelated histories. Falsified by making the parser treat the tree id as a conflicted path (the parser test failed with the tree id first in the list) and by inserting `git merge --no-commit --no-edit` before `merge-tree` (the clean-merge test failed with a changed index, changed status and `MERGE_HEAD = true`), then restoring. Scoped checks: `cargo fmt --all`; `cargo clippy -p workspace-engine --all-targets --locked -- -D warnings` clean; `cargo nextest run -p workspace-engine --test branch_delivery` 37 passed; `cargo nextest run -p workspace-engine --lib branch::tests` 4 passed. Git version and fallback exercise are also recorded in `proposal.md` §7. No `chat.rs` change. Task 7 starts from the two read-only report types and their engine methods: they need no session mode or capability gate and have no apply/record path, and `GitService::merge_tree`/`diff_names` stay crate-private. |
| 7 · Shell routes and the branch dialog | Done 2026-10-28 | Added `GET /api/branch-info` (repo + session; `BranchInfo` plus `branches`), `POST /api/branch-suggestion` (one `source` from the dialog, otherwise the session title or the last user message; never a repository file), `POST /api/branch-prepare` (`operation=create|switch|delete`), `POST /api/branch` (`approval=branch` only; the exact prepared `GitMutation` is sent back as JSON and compared by the engine; optional `confirm_unmerged=true`), and `GET /api/merge-preparation` (`mode=merge|rebase`, `base`). Refusals keep the commit routes' `{error, code, message}` shape; a refused preparation is 409 with the preparation attached, so the dialog can show the reason and a collision alternative. No route executes merge, rebase, or cherry-pick and no route accepts a command. The folder menu gains *Branch…*; the dialog shows current branch, upstream, ahead/behind, in-progress operation, and conflicted paths, prefills an editable name from the suggestion, offers a free alternative without changing the field, treats Create and Switch as separate preparation + approval cards (an optional "Switch to it after creating" checkbox asks for the switch as a second approval), offers deletion only for non-current branches with a first card naming the ref and its merged status (or count and subjects) and a second, differently worded card before `-D`, and renders the merge/rebase report as text ending "Nothing was merged or changed. Damaian ran no merge and made no commit." / "Nothing was replayed." Approval cards focus Cancel rather than the confirming button. Ten route tests in `branch_route_tests`: info (and a non-repository 400), suggestion from the session and from a dialog source including a seeded key absent from the name, `approval=commit` refused with no ref, Ask mode 409 with no record or ref, a real create with the ref present and byte-identical HEAD/index/status, a switch that moves HEAD and the working tree, a merged delete using `-d`, an unmerged delete refused twice and deleted with `-D` only after `confirm_unmerged`, the current branch refused, merge and rebase preparation with byte-identical index/HEAD/status and no MERGE_HEAD/REBASE_HEAD, and 404s for merge/rebase/cherry-pick routes. Falsified by accepting `approval=commit` on `/api/branch`: the approval test failed with 200 and a created ref; restored. Scoped checks: `cargo fmt --all`; `cargo clippy -p desktop-shell --all-targets --locked -- -D warnings` clean; `cargo nextest run -p desktop-shell --lib` 220 passed, 6 skipped; `node --check`; `npm ci` then `npm run lint:web` clean after `npm run lint:web:fix`. Deviations: `branch-info` requires `session` (validated) and also returns `branches` so the dialog can offer switch/delete targets; a refused preparation is HTTP 409 with the preparation attached instead of a 200 carrying a refusal field; the apply route takes the prepared mutation as JSON rather than rebuilding it from form fields, so the approval names exactly what was reviewed. A concurrent session wrote a second branch-dialog implementation into the same files during this task; it was removed in favour of this one and the route contract above, and its `USER_GUIDE.md`/`TROUBLESHOOTING.md` edits were left uncommitted. No manual browser walk: the web UI bootstraps through Tauri's desktop API token, so it cannot be exercised from a plain browser here. No `chat.rs` change. Task 8 starts from the final route contract (branch-info takes `repo` and `session`) and the two-card delete flow; docs should describe separate create/switch approvals and the report's sentence that nothing changed. |
| 8 · Docs, acceptance walk and close-out | Done 2026-10-28 | Documented the shipped branch delivery. `USER_GUIDE.md` gains a **Branches** section and contents entry: the folder-menu entry, suggested/editable name and the `damaian/` prefix, Create and Switch as separate approvals, merged versus unmerged deletion, and the merge/rebase report that changes nothing. `TROUBLESHOOTING.md` gains "A branch operation was refused, or its outcome is unknown" (detached `HEAD` and in-progress operations, dirty switch, invalid/colliding name, mode and `allow_git_branches`, the unmerged second confirmation, conflict prediction unavailable on older Git, `branches/<prepare-id>.json`, the audit verbs, and the absence of a branch-specific recovery **Inspect** row) plus a row for the prepared records in the data-directory table. `SECURITY.md` gains "Git Branches" (one typed approval per `GitMutation`, Code mode plus `allow_git_branches`, no force/merge/rebase/cherry-pick, names from redacted text, restricted paths, side-effecting markers and audit) and adds the key to the repository trust-boundary list. `proposal.md` §7 now records what shipped, why §5.7 stayed dormant under D7, every refused state with its message, twelve deviations from the proposal, the conflict-prediction limitation, a 17-row criterion-to-test acceptance walk (the §5.7 worktree rows conditional, the quality-gate row outstanding), and the manual-walk gap. `README.md` marks #36 Done with the CI run outstanding, removes it from the ready and in-progress sets, records that nothing is promoted because #37 still needs #33 (not built), updates #37's `Depends on:` line to built, and updates the priority and implementation-order rows. `CHANGELOG.md` untouched, as instructed. `node scripts/check-spec-status.mjs` reports 0 disagreements after this row. Scoped checks: `scripts/quality-gate.sh fmt node lint typos deny` clean; `cargo clippy -p workspace-engine -p desktop-shell --all-targets --locked -- -D warnings` clean; `cargo nextest run -p workspace-engine --test branch_delivery` 37 passed; `cargo nextest run -p desktop-shell --lib` 220 passed (6 skipped); `node --check` and `npm run lint:web` clean. No manual browser walk, stated in §7 and the Task 7 row. **CI has not run for this branch**, so the workspace clippy/test gate and the full seven-check gate are not claimed as passed. |

**Goal:** A Code-mode session can suggest and create a branch, switch to it,
delete it with a distinct confirmation when commits would be lost, and report
what a merge or rebase would do — without ever executing a merge, rebase, or
cherry-pick and without ever constructing a force operation.

**Architecture:** Read-only branch state and merge-base resolution join
`git_service.rs`. A new `workspace-engine/src/branch.rs` holds naming, the
`GitMutation` type, the prepared-record store under `<data_dir>/branches/`, and
the mutation paths through `checkpoint::run_user_git`. `BranchInfo` is serialised
to the desktop shell, which adds routes and a folder-menu dialog. No new
dependency.

**Tech Stack:** Rust 2024, the existing `serde`/`serde_json`, the existing
session and audit logs, vanilla JavaScript. No framework, no bundler.

## Global Constraints

Every task's requirements implicitly include this section.

- **Read [`context.md`](context.md) in full before Task 1.** Its §3 corrections
  override the proposal's §2, and §4's D1–D15 are the decisions the tasks
  implement. Do not re-derive them from `proposal.md` alone.
- **Do not edit `crates/workspace-engine/src/chat.rs`.** Another spec owns it in
  a parallel worktree. Branch operations are user actions, not model tools (D2).
  If a task appears to need `chat.rs`, stop, record why in `context.md` and the
  progress row, and report rather than breaking the pairing.
- **Preparation never executes a merge, rebase, or cherry-pick.** The only merge
  computation is `git merge-tree --write-tree`, which must not touch the index,
  the working tree, or `HEAD`.
  There is no `merge --no-commit` fallback, no `rebase`, and no `cherry-pick`
  command anywhere in this spec's code.
- **No command this spec constructs contains `-f` or `--force`.** Creation uses
  `git branch` (which refuses an existing ref); switching uses `git switch
  --no-guess`; deletion uses `git branch -d` unless the distinct unmerged
  confirmation was given, in which case `-D` is the only exception. Test that
  the constructed argv contains neither short nor long force flag.
- **Push, fetch with credentials, tags, and PR creation are spec 37.** Never
  push, never tag, never write a remote.
- **Each mutation is its own typed approval.** A create approval cannot switch,
  a switch approval cannot delete, and an unmerged deletion needs a distinct
  confirmation naming the count and commit subjects. No `Allow Always`.
- **A branch operation is a capability.** Code mode only; the resolved profile
  must permit `allow_git_branches`; repository config may deny it and may never
  grant it. No settings UI may bypass the engine check.
- **Names are validated through Git, never silently altered.** The composed
  candidate runs through `git check-ref-format --branch`; a collision is reported
  with a free suffixed alternative, and the original is not changed. A prefix is
  checked in Rust first (D6, context §3.14).
- **Branch suggestions are built from redacted text.** The source title/request
  is passed through `SecretScanner::redact` before slugging; no raw title is
  returned or audited (D10).
- **Never operate on the user's checkout or another worktree.** Tests create
  their own temporary repository. A manual run uses a temporary
  `DAMAIAN_DATA_DIR` and never binds `127.0.0.1:4765`.
- **Fixtures:** `enable_index_watcher: false`; the data directory sits beside the
  temporary repository, not inside it. Real refs change only in temporary
  repositories.
- **Scoped checks per task; the full gate runs on CI only** (AGENTS.md since
  2026-10-15). Each task runs `cargo fmt --all`, scoped
  `cargo clippy -p <crate> --all-targets --locked -- -D warnings` for every crate
  it touches, and the named tests. Never `cargo clippy --workspace` or
  `cargo nextest run --workspace` locally.
- **Tests first, and falsify every load-bearing test.** Write the named failing
  test, confirm it fails, implement, then break what the test guards, confirm it
  fails, restore, and record the mutation in the progress row. A test that
  passes without exercising the guard is not evidence.
- **Do not move the CHANGELOG.** This spec adds nothing to or removes anything
  from `Unreleased`; the planner handles changelog movement at close-out.
- **Commit locally after each task** with a short descriptive subject about the
  effect, not the process. The user pushes.

## File Structure

| File | Change |
|---|---|
| `crates/workspace-engine/src/git_service.rs` | `BranchInfo`, `branch_info`, public `merge_base`, branch-name validation and existence reads (Tasks 1–2) |
| `crates/workspace-engine/src/branch.rs` | **New.** Slug/naming, `GitMutation`, prepared record, creation/switch/delete, merge and rebase preparation (Tasks 2, 4–6) |
| `crates/workspace-engine/src/config.rs` | `git_branch_prefix`, `allow_git_branches` (Tasks 2–3) |
| `crates/workspace-engine/src/profile.rs` | Read-only profile denies branch mutation (Task 3) |
| `crates/workspace-engine/src/effective_policy.rs` | Capability label (Task 3) |
| `crates/workspace-engine/src/lib.rs`, `workspace_engine.rs` | Re-exports and the `BranchEngine` (Tasks 1–6) |
| `crates/workspace-engine/tests/branch_delivery.rs` | **New.** Engine-level acceptance tests (Tasks 1–6) |
| `crates/workspace-engine/tests/permission_profiles.rs` | Config-key record updates and the capability's weakening case (Tasks 2–3) |
| `crates/desktop-shell/src/lib.rs` | Branch, suggestion, delete and merge-preparation routes (Task 7) |
| `crates/desktop-shell/static/app.js`, `index.html`, `style.css` | Folder-menu entry, branch dialog, delete confirmation, merge report (Task 7) |
| `docs/USER_GUIDE.md`, `docs/TROUBLESHOOTING.md`, `SECURITY.md` | Task 8 |
| `docs/specs/README.md`, `proposal.md`, this file | Task 8 |

## Order and parallelism

```
1 ──> 2 ──> 3 ──> 4 ──> 5 ──> 6 ──> 7 ──> 8
```

Sequential. Task 1 defines the state every later refusal reads. Task 2's naming
is the input Task 4 prepares. Task 3 defines the gate Task 4 checks. Task 5
reuses Task 4's record and marker machinery. Task 6 is independent engine work
but follows so the mutation surface is stable first. Task 7 needs the engine
surface; Task 8 evaluates the finished repository. No task pairs with a
`chat.rs` spec because no task edits that file.

---

### Task 1 · Read-only branch information

**Files:** `crates/workspace-engine/src/git_service.rs`,
`crates/workspace-engine/src/lib.rs`,
`crates/workspace-engine/tests/branch_delivery.rs` (new).

**Goal:** one typed read that answers current branch, `HEAD`, detached state,
upstream, ahead/behind, in-progress operation, and conflicted paths, plus a
public validated merge-base. It must not change a ref, the index, or the working
tree.

- [x] Add `BranchInfo` with camelCase serde:
      `current: Option<String>`, `head_oid: Option<String>` (D5),
      `detached: bool`, `upstream: Option<String>`, `ahead: u32`, `behind: u32`,
      `in_progress_operation: Option<String>`, `conflicted_paths: Vec<String>`.
- [x] Add `GitService::branch_info`; return a clear `InvalidInput` error for a
      folder that is not a Git work tree. Read the branch with
      `symbolic-ref --short -q HEAD`, the full SHA with
      `rev-parse --verify -q HEAD`, the upstream with
      `rev-parse --abbrev-ref --symbolic-full-name @{upstream}`, and ahead/behind
      with the existing `ahead_behind` helper (`git_service.rs:785`). No upstream
      means `0/0`, not an error.
- [x] Detect an in-progress operation by checking
      `git rev-parse --git-path <marker>` for `MERGE_HEAD`, `CHERRY_PICK_HEAD`,
      `REVERT_HEAD`, `rebase-merge`, `rebase-apply`, `sequencer`, and
      `BISECT_LOG`. Return a short stable label (`merge`, `cherry-pick`,
      `revert`, `rebase`, `sequencer`, `bisect`), not the marker filename alone.
      Do not reuse `commit.rs`'s private list by editing `commit.rs` in this
      task; context §3.12/§4 D4 records the duplication and a later task may
      unify it.
- [x] Read conflicted paths with `git diff --name-only --diff-filter=U -z
      --no-ext-diff --no-textconv`, then drop restricted paths with the existing
      `path_is_restricted` rule. This is cheaper and more complete than
      `changes`.
- [x] Audit one `git_branch_read` event with the root, whether it succeeded, and
      counts only; no path names of restricted files and no branch content.
- [x] Make `merge_base(root, base)` public, validate `base` with the existing
      `validate_base` first, and document that it is a pure read. `diff_in`
      continues to use it.
- [x] Re-export `BranchInfo` from `lib.rs`.
- [x] Tests in `tests/branch_delivery.rs`: a repository with a local bare
      `origin` reports upstream and ahead/behind (0/0, then 1/0, then 1/1 using
      refs moved only in the fixture); a detached `HEAD` reports
      `current: None`, `detached: true`; an unborn repository reports the branch
      name, `head_oid: None`, `detached: false`; a real conflicted merge reports
      `in_progress_operation: Some("merge")` and the conflicted path; a clean
      repository reports `None` and an empty list; `merge_base` resolves the fork
      point and rejects an option-shaped base.
- [x] Assert the read changes nothing: hash `.git/index`, `HEAD`'s full SHA, and
      `git status --porcelain=v2` before and after `branch_info` and
      `merge_base`, and compare them byte for byte.
- [x] Run `cargo fmt --all`; scoped checks:
      `cargo clippy -p workspace-engine --all-targets --locked -- -D warnings`
      and `cargo nextest run -p workspace-engine --test branch_delivery`.
- [x] Falsify: temporarily make `branch_info` report the upstream ahead count as
      behind (or skip the merge-marker scan) and confirm the named test fails;
      restore.
- [x] Update this task's progress row and commit.

### Task 2 · Suggested branch name

**Files:** `crates/workspace-engine/src/config.rs`,
`crates/workspace-engine/src/branch.rs` (new),
`crates/workspace-engine/src/git_service.rs`,
`crates/workspace-engine/src/lib.rs`,
`crates/workspace-engine/tests/branch_delivery.rs`,
`crates/workspace-engine/tests/permission_profiles.rs`.

**Goal:** a suggested name derived from the task title or request, prefixed by a
configurable pattern, validated with `git check-ref-format --branch`, with a
collision reported and a free suffixed alternative offered. It changes no ref.

- [x] Add `git_branch_prefix` to `Config` (default `damaian/`) and
      `ConfigOverlay` (`Option<String>`); parse and serialise it; classify it as
      `Preference`; apply it through `preference()` so a repository may set it
      and a profile may not; include it in `to_policy_text`. Update
      `the_preference_keys_are_exactly_spec_34s_free_keys` by adding the key, and
      add a repository-scope load test proving it is honoured.
- [x] Add `branch.rs` with:
      `slugify_branch_title(text) -> String` (lowercase ASCII alphanumerics,
      every other run collapsed to `-`, leading/trailing `-` trimmed, at most 50
      characters, `change` when empty);
      `validate_branch_prefix(prefix) -> Result<()>` (reject empty? no: an empty
      prefix is allowed; reject leading `-` or `/`, `..`, `@{`, whitespace and
      control characters, `//`, a trailing `/`, a component ending `.lock`, and a
      trailing `.`; context §3.14);
      and `BranchSuggestion { name, valid, refusal: Option<String>, exists,
      alternative: Option<String> }`.
- [x] Add `branch::suggest_branch_name(git, scanner, root, prefix, source)`.
      Redact `source` through `scanner.redact(...).text` before slugging (D10),
      compose `<prefix><slug>`, validate the prefix in Rust, validate the
      composed name with `git check-ref-format --branch <name>`, and check
      collisions against the local branch list. On collision keep `name`
      unchanged, set `exists: true`, and set `alternative` to the first free
      `<name>-2`, `<name>-3`, ….
- [x] Add `GitService::check_branch_name` (runs `check-ref-format --branch` and
      returns the reason on failure), `local_branches` (one
      `for-each-ref --format=%(refname:short) refs/heads/`), and
      `branch_exists`. All go through `read_git`; none mutates.
- [x] Tests: slug lowercases and collapses punctuation; truncation; empty source
      falls back to `change`; a configured prefix is used; a prefix containing
      `@{` is refused before git runs; a name git rejects (for example one ending
      in `.lock`) is reported invalid with a reason; a colliding name is
      reported with the `-2` alternative and is not silently changed; a source
      containing a seeded fake AWS key produces a name with no key fragment and
      no audit line containing the key.
- [x] Run `cargo fmt --all`; scoped checks:
      `cargo clippy -p workspace-engine --all-targets --locked -- -D warnings`,
      `cargo nextest run -p workspace-engine --test branch_delivery`, and
      `cargo nextest run -p workspace-engine --test permission_profiles`.
- [x] Falsify: remove the redaction call and confirm the key test fails; restore.
- [x] Update the progress row and commit.

### Task 3 · The `allow_git_branches` capability

**Files:** `crates/workspace-engine/src/config.rs`,
`crates/workspace-engine/src/profile.rs`,
`crates/workspace-engine/src/effective_policy.rs`,
`crates/workspace-engine/tests/permission_profiles.rs`.

**Goal:** branch mutation is a capability a profile or repository can deny, and
repository config cannot grant it. No behaviour is wired until Task 4.

- [x] Add `allow_git_branches` beside `allow_git_commits`: `Config` default
      `true`, `ConfigOverlay` field, parse, serialise, `to_policy_text`,
      `ConfigKeyKind::Capability`, restrict-only in `apply_overlay_scoped`, and
      the effective-policy label "Git branches allowed".
- [x] Add it to `ProfileCapabilities`, `Config::profile_capabilities`, the
      Read-only profile (`false`), profile split/import plumbing, and every
      exhaustive table in `profile.rs`.
- [x] Tests: the weakening case in `tests/permission_profiles.rs` proves a
      repository can set it false but not true; the Read-only profile resolves
      false; the built-in profile table expects the new column; the capability
      has a human-readable label.
- [x] Run `cargo fmt --all`; scoped checks:
      `cargo clippy -p workspace-engine --all-targets --locked -- -D warnings`
      and `cargo nextest run -p workspace-engine --test permission_profiles`.
- [x] Falsify: make the repository overlay accept `true`, confirm the weakening
      test fails, restore.
- [x] Update the progress row and commit.

### Task 4 · Branch creation and switching

**Files:** `crates/workspace-engine/src/branch.rs`,
`crates/workspace-engine/src/lib.rs`,
`crates/workspace-engine/src/workspace_engine.rs`,
`crates/workspace-engine/tests/branch_delivery.rs`.

**Goal:** prepared, typed, one-shot approvals for `CreateBranch` and
`SwitchBranch`; creation leaves `HEAD` unchanged; switching updates the index
and working tree; every refusal names why.

- [x] Define `GitMutation` with one variant per operation:
      `CreateBranch { name, start_point }` and `SwitchBranch { name }` (add
      `DeleteBranch` in Task 5). There is no catch-all variant.
- [x] Define the durable `BranchRecord` and its `<data_dir>/branches/` store:
      `prepare_id`, repository root, session id, mutation, resolved start SHA,
      expected state, `BranchInfo` snapshot, state
      (`prepared`/`applied`/`failed`), `created_ref`, `applied_at_ms`, and the
      command lines actually run. A `prepare_id` is loaded and compared at apply
      time; a record not in `prepared` state is refused.
- [x] `BranchEngine` in `branch.rs`, built in `workspace_engine.rs` with the
      config, audit log, scanner, session store, path policy and `GitService`.
      `prepare_create(name, start_point, session_id)` and
      `prepare_switch(name, session_id)` return a `BranchPreparation` with
      `refusal: Option<String>`, `can_apply`, the mutation, the suggestion, and
      the start point. Refuse: non-repository, unknown session, any mode other
      than Code, `allow_git_branches == false`, any `in_progress_operation`,
      detached `HEAD` for creation without an explicit start point, unborn
      `HEAD` for creation, an invalid name, and an existing target ref.
- [x] Resolve the start point with
      `git rev-parse --verify -q --end-of-options <start>^{commit}`, defaulting
      to `HEAD`; reject an option-shaped or unresolvable value before any
      branch command. Report the original and resolved SHA.
- [x] Creation runs `git branch <name> <resolved-sha>` through
      `checkpoint::run_user_git`; it never switches. Switching runs
      `git switch --no-guess <name>` (D6), never `symbolic-ref`, so the index and
      working tree move with `HEAD`.
- [x] Dirty-switch refusal (D15): before switching, intersect the paths that
      differ between `HEAD` and the target with the working tree's changed and
      untracked paths, and refuse naming those paths. Let Git's own check remain
      the backstop.
- [x] `apply(prepare_id, approved_mutation, approved_by)` requires the exact
      mutation value from the record and consumes the record. A second apply, a
      different mutation, or a stale record is refused without running git.
      There is no persistent approval.
- [x] Bracket the mutation with the synthetic task + `start_action(..., true)` /
      `finish_action` pattern from `commit.rs:781`–`:843`, and audit
      `git_branch_created` / `git_branch_switched` with refs, start point,
      approvedBy, and outcome. Record `git_branch_refused` for a policy refusal.
- [x] `inspect(prepare_id)` compares `show-ref --verify refs/heads/<name>` with
      the record's expected state and reports `landed`/`not landed`/`other ref
      state`; it never retries.
- [x] Tests: creation leaves `HEAD` and the index unchanged and the ref exists at
      the start point; create-then-switch moves `HEAD` and the working tree to
      the new branch; switching an existing branch updates files; Ask/Plan/Review
      and `allow_git_branches=false` are refused; mid-merge and detached states
      are refused with the reason; a collision and an invalid name are refused;
      a dirty switch names the paths; a create approval cannot apply a switch and
      a second apply is refused; a dangling record's `inspect` reports the ref
      truthfully; the constructed command lines contain neither `-f` nor
      `--force`; the marker pair and audit exist.
- [x] Run `cargo fmt --all`; scoped checks:
      `cargo clippy -p workspace-engine --all-targets --locked -- -D warnings`,
      `cargo nextest run -p workspace-engine --test branch_delivery`, and
      `cargo nextest run -p workspace-engine --test permission_profiles`.
- [x] Falsify: make creation use `git branch -f`, confirm the force-flag and
      existing-ref tests fail; restore. Make `apply` reuse a record twice and
      confirm the consumption test fails; restore.
- [x] Update the progress row and commit. If this task grows past the creation
      and switching boundary, stop at the last coherent point and record the
      deviation rather than folding deletion in.

### Task 5 · Branch deletion with the unmerged confirmation

**Files:** `crates/workspace-engine/src/branch.rs`,
`crates/workspace-engine/tests/branch_delivery.rs`.

**Goal:** delete a fully merged branch with ordinary approval; require a
distinct confirmation that names the count and subjects when commits would be
lost; never delete the current branch.

- [x] Add `GitMutation::DeleteBranch { name, unmerged_commits }`.
- [x] `prepare_delete(name, session_id)` refuses the current branch, a missing
      branch, an in-progress operation, and a non-Code/denied session. Compute
      `git rev-list --count <branch> --not --remotes --not <default>` and the
      subject lines of those commits with
      `git log --format=%s <branch> --not --remotes --not <default>`. Resolve the
      default branch in D11's order; when none resolves, mark the count as
      unknown and require the unmerged path.
- [x] `apply` for a fully merged branch runs `git branch -d <name>`; for an
      unmerged branch the first approval is refused with the count and subjects,
      and a distinct `confirm_unmerged` confirmation (same prepared id, same
      mutation, explicitly set) runs `git branch -D <name>`. Any other case
      refuses without running git.
- [x] Use the Task 4 marker and audit path with `git_branch_deleted`; the audit
      records the ref, count, whether the second confirmation was used, and the
      outcome, never commit message bodies.
- [x] Tests: a merged branch deletes with `-d`; an unmerged branch reports the
      count and subjects, refuses the first approval, and deletes with `-D` only
      after the distinct confirmation; the current branch is refused before git;
      a missing branch is refused; the command lines contain no force flag except
      the confirmed `-D`; the audit names the confirmation.
- [x] Run `cargo fmt --all`; scoped checks:
      `cargo clippy -p workspace-engine --all-targets --locked -- -D warnings`
      and `cargo nextest run -p workspace-engine --test branch_delivery`.
- [x] Falsify: use `-D` for the merged case and confirm the merged-branch test
      fails; restore.
- [x] Update the progress row and commit.

### Task 6 · Merge and rebase preparation without executing either

**Files:** `crates/workspace-engine/src/branch.rs`,
`crates/workspace-engine/src/git_service.rs` (only if a read helper is needed),
`crates/workspace-engine/tests/branch_delivery.rs`.

**Goal:** report what a merge would do — merge base, incoming commits, changed
files, conflicted paths, fast-forward — using `merge-tree --write-tree`, with no
effect on the index, working tree, or `HEAD`, and an honest rebase report.

- [x] `prepare_merge(base, session_id)` validates `base`, resolves its merge base
      with `HEAD`, lists incoming commits (`git log --format='%h %s'
      HEAD..<base>`), runs
      `git merge-tree --write-tree --name-only <base> HEAD`, and derives changed
      files from the resulting tree versus `HEAD` (`git diff --name-only
      HEAD <tree>`). Parse per D12: first line is the tree id, following lines up
      to the first blank line are conflicted paths, the rest is informational.
      Exit 0 is clean, exit 1 is conflicts, any other failure is unavailable.
- [x] Compute `fast_forward` with
      `git merge-base --is-ancestor HEAD <base>` (exit 0 means the merge would
      fast-forward). Report the merge base, incoming commits, changed files,
      conflicted paths, and a `conflict_prediction_available` flag with the
      reason when unavailable. Never run `git merge`, `--no-commit`, or any
      index-writing command.
- [x] On an unsupported `merge-tree --write-tree`, report conflict prediction as
      unavailable with the Git error and still report base/ahead/behind. Do not
      fall back to a real merge (D12).
- [x] `prepare_rebase(onto, session_id)` reports the merge base, the commits that
      would be replayed (`merge-base..HEAD`), the onto ref, and states that
      conflict prediction for a rebase is unavailable because it would require
      replaying (D13). It replays nothing.
- [x] Audit `git_merge_prepared` / `git_rebase_prepared` with the base, counts,
      conflict count, availability, and no commit message bodies.
- [x] Tests: a clean non-overlapping merge reports no conflicts and the expected
      changed files and incoming commit; an overlapping merge reports the
      conflicted path and exit status 1; a fast-forward case reports
      `fast_forward: true`; unrelated histories or an unresolvable base reports
      prediction unavailable and no merge is attempted; a rebase plan lists the
      right commits and onto and does not claim conflict prediction; the index
      hash, `HEAD` SHA, and `git status --porcelain=v2` are byte-identical before
      and after every preparation; no test observes a `.git/MERGE_HEAD`.
- [x] Run `cargo fmt --all`; scoped checks:
      `cargo clippy -p workspace-engine --all-targets --locked -- -D warnings`
      and `cargo nextest run -p workspace-engine --test branch_delivery`.
- [x] Falsify: make the parser treat the tree id as a conflicted path and confirm
      the parse test fails; make preparation call `git merge --no-commit` and
      confirm the byte-identical test fails; restore.
- [x] Record the observed Git version and any fallback exercise in the progress
      row and `proposal.md` §7. Update the progress row and commit.

### Task 7 · Shell routes and the branch dialog

**Files:** `crates/desktop-shell/src/lib.rs`,
`crates/desktop-shell/static/app.js`,
`crates/desktop-shell/static/index.html`,
`crates/desktop-shell/static/style.css`.

**Goal:** the engine surface is reachable from the app: branch state and a
suggestion, create/switch with their own approvals, delete with the two-step
confirmation, and a merge report that states it changed nothing.

- [x] Add routes: `GET /api/branch-info`,
      `POST /api/branch-suggestion`, `POST /api/branch-prepare`,
      `POST /api/branch` (requires `approval=branch`; optional
      `confirm_unmerged=true`), and `GET /api/merge-preparation`
      (`mode=merge|rebase`). Map typed refusals to `{error, code, message}` with
      the same shape commit uses (`lib.rs:1792`).
- [x] Reuse `engine_for_repo` and the session parameter; never derive a branch
      name from a repo-supplied file, and never accept a raw command.
- [x] Add a *Branch…* item to the folder menu (`app.js:1354`) and a dialog that
      shows current branch, upstream, ahead/behind, in-progress operation,
      conflicted paths, and any refusal. The name field is editable and prefilled
      from the suggestion; a collision shows the free alternative and does not
      silently change the field; create and switch are separate checkboxes /
      actions with separate approvals.
- [x] Implement the delete flow as two cards: the first names the ref and says
      it is fully merged, or names the count and commit subjects and requires a
      second, differently worded confirmation before `-D`. The current branch is
      not offered.
- [x] Implement the merge report as read-only text: merge base, incoming
      commits, changed files, conflicted paths, fast-forward yes/no, and an
      explicit sentence that nothing was merged or changed. There is no merge
      button. The rebase report lists commits and onto and says conflict
      prediction is unavailable.
- [x] Route tests in `desktop-shell/src/lib.rs`: info JSON, suggestion, missing
      approval → 400 with no ref, Ask mode → 409, a real create through the route
      with the ref present and HEAD unchanged, a switch through the route, the
      delete two-step, a merge preparation with byte-identical index/HEAD/status,
      and no route that executes merge/rebase.
- [x] If a manual browser walk is not possible (the web UI bootstraps through
      Tauri), say so in the progress row rather than claiming it.
- [x] Run `cargo fmt --all`; scoped checks:
      `cargo clippy -p desktop-shell --all-targets --locked -- -D warnings`,
      `cargo nextest run -p desktop-shell --lib`,
      `node --check crates/desktop-shell/static/app.js`; run `npm ci` then
      `npm run lint:web`. If web assets are not yet formatted, run
      `npm run lint:web:fix` before linting.
- [x] Falsify: accept `approval=commit` for a branch and confirm the route test
      fails; restore.
- [x] Update the progress row and commit.

### Task 8 · Docs, acceptance walk and close-out

**Files:** `docs/USER_GUIDE.md`, `docs/TROUBLESHOOTING.md`, `SECURITY.md`,
`docs/specs/36_branch_and_worktree_delivery/proposal.md`,
`docs/specs/36_branch_and_worktree_delivery/tasks.md`,
`docs/specs/README.md`.

**Goal:** the shipped behaviour is documented, the acceptance criteria are
evidenced or honestly marked conditional, and the spec records what shipped.

- [x] `USER_GUIDE.md`: branch suggestion and creation, that creation and
      switching are separate approvals, what the merge report says and that it
      changes nothing, and why deleting a branch with unmerged commits asks
      twice.
- [x] `TROUBLESHOOTING.md`: what a detached `HEAD`, mid-merge, mid-rebase, or
      bisect prevents and why; what to do when conflict prediction is
      unavailable; where branch operations appear in the audit log; which data
      files (`branches/`) the prepared records live in.
- [x] `SECURITY.md`: branch mutation is a Code-mode capability
      (`allow_git_branches`), repository config may deny and never grant it,
      approvals are per operation and consumed once, no force or merge is
      executable here, and branch names are built from redacted text.
- [x] `proposal.md` §7: what shipped, the minimum Git version for `merge-tree
      --write-tree`, how often the fallback was taken, whether §5.7 stayed
      reporting-only, every state refused rather than handled with its message,
      any deviation, and a criterion-to-test table.
- [x] Walk the acceptance criteria in §6 and record each as met with the test
      that proves it, or conditional (the §5.7 worktree items, which are
      reporting-only per D7). Do not mark a manual browser walk as done if it was
      not performed.
- [x] Update the README implementation-order row and current-state/history
      paragraphs to **Done** with the CI run when it is known. Do **not** edit
      `CHANGELOG.md`; this repository's planner moves the `Unreleased` entry at
      close-out.
- [x] Run `node scripts/check-spec-status.mjs` and resolve any record
      disagreement.
- [x] Run the scoped checks for every crate touched by the finished feature,
      `scripts/quality-gate.sh fmt node lint typos deny`, and report that the
      workspace clippy/test gate is left to CI. Do not describe the work as
      having passed the quality gate before CI is green.
- [x] Record the final progress row, set `**Done:**`, and commit.

---

## Notes for the next task session

- Tasks 1 and 2 are planned as the complete read-only foundation: after them,
  nothing in the feature can mutate a ref yet. That is deliberate.
- The proposal's §5.7 is not the first thing to build. Spec 57 already creates
  worker worktrees and spec 60 already lists them; D7 keeps this spec to
  reporting. Do not add worktree creation here.
- The switch command in the proposal (`symbolic-ref`) is wrong; D6 and context
  §3.17 explain why and what to run instead.
- If a task needs `chat.rs`, stop and record it. Nothing in this plan should.
