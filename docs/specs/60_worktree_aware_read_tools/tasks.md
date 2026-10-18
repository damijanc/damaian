# Worktree-Aware Read Tools Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Implements:** [`proposal.md`](proposal.md) · background in [`context.md`](context.md)
**Started:** 2026-10-17 (planned the same day; references checked against the tree on 2026-10-17)

## Progress

| Task | State | Notes |
|---|---|---|
| 1 · Registered-worktree resolution and refusal | Done 2026-10-17 | Details in the note below the table. |
| 2 · Git read hardening on the main checkout | Done 2026-10-17 | Details in the note below the table. |
| 3 · `worktree` on the five read tools | Done 2026-10-18 | Details in the note below the table. |
| 4 · `list_worktrees` and `read_git_log` | Done 2026-10-18 | Details in the note below the table. |
| 5 · `worktree_survey` eval scenario | Not started | |
| 6 · Docs, live re-run and close-out | Not started | |

**Task 1 (Done 2026-10-17).** New module `worktree_access.rs`:
`parse_worktree_list` reads `git worktree list --porcelain -z` (NUL-terminated
fields, so a path with a newline cannot forge an entry), `GitService::worktrees`
runs it with `--no-optional-locks` and `-c core.fsmonitor=false`,
`classify_worktrees` gives each listed entry a `WorktreeRefusal` or none, and
`resolve_worktree` matches a `worktree` argument by canonical path against the
reachable set. A refusal names the reason when the argument is a registered
worktree that is not reachable (own checkout, bare, prunable, missing, outside
the repository root and `allowed_roots`), and always lists the reachable
worktrees. `PathPolicy::is_inside_allowed_root` added (false with an empty
list). **Deviation:** C1 — the excluded entry is the session's own checkout,
not git's main worktree; proposal §5.1 updated. A non-repository folder makes
`GitService::worktrees` return `ClientError::Git` rather than an empty list;
Task 4 decides what `list_worktrees` shows for it. **Discovery for Task 5:**
git marks a worktree prunable as soon as its `.git` file is gone, with the
directory still present, which is how the prunable test builds one. Tests:
`tests/worktree_reads.rs`, 14 tests, plus 5 parser unit tests in the module
(`test(worktree_access)`); all pass. **Falsified:** not excluding the own
checkout failed 6 tests; skipping the prunable check failed
`a_prunable_worktree_is_refused_with_the_reason`; comparing the argument
without canonicalizing failed 8, including the symlink alias and trailing-slash
tests; letting an empty `allowed_roots` allow everything failed
`an_outside_worktree_is_refused_with_empty_allowed_roots` and the linked-worktree
test. Each was reverted. No `chat.rs` or `mode.rs` change in this task.
**Checks (local):** fmt, node, lint and typos pass; `cargo deny check` passes
(outside the agent sandbox, which cannot take its advisory-db lock); scoped
clippy for `workspace-engine` is clean; nextest over `worktree_reads`,
`worktree`, `agent_tools` and the `worktree_access`/`path_policy`/`git_service`
unit tests: 80 passed. The full seven-check gate passed in CI on 2026-10-17.

**Task 2 (Done 2026-10-17).** `git_service.rs`: every git process starts from
one runner, `read_git`: `--no-optional-locks`, `-c core.fsmonitor=false`, `-c
diff.autoRefreshIndex=false`, stdin null (`status`, `diff`, `summary`,
`worktrees`); `diff` adds `--no-ext-diff --no-textconv`. `diff` lists the
changed paths with `--name-status -z --no-renames` and drops restricted ones.
When nothing was dropped, the content diff runs without pathspecs; when
something was, over `:(top,literal)` pathspecs; when everything was, not at
all. `status` drops porcelain entries naming a restricted path on either side
of a rename, after undoing git's C-style path quoting. A path is restricted
when it or any parent directory is (`PathPolicy::is_restricted`), so a file
below `config/private` is dropped, as `list_directory` would never reach it.
Diff output is redacted, then cut to `max_read_lines`/`max_file_bytes` with a
last line `[Diff truncated by lines|bytes: N of M lines shown; F of T files not
shown.]`. `GitService::new` takes the `PathPolicy` and a new `GitReadCaps`
(`from_config`); `workspace_engine.rs` and the module's test helper follow. The
CLI and `tests/foundation.rs` needed no edit: `diff`'s signature is unchanged
until Task 3.

**Deviations.** (1) C3 was incomplete. `git diff` rewrites the index under
`index.lock` whenever it finds stat-only changes (`refresh_index_quietly`),
whatever `--no-optional-locks` says; only `diff.autoRefreshIndex=false` stops
it. The patch output is unchanged by it. The `--name-status` listing can then
name a stat-only file, which only widens the pathspec. Proposal §5.4 updated.
(2) `status` is capped too, by the same two caps, since requirement 8 covers
every git read: `GitStatus` gained `omitted`, and `format_git_status` (the only
`chat.rs` edit) ends a cut listing with `[Status truncated: N of M entries
shown.]`. (3) A status whose only changes are restricted is `clean`, so the
model reads "Working tree clean." and "No differences." Nothing hints at the
restricted change, as `list_directory` does not hint at a dropped path.

**Main-checkout output changes**, all intended: restricted entries are gone
from `read_git_status`, `read_git_diff`, `damaian git-status`'s `fileCount`,
`damaian git-diff`, and the desktop shell's `/api/git-status` file list (the
post-patch "changed" message). Diffs over 400 lines or 1 MiB, and statuses over
400 entries, are cut with the footers above. A diff under a user-configured
external or textconv driver shows git's own diff instead. Reads no longer
refresh `.git/index`'s stat cache. The repo bar's line counts (`summary`) still
include restricted files: they are numbers, not paths. Audit records are
unchanged.

**Discovery, not fixed.** `.gitattributes` can also select a `filter=<name>`
whose `clean` command the user's config defines, and `status`/`diff` run it on
stat-dirty files. No git flag disables filters wholesale, and disabling them
would make git-lfs or git-crypt files read as modified. Left for a later
decision; recorded in proposal §7.

**Tests.** `tests/worktree_reads.rs` grows by 11 (25 in all), plus
`porcelain_paths_are_unquoted_before_they_are_judged` in the module. Step 2:
the tests first failed to compile (no `GitReadCaps`, no `GitStatus::omitted`).
The first implementation then failed `status_does_not_take_the_index_lock`,
which is how deviation (1) was found.

**Falsified**, each reverted. No status filter failed
`a_restricted_tracked_file_is_absent_from_status_and_diff`. No diff filter
failed 4 tests, the rename test among them. No parent-directory check failed
`a_file_inside_a_restricted_directory_is_absent_from_status_and_diff`; it first
survived with a `private/` pattern, which matches descendants on its own, so
the test uses `config/private`. No unquoting failed the restricted test.
Judging only a rename's new name failed the rename test; it first survived
because the test only renamed onto a restricted path, and now renames off one
too. No diff cap and no status cap failed their truncation tests. No
`--no-ext-diff`, no `--no-textconv` and no `core.fsmonitor=false` each failed
its driver test, and each driver test also proves its driver runs under plain
git. No `--no-optional-locks` and no `diff.autoRefreshIndex=false` each failed
`status_does_not_take_the_index_lock`. A `--no-renames` content diff failed
`a_diff_with_no_changes_is_unchanged`.

**Checks (local):** fmt, node, lint and typos pass; `cargo deny check` passes
(outside the agent sandbox, which cannot take its advisory-db lock); scoped
clippy for `workspace-engine`, `damaian-cli` and `desktop-shell` is clean;
nextest over `worktree_reads`, `foundation`, `agent_tools` and the
`git_service`/`worktree_access` unit tests: 233 run, 232 passed inside the
sandbox, and the one failure,
`index_cache_picks_up_file_changes_via_watcher_without_full_rescan`, needs
FSEvents and passes outside it. Workspace clippy and tests are left to CI.

**Task 3 (Done 2026-10-18).** `chat.rs`: `ReadFile`, `ListDirectory` and
`SearchContent` gained `worktree`; `ReadGitStatus { worktree }`; `ReadGitDiff`
gained `worktree`, `paths`, `stat` and `base`. `dispatch_read_only_action`
resolves `worktree` once with `resolve_worktree` and hands the worktree's
canonical root to the existing functions. A refusal is `ActionOutcome::Failed`
with the resolver's message, which lists the reachable worktrees. To record the
worktree, each function gained an `_in` sibling taking `worktree: Option<&str>`,
the display path: `FileAccessController::read_file_in`,
`NavigationController::list_directory_in` / `search_content_in`, and
`GitService::status_in` / `diff_in(root, worktree, &DiffOptions)`. The old
functions pass `None`, so the CLI, the desktop shell, context assembly and every
existing test call them unchanged. Every result, summary, label and audit value
that names the worktree is added only when `worktree` was given. `mode.rs`:
`ReadGitStatus` became `ReadGitStatus { .. }` in both matches, and the test
list's constructors gained the new fields. Nothing else there changed.

**Deviations.** (1) All five audit records carry `worktree`, empty for the
session's checkout, not only `git_status_read` and `git_diff_read`
(requirement 10). Proposal §5.5 updated. (2) A worktree read's `FileRead`
evidence path is `<worktree>:<path>`, the same form as the marker's `ref`, so a
step's evidence cannot take a worktree read for a read of the same path in the
session's checkout. (3) `read_git_diff`'s marker `ref` adds ` base=…`, ` stat`
and ` paths=…` when they are given, so two differently scoped diffs are two
references; a plain diff's `ref` is unchanged. (4) The descriptions say that
`worktree` takes a registered worktree's path and that a refusal lists the
reachable ones. They do not name `list_worktrees` yet, since offering a tool
that does not exist would mislead the model. Task 4 adds it to the
descriptions. (5) Base validation is stricter than `check-ref-format` in two
cheap ways: no leading `/` and no `@`. (6) The merge base is resolved with its
own `git merge-base --end-of-options <base> HEAD`, and the diff runs against the
SHA, so the listing and the content diff compare against the same commit.

**Discovery.** `PathPolicy::resolve_for_write` is the resolver for `paths`,
because a deleted file must still be nameable. Its refusal says "Write target",
so `diff_scope` replaces the message with one about the checkout being read.
`--stat` output has no `diff --git` lines, so a cut stat footer reports `0 of 0
files not shown`; a stat over 400 lines is rare enough to leave.

**Tests.** `tests/worktree_reads.rs` grows by 14 (39 in all): each of the five
tools through `MockModelAdapter` with and without `worktree`; a refused
worktree failing all five with no approval stop; a tracked restricted `.env`
changed in a worktree; the audit `worktree` field on all five records; and
`diff_in` with `base`, `paths` and `stat`, bad bases, escaping paths, and the
three options through a tool call. Step 2: the tests first failed to compile
(no `DiffOptions`, no `diff_in`).

**Falsified**, each reverted. Dispatching on the session's root instead of the
worktree's failed 7 tests. Falling back to the session's root on a refusal
failed the refusal test. Dropping the marker prefix failed the `read_file` and
`list_directory` tests. Skipping `validate_base` failed the bad-base test.
Resolving `paths` without `PathPolicy` against the worktree failed 3 tests,
the escape test among them. Recording an empty `worktree` in `git_diff_read`
failed the audit test.

**Checks (local):** fmt, node, lint and typos pass. `cargo deny check` fails
only on the agent sandbox's read-only advisory-db lock. Scoped clippy for
`workspace-engine` is clean. `cargo nextest run -p workspace-engine`: 1272
run, 1271 passed; the one failure,
`index_cache_picks_up_file_changes_via_watcher_without_full_rescan`, needs
FSEvents and the sandbox has none. Step 6: the deterministic eval tier passes
every scenario. The CLI, the desktop shell and context assembly call the
unchanged functions, so this task changes no other crate. Workspace clippy
and tests are left to CI.

**Task 4 (Done 2026-10-18).** `git_service.rs`: `WorktreeListing` /
`WorktreeListingEntry` and `GitService::list_worktrees` / `log_in`, with
helpers `main_worktree_branch`, `ahead_behind`, `changed_file_count` and
`cap_log`. `list_worktrees` classifies the reachable set once, then reports
each entry's display path, branch (or none, shown as `detached`), short `HEAD`,
changed-file count (restricted paths not counted) and ahead/behind against
`base` (the model's override, validated, else the main worktree's branch);
capped at 20 with the rest counted, and audited `git_worktrees_listed`.
`log_in` runs `git log --oneline --no-ext-diff --no-textconv` over `base..HEAD`
(`HEAD`'s history when there is no base), `base` validated and passed after
`--end-of-options`, capped by `max_count` and the configured line/byte caps,
redacted, audited `git_log_read` with `worktree`. `chat.rs`: the two variants,
`ActionEffect::ReadOnly`, `read_tool_worktree` (log only — `list_worktrees` has
no `worktree`), `tool_action_marker_unscoped`, `tool_action_label`, the two
definitions, `tool_action_from_call` arms, the offering list, the dispatch arms
and `format_worktree_listing`. `mode.rs`: both variants in the two read-only
arms plus the matrix test list (allowed in every mode and every profile).

**Deviations.** (1) `list_worktrees` spawns `1 + 3n` git processes, not the
proposal's `1 + 2n`: `rev-parse --short HEAD` is added per worktree because
`entry.head` is the full SHA and the tool reports the short one. Proposal §5.2
updated. (2) `read_git_log` without `base` reads `main..HEAD` (the main
worktree's branch) and, when the main worktree is detached, the whole `HEAD`
history — the "default absent" case from §5.2, left under-specified there and
chosen here. (3) `read_git_log`'s audit carries `worktree`, empty for the
session's checkout, matching Task 3 deviation (1), though §5.5 names only
`git_status_read` and `git_diff_read`. (4) The line cap is applied at `-n` in
git (a silent bound, like `max_matches`); the byte cap is what `cap_log` cuts
and reports.

**Discovery, not fixed.** `git rev-list --left-right --count base...HEAD`
prints the left count (behind) then the right count (ahead), and the two are
tab-separated — easy to swap when naming the return `(ahead, behind)`; the
first implementation did, and the ahead/behind test caught it.

**Tests.** `tests/worktree_reads.rs` grows by 8 (47 in all): the listing
fields, detached omission, restricted count, the 20 cap, log newest-first /
`max_count` / configured cap, and a dispatch test proving both batch in one
round, need no approval, and audit. Step 2: the tests first failed to compile
(no `WorktreeListing`, no `log_in`). Falsified, each reverted: dropping the
restricted filter from `changed_file_count` counted the `.env` (2 instead of 1);
ignoring `max_count` returned 4 lines instead of 2; swapping ahead/behind made
the listing report `0 ahead, 1 behind` for a worktree one commit ahead.

**Checks (local):** fmt, node, lint, typos and deny pass; scoped clippy for
`workspace-engine` is clean; `cargo nextest run -p workspace-engine --locked`:
1297 run, 1297 passed (24 skipped). Workspace clippy and tests are left to CI.

## Global Constraints

Every task's requirements implicitly include this section.

- **Read [`proposal.md`](proposal.md) §2, §3 and §5 and the corrections below
  before starting.** Where a correction contradicts the proposal's prose, the
  correction wins, and the task that acts on it updates the proposal in the
  same change (most were already folded in while planning).
- **No command-policy change** (requirement 9). Nothing in this spec edits
  `command_policy.rs`, `command_allowlist`, `is_low_risk_read_only` or the
  shell-control gate. Spec 64 is changing `command_policy.rs` on another
  branch; if a task finds itself wanting to touch it, stop and say so.
- **One resolution path** (§5.3). A worktree argument becomes a root once,
  through `worktree_access::resolve_worktree`, and the existing
  `FileAccessController`, `NavigationController` and `GitService` functions
  receive that root. No parallel read, walk, restricted or redaction code.
- **The session's policy, never the worktree's** (requirement 7, #34). No code
  path loads `.damaian/config.toml` from a worktree.
- **No worktree is trusted by location.** Only an entry git lists, compared by
  canonical path. Never accept a directory because it is under
  `.claude/worktrees/` or `.damaian/worktrees/`.
- **Without `worktree`, output is byte-identical** (requirement 2), except
  where Task 2's hardening changes the main checkout's git reads on purpose.
  Each such change is listed in that task's Progress row.
- **`mode.rs` stays a minimal addition.** Spec 64's uncommitted branch
  (`claude/modest-swanson-faf4df`) edits `mode.rs` too. Add match arms and
  widen patterns (`ReadGitStatus` → `ReadGitStatus { .. }`); do not reflow,
  reorder or rename anything there.
- **`chat.rs` is shared.** Spec 63 edits it around `record_command_findings`
  and the `run_command` description. Keep this spec's edits to the tool
  definitions, `parse_tool_call`, the read-only dispatch arms,
  `action_effect`, `tool_action_marker` and the progress labels.
- **Tests:** fixture repositories are fresh `git init` directories under the
  temp dir with `user.name`, `user.email` and `commit.gpgsign=false` set
  locally, as `tests/worktree.rs` does. Engines built in tests set
  `enable_index_watcher: false` and their own data directory. Model-dependent
  paths use `MockModelAdapter` or `DAMAIAN_MOCK_MODEL_RESPONSE`.
- **Falsify every load-bearing test.** Break what it guards, confirm it fails,
  revert, and record that in the Progress row.
- **A private Cargo target directory per worktree.** Prefix every `cargo`
  command with `CARGO_TARGET_DIR="$PWD/target"` from the worktree root.
- **Scoped checks per task; the full gate runs on CI only** (AGENTS.md, since
  2026-10-15). Each task runs `scripts/quality-gate.sh fmt node lint typos deny`,
  `cargo clippy -p <crate> --all-targets --locked -- -D warnings` for every
  crate it touches, and the test files it names.
- **Never `git commit` or `git push` unasked.** Each task ends by showing the
  change and the check results. One-line commit subject, no body.

## Corrections to the proposal found while planning

Each one names the task that owns it.

| # | Proposal said | Code says | Resolution |
|---|---|---|---|
| C1 | A reachable worktree "is not the main worktree (that is the session's own checkout)" (§5.1) | A spec 57 worker session's repository root **is** a linked worktree (`.damaian/worktrees/<slug>`). From there git still lists the main worktree first, and the session's own checkout is a later entry | Exclude the entry whose canonical path is the session's canonical root, whichever it is. The main worktree is then an ordinary entry, reachable only by the location rule (for a worker, outside its root, so it needs `allowed_roots`). A `bare` entry has no working tree and is excluded. Proposal §5.1 updated. **Task 1** |
| C2 | Restricted paths are excluded with `:(exclude,glob)<pattern>` pathspecs (§5.4) | `restricted_patterns` are gitignore-style and matched by `PathPolicy::is_restricted` (`ignore.rs`): `.env` matches at any depth, a trailing `/` means a directory, and `!` negates. A glob pathspec has none of those semantics, so the translation would drift from what `read_file` refuses | Filter with `PathPolicy::is_restricted` itself, the same matcher `list_directory` uses: list the changed paths first (`diff --name-status -z --no-renames`, so both sides of a rename are checked), drop restricted ones, and run the content diff over the remaining paths as `:(top,literal)` pathspecs after `--`. Proposal §5.4 updated. **Task 2** |
| C3 | Every invocation passes `-c core.fsmonitor=false` (§5.4) | `git status` refreshes the index and takes `index.lock` opportunistically. In a worktree where another agent is running git, a Damaian read could make that agent's `git add` fail. `checkpoint.rs::run_user_git` already passes `--no-optional-locks` for this reason. Task 2 found `git diff` does the same whatever `--no-optional-locks` says | Every read also passes `--no-optional-locks` and `-c diff.autoRefreshIndex=false`. Proposal §5.4 updated. **Tasks 1, 2** |
| C4 | `GitService::status`/`diff` take a root and apply restricted patterns and caps (§5.3, §5.4) | `GitService::new(audit_log, scanner)` (`git_service.rs:174`) has no `Config` or `PathPolicy`. It is built once in `workspace_engine.rs:96`, and `damaian-cli/src/main.rs:173,184` and `tests/foundation.rs:1381` call `status`/`diff` | `GitService::new` gains the `PathPolicy` and the caps (`max_read_lines`, `max_file_bytes`). The CLI's `damaian git status`/`diff` get the same hardening, which is intended: they are the same read. **Task 2** |
| C5 | `GitStatus` entries for restricted paths are dropped (§5.4) | `GitStatus.raw` carries git's raw porcelain beside `files`, and `format_git_status` (`chat.rs`) is what the model sees | Filter `raw` with the same rule, so no restricted path survives in either field. **Task 2** |
| C6 | "The mode matrix allows them everywhere" (§5.6) | `mode.rs` has two exhaustive matches over `ToolAction`: `profile_permits` (line 168) and `mode_permits` (line 244), plus the test list at line 472. `ToolAction::ReadGitStatus` is a unit variant, so adding `worktree` changes its pattern in all three places and in `chat.rs` | Add `ListWorktrees` and `ReadGitLog` to both matches' read-only arms; widen `ReadGitStatus` to `ReadGitStatus { .. }`. Nothing else in `mode.rs` changes (Global Constraints). Proposal §5.6 updated. **Tasks 3, 4** |

## File Structure

| File | Change |
|---|---|
| `crates/workspace-engine/src/worktree_access.rs` | **New (Task 1).** Porcelain parser, reachability classification, argument resolution |
| `crates/workspace-engine/src/git_service.rs` | `worktrees` (Task 1); hardened `status`/`diff`, new constructor (Task 2); `diff` options, `log`, `ahead_behind` (Tasks 3, 4) |
| `crates/workspace-engine/src/path_policy.rs` | `is_inside_allowed_root` (Task 1) |
| `crates/workspace-engine/src/workspace_engine.rs` | `GitService::new` call (Task 2) |
| `crates/damaian-cli/src/main.rs` | Follows the new `diff` signature (Task 2) |
| `crates/workspace-engine/src/chat.rs` | Tool definitions, `parse_tool_call`, `ToolAction` fields and variants, read-only dispatch, `action_effect`, `tool_action_marker`, progress labels (Tasks 3, 4) |
| `crates/workspace-engine/src/mode.rs` | Two arms and one widened pattern (Tasks 3, 4) |
| `crates/workspace-engine/tests/worktree_reads.rs` | **New (Task 1).** Grows with every task |
| `crates/eval-harness/scenarios/worktree_survey.toml`, fixture | **New (Task 5)** |
| `docs/USER_GUIDE.md`, `docs/TROUBLESHOOTING.md`, `SECURITY.md`, `docs/specs/52_mcp_server_mode.md`, `docs/specs/64_read_only_command_boundary.md` | Task 6 |

## Interfaces

Task 1 lands these; later tasks consume them.

```rust
// worktree_access.rs
pub struct WorktreeEntry {          // one `git worktree list --porcelain -z` record
    pub path: PathBuf,              // as git listed it
    pub head: Option<String>,
    pub branch: Option<String>,     // `refs/heads/` stripped
    pub bare: bool,
    pub detached: bool,
    pub locked: bool,
    pub prunable: bool,
}
pub enum WorktreeRefusal { OwnCheckout, Bare, Prunable, Missing, OutsideAllowedRoots }
pub struct ClassifiedWorktree {
    pub entry: WorktreeEntry,
    pub canonical: Option<PathBuf>,
    pub display: String,            // repository-relative inside the root, else absolute
    pub refusal: Option<WorktreeRefusal>,
}
pub fn parse_worktree_list(raw: &[u8]) -> Vec<WorktreeEntry>;
pub fn classify_worktrees(policy: &PathPolicy, repository_root: &Path,
                          entries: Vec<WorktreeEntry>) -> Result<Vec<ClassifiedWorktree>>;
pub fn resolve_worktree(git: &GitService, policy: &PathPolicy, repository_root: &Path,
                        requested: &str) -> Result<ClassifiedWorktree>;

// git_service.rs
impl GitService { pub fn worktrees(&self, root: &Path) -> Result<Vec<WorktreeEntry>>; }
// path_policy.rs
impl PathPolicy { pub fn is_inside_allowed_root(&self, canonical: &Path) -> bool; }
```

`resolve_worktree` refuses with `ClientError::AccessDenied`. It does not audit:
the tool that called it records the read (§5.5), and Task 4's `list_worktrees`
records `git_worktrees_listed`.

## Order and parallelism

```
1 ──> 2 ──> 3 ──> 4 ──> 5 ──> 6
```

Sequential. Tasks 2–4 all edit `git_service.rs`, and 3–4 both edit `chat.rs`
and `mode.rs`. Task 1 touches no shared file except `git_service.rs`,
`path_policy.rs` and `lib.rs`, so it could run beside spec 63 or 64. Tasks 3
and 4 must not run beside #59, #61, #62 or another spec's `chat.rs` task
(`docs/specs/README.md`, "Parallel work").

---

## Task 1: Registered-worktree resolution and refusal

**Requirements:** 5, 6. **Proposal:** §5.1. **Corrections:** C1, C3.
**Files:** `worktree_access.rs` (new), `git_service.rs`, `path_policy.rs`,
`lib.rs`, `tests/worktree_reads.rs` (new).

- [x] **Step 1: Write the failing tests** in `tests/worktree_reads.rs`, each on
      a fresh fixture with two linked worktrees under `.claude/worktrees/`:
  - `registered_worktrees_inside_the_root_are_reachable` (both, by display
    path, and the main checkout is not in the reachable set);
  - `a_worktree_argument_resolves_relative_or_absolute_to_the_same_root`;
  - `a_symlink_under_claude_worktrees_pointing_outside_is_refused`;
  - `a_symlink_to_a_registered_worktree_resolves_to_that_worktree` (canonical
    comparison is the rule, not "no symlinks");
  - `a_plain_directory_that_is_not_registered_is_refused`;
  - `a_refusal_lists_the_reachable_worktrees`;
  - `a_prunable_worktree_is_refused_with_the_reason`;
  - `the_sessions_own_checkout_is_not_a_worktree_argument`;
  - `an_outside_worktree_is_refused_with_empty_allowed_roots`;
  - `an_outside_worktree_is_reachable_inside_an_allowed_root`;
  - `from_a_linked_worktree_the_main_checkout_is_listed_but_outside` (C1);
  - `a_nonexistent_argument_is_refused_with_the_list`;
  - `an_allowed_roots_entry_that_holds_only_the_repository_keeps_outside_worktrees_out`;
  - `a_folder_that_is_not_a_repository_resolves_no_worktree`.
  And parser unit tests in the module: main plus linked entries, `detached`,
  `locked <reason>`, `prunable <reason>`, `bare`, and a path containing a
  newline.
- [x] **Step 2: Run them and confirm they fail** (the module does not exist,
      so they fail to compile).
- [x] **Step 3: `parse_worktree_list` and `GitService::worktrees`.** `-z`
      output; git run with `--no-optional-locks -c core.fsmonitor=false`.
- [x] **Step 4: `PathPolicy::is_inside_allowed_root`, `classify_worktrees`,
      `resolve_worktree`.** Reasons checked in this order: bare, prunable,
      missing (does not canonicalize, or not a directory), own checkout,
      outside (neither inside the canonical repository root nor inside a
      canonical `allowed_roots` entry, or `PathPolicy::canonical_root` refuses
      it). Prunable before missing, so a deleted worktree reports the reason git
      gives.
- [x] **Step 5: Falsify.** Accept the first entry instead of excluding the own
      checkout; skip the prunable check; compare the requested path without
      canonicalizing; treat an empty `allowed_roots` as allowing everything.
      Confirm a named test fails for each, and revert.
- [x] **Step 6: Update the proposal** for C1 and C3.
- [x] **Step 7: Scoped checks.** `cargo nextest run -p workspace-engine --test
      worktree_reads`, `cargo nextest run -p workspace-engine -E
      'test(worktree_access) + test(path_policy)'`, scoped clippy for
      `workspace-engine`, and the fast checks.

## Task 2: Git read hardening on the main checkout

**Requirements:** 8 (and its acceptance rows). **Proposal:** §5.4.
**Corrections:** C2, C3, C4, C5. **Files:** `git_service.rs`,
`workspace_engine.rs`, `damaian-cli/src/main.rs`, `tests/foundation.rs` (if its
call changes), `tests/worktree_reads.rs`, `chat.rs` only if
`format_git_status` must change.

- [x] **Step 1: Write the failing tests** (all on the main checkout):
  - `a_restricted_tracked_file_is_absent_from_status_and_diff` (both
    `files` and `raw`; both unstaged and `staged`);
  - `a_rename_onto_a_restricted_path_is_excluded`;
  - `a_large_diff_is_truncated_and_says_what_it_cut`;
  - `a_gitattributes_diff_driver_is_not_invoked` (local `.git/config` defines
    `diff.evil.command` writing a marker file; `.gitattributes` selects it;
    the marker must not appear) and the same for `textconv`;
  - `fsmonitor_is_not_run` (local `core.fsmonitor` pointing at a script that
    writes a marker);
  - `status_does_not_take_the_index_lock` (an existing `index.lock` does not
    make `status` fail);
  - `a_diff_with_no_changes_is_unchanged` (byte-identical to today's output
    for an ordinary diff with nothing restricted).
  Added while implementing:
  `a_file_inside_a_restricted_directory_is_absent_from_status_and_diff`,
  `a_status_with_only_restricted_changes_is_clean` and
  `a_large_status_is_truncated_and_says_what_it_cut`.
- [x] **Step 2: Run them and confirm they fail.**
- [x] **Step 3: One hardened git runner** in `git_service.rs`: `--no-optional-locks`,
      `-c core.fsmonitor=false`, `-c diff.autoRefreshIndex=false`, stdin null; `diff` and `log` add
      `--no-ext-diff --no-textconv`. `summary` uses it too.
- [x] **Step 4: Restricted exclusion (C2, C5)** with `PathPolicy::is_restricted`.
- [x] **Step 5: Caps.** `max_read_lines` and `max_file_bytes`, whichever binds
      first, with a trailing line naming what was cut (lines and files
      omitted), the shape #47 §5.5 uses.
- [x] **Step 6: Constructor (C4).** `GitService::new` takes the `PathPolicy`
      and caps; update `workspace_engine.rs`, the module's test helper and the
      CLI.
- [x] **Step 7: Falsify** each test (drop the exclusion, the cap, each flag)
      and revert.
- [x] **Step 8: Scoped checks**, including `--test foundation` and
      `--test agent_tools`, and clippy for `workspace-engine` and `damaian-cli`.
      Record in the Progress row every way main-checkout output changed.

## Task 3: The `worktree` argument on the five read tools

**Requirements:** 2, 7, 10. **Proposal:** §5.2, §5.3, §5.5.
**Corrections:** C6. **Files:** `chat.rs`, `mode.rs`, `git_service.rs`,
`tests/worktree_reads.rs`, `tests/agent_tools.rs`.

- [x] **Step 1: Write the failing tests:** for each of `read_file`,
      `list_directory`, `search_content`, `read_git_status` and `read_git_diff`,
      a dispatch through `MockModelAdapter` with `worktree` reads that
      worktree, and a refused `worktree` is `ActionOutcome::Failed` naming the
      reachable worktrees with no approval stop; a restricted file inside a
      worktree is refused by `read_file` and absent from the others; the audit
      records carry `worktree`; the action marker's `ref` is
      `<worktree>:<path>`; `read_git_diff` with `paths`, `stat` and `base`
      (merge base) on a worktree; `base` of `--output=/tmp/x` and `main..; rm`
      refused before git runs; a `paths` entry escaping the worktree refused.
- [x] **Step 2: Run them and confirm they fail.**
- [x] **Step 3: `ToolAction` fields.** `worktree: Option<String>` on
      `ReadFile`, `ListDirectory`, `SearchContent`; `ReadGitStatus` becomes
      `ReadGitStatus { worktree }`; `ReadGitDiff` gains `worktree`, `paths`,
      `stat`, `base`. Widen the `mode.rs` patterns (C6) and nothing else there.
- [x] **Step 4: `parse_tool_call` and tool definitions.** Descriptions say
      that worktrees exist, that `list_worktrees` finds them, and that
      `worktree` takes the path it returns (§5.2, last paragraph).
- [x] **Step 5: Dispatch.** Resolve once with `resolve_worktree`, then call the
      existing function with the worktree's canonical root. `base` validation:
      `git check-ref-format --allow-onelevel`-equivalent rules in Rust, no
      leading `-`, passed after `--end-of-options`.
- [x] **Step 6: Byte-identical check.** The deterministic eval tier
      (`cargo run -p eval-harness -- run --tier deterministic`) passes
      unchanged.
- [x] **Step 7: Falsify** and revert.
- [x] **Step 8: Scoped checks.**

## Task 4: `list_worktrees` and `read_git_log`

**Requirements:** 1, 3, 10. **Proposal:** §5.2, §5.5, §5.6.
**Corrections:** C6. **Files:** `chat.rs`, `mode.rs`, `git_service.rs`,
`tests/worktree_reads.rs`.

- [x] **Step 1: Write the failing tests:** `list_worktrees` returns path,
      branch or `detached`, short `HEAD`, changed-file count (restricted paths
      not counted) and ahead/behind against the main worktree's branch;
      omitted ahead/behind when the main worktree is detached; capped at 20
      with the rest counted; `read_git_log` returns `short-sha subject` lines,
      newest first, capped by `max_count` and the configured cap; both are
      `ActionEffect::ReadOnly`, batch in one round, and are allowed in every
      `SessionMode` and every profile; audit `git_worktrees_listed` and
      `git_log_read`.
- [x] **Step 2: Run them and confirm they fail.**
- [x] **Step 3: Variants, `action_effect`, `tool_action_marker`, both `mode.rs`
      arms (C6), progress labels, definitions, parsing, dispatch.**
- [x] **Step 4: Falsify** and revert.
- [x] **Step 5: Scoped checks**, including `--test permission_profiles`.

## Task 5: The `worktree_survey` eval scenario

**Requirements:** acceptance §6. **Proposal:** §5.7. **Files:**
`crates/eval-harness/scenarios/worktree_survey.toml`, its fixture, and the
runner only if it cannot yet create linked worktrees or a symlink in a fixture.

- [ ] **Step 1: Scenario and fixture** per §5.7: two registered worktrees (one
      dirty with a restricted file changed, one with commits ahead of `main`)
      and a symlink under `.claude/worktrees/` pointing outside.
- [ ] **Step 2: Run it and confirm it fails** before any fixture or runner
      support it needs exists, then make it pass.
- [ ] **Step 3: Assertions:** zero approval stops; the symlink refused; the
      restricted file absent from the diff; the answer names both worktrees.
- [ ] **Step 4: Scoped checks** (`cargo nextest run -p eval-harness`).

## Task 6: Docs, live re-run and close-out

- [ ] **Step 1: Live re-run** of the motivating prompt against
      `deepseek-v4-flash` on this repository. Record approvals made and why in
      proposal §6/§7.
- [ ] **Step 2: Docs** (§5.8): `USER_GUIDE.md`, `TROUBLESHOOTING.md` (the
      refusal reasons from Task 1), `SECURITY.md` (§5.4).
- [ ] **Step 3: Cross-references.** #52 records whether its server exposes
      `worktree`; #64's requirement 5 dependency on #60 is updated.
- [ ] **Step 4: Close the spec** per AGENTS.md "When a spec becomes Done":
      status in the proposal, README row and this file; `Depends on:` search;
      What to build next; remove the item from `CHANGELOG.md` Unreleased.
- [ ] **Step 5: Checks.** Fast checks and scoped clippy/tests locally; the full
      gate on CI. Record the CI result here when it is green.
