# Agent Working Capability Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Implements:** [`proposal.md`](proposal.md) §5, requirements 1–4 · background and
corrections in [`context.md`](context.md)
**Started:** 2026-09-18 — **Done:** 2026-09-18

**Goal:** Give the agent a working floor — reads that return a range instead of
a whole file, listing and content search as tools, and edits whose payload is
the size of the change — without widening what the agent may do.

**Architecture:** The tree traversal in `indexer.rs` is lifted into
`tree_walk.rs` as a visitor-shaped free function so there is exactly one place
the `.gitignore` and symlink-escape rules live; `ProjectIndexer` becomes its
first caller and the two new navigation tools are two more visitors.
`file_access.rs` gains a line range and returns a bounded payload. `edit.rs`
splices a single anchored match into full file content and hands
`PatchEngine::create_patch` the same `ProposedChange` a whole-file proposal
produces, so nothing downstream changes. `chat.rs` gains four tool definitions
and their dispatch arms.

**Tech Stack:** Rust 2024 (workspace edition). One new direct dependency:
`regex`, already in `Cargo.lock` transitively via `syntect` and `tokenizers`,
MIT/Apache-2.0, both on `deny.toml`'s allow-list.

## Progress

| Task | State | Notes |
|---|---|---|
| 1 · Extract the walk | Done | `tree_walk.rs` with a `WalkEvent` visitor; `ProjectIndexer::walk` deleted and `index_repository` now drives it through `add_file`, which was already factored out so nothing had to move. 1 new test. **Plan correction:** the plan named the skip reason `not_a_regular_file`; the index has always spelled it `not_regular_file` and its tests assert on it. Guarded by the 11 existing index tests, all green. |
| 2 · Ranged reads | Done | `LineRange`, `ReadWindow`, and `FileRead.{line_range,total_lines,truncated_by}`; `max_read_lines` (400) added restrict-only with a new `restrict_only_limit` helper and a `parse_read_lines` that refuses 0. 4 new tests; byte cap mutation-tested (disabling it fails `a_few_enormous_lines_…`). **Plan deviation:** the plan's `range: Option<LineRange>` was wrong — `None` would have meant "capped" for `context_manager` too, silently shrinking what every task sees, and its own comment contradicted the code. Replaced with a three-variant `ReadWindow` so each call site states its intent: `Whole` (context assembly, keeps the original `max_file_bytes` refusal), `Default` (the tool, capped), `Range` (also capped, so a large range cannot step around the cap). Also needed `#[allow(clippy::too_many_arguments)]` with a reason. |
| 3 · `list_directory` | Done | `navigation.rs` with `NavigationController` and `DirectoryListing`; a `tree_walk` visitor, `max_list_entries` (200) restrict-only. 3 new tests (8 in file). Restricted paths are checked **per entry**, not just on the starting directory, because a path can name a secret (`deploy/prod-key.pem`). **Beyond the plan:** added `repository_config_may_lower_the_navigation_caps_but_not_raise_them` to `repository_config_trust.rs` — no restrict-only key had a trust test at all, so an unclassed key would have been a silent hole. The `.env` fixture tests the restriction rather than the ignore rules: `.env` is in `DEFAULT_RESTRICTED_PATTERNS` and deliberately not in `DEFAULT_IGNORE_PATTERNS`. |
| 4 · `search_content` | Done | `ContentMatch`/`ContentSearch` on `NavigationController`; `regex` promoted to a direct dependency (lockfile diff is **one line** — the edge only, no version change, no new packages); `max_search_matches` (50) and `max_match_line_chars` (500) restrict-only. 6 new tests, not the planned 5 — added `a_max_matches_argument_cannot_raise_the_configured_cap`, because asserting only that the argument is honoured would pass if it could also raise the cap. Redaction mutation-tested: replacing `redact(line)` with the raw line fails the secret test with the key visible. `cargo deny check` green. **Doc correction:** the proposal and context said `regex` arrives via `syntect` and `tokenizers`; `cargo tree -i regex` shows `tokenizers` only — `syntect` is built with `regex-fancy` and pulls `fancy-regex`. Both files corrected. |
| 5 · `edit_file` splice | Done | `RegionEdit` and `region_edits_to_changes` in `edit.rs`; anchor must match exactly once, zero and two matches both refuse, and the two-match refusal names the count. 6 new tests, not the planned 5 — added `two_edits_to_one_file_build_on_each_other`, because two edits to one file read from disk each time would silently drop the first. **Plan correction:** the 150 KB fixture was specified as `(1..=6000)` lines of `fn f{n}() {{}}\n`, which is only ~84 KB — the assertion would have passed a fixture that never exceeded the threshold. Built at 12 000 lines instead. Fail-closed rule mutation-tested: `if count > 1` → `> 2` fails the twice-match test. |
| 6 · Wire the four tools | Done | Four tool definitions and three new `ToolAction` variants (`ListDirectory`, `SearchContent`, `EditFile`); `ReadFile` grew a `range: Option<LineRange>`. `ChatOrchestrator` gained `navigation` and `path_policy` fields (the plan's file-structure table omitted both, but the dispatch arms need them). `read_file`'s schema now carries `start_line`/`end_line`, and its result states the range and any truncation. 2 new tests, matching the plan. The `edit_file` arm reuses the `ProposePatch` arm's `create_patch` body verbatim, so requirement 4's "indistinguishable downstream" holds by construction. |
| 7 · Acceptance criteria, docs, close the slice | Done | 3 new tests (25 in file): the region/whole-file patch equivalence, one truncation notice per read tool, and the audit-event test. **Plan correction:** the audit test as written asserted on `.damaian/audit.log`; the log is actually `data_dir/audit/events.jsonl` (`audit.rs:61`). Full suite: **678 passed, 16 skipped** (not the planned 675 — Task 4 and Task 5 each added one test beyond plan). **Missed call site:** `damaian-cli/src/main.rs:150` still called `read_file` with six arguments after Task 2's signature change and did not compile; fixed to `ReadWindow::Whole`. |

**Baseline measured 2026-09-17:** `cargo nextest run --workspace --locked` is
**652 passed, 16 skipped**, and takes about five minutes locally — see
`OBSERVATIONS.md` entry 8 for why, and the Global Constraints for what that means
for this plan.

The running check per task is the count in `tests/agent_tools.rs` — 1, 5, 8, 14,
20, 22, 25 as Tasks 1–7 land. (The plan originally wrote 1, 5, 8, 13, 18, 20,
23; Task 4 added six tests rather than five, and Task 5 did the same, so the
real count runs two ahead from Task 4 onward — recorded in those rows.) Task 7
verifies the whole workspace once, at **678**. A task whose file count comes out
wrong has a missing or duplicated test; find it then rather than at the end.

## Global Constraints

Every task's requirements implicitly include this section.

- **Read [`context.md`](context.md) before starting.** Four of the proposal's
  §2 statements were corrected there on 2026-09-17. Each task below already
  accounts for its correction; do not "fix" a task back to §2's wording.
- **This slice adds no autonomy.** Every tool here is read-only or produces a
  patch the user still approves. If a change would let the agent write, run, or
  approve something it could not before, it is out of scope — stop and raise it.
- **Do not touch `command_policy.rs`.** Requirement 3 is met by these being
  engine tools rather than shell commands. No `command_allowlist` entry, no
  change to `is_low_risk_read_only`, no relaxation of the shell-control gate.
- **Do not touch `patch_engine.rs`.** Requirement 4's promise is that a region
  edit is indistinguishable downstream. A change there means the splice is
  producing something a whole-file proposal would not.
- **Every capped result states what it cut.** A truncated result that reads as
  complete is the failure mode §5.5 exists to prevent. Never copy
  `truncate_output` (`command_runner.rs`), which cuts silently.
- **New config keys are `RepositoryKeyClass::RestrictOnly`.** A repository may
  lower a cap, never raise one. Spec 34's rule.
- **New tests use `test_config`** (`tests/foundation.rs:54`), which sets
  `enable_index_watcher: false`. A test that registers an FSEvents watcher costs
  seconds of waiting on `fseventsd` that no test here needs.
- **Clippy warnings are errors.** Fix rather than suppress; an `#[allow(...)]`
  needs a comment saying why.
- **Per task, run only the targeted tests; the full suite and the full gate run
  once, in Task 7.** A local `cargo nextest run --workspace --locked` takes about
  five minutes — it is 92% idle, blocked on `git` subprocesses in the checkpoint
  object store (`OBSERVATIONS.md` entry 8), not something a faster machine or a
  build cache fixes. Seven of them would be 35 minutes of waiting for a slice
  that touches none of the slow suites. Each task below names the narrow command
  to run instead.
- **The gate still gates.** Nothing is called finished until every quality-gate
  command from `AGENTS.md` has passed — that is seven commands, the list there is
  authoritative, and `typos` and `node --check crates/desktop-shell/static/app.js`
  are the two that get missed. Deferring them changes when they run, not whether.
  If a task's narrow run goes red, stop there rather than carrying it forward.
- **Commit messages:** one subject line, no body, no `Co-Authored-By`. Rationale
  belongs in this plan and the proposal. Never cite commit SHAs in
  documentation.
- **Do not commit without asking.** Show the change and the gate result; the
  decision to commit is Damijan's.

## File Structure

| File | Responsibility |
|---|---|
| `crates/workspace-engine/src/tree_walk.rs` | **New.** `walk()` — the traversal lifted from `ProjectIndexer::walk`: per-directory `.gitignore` accumulation, sorted entries, symlink-escape rejection. Visitor-shaped, no index knowledge. |
| `crates/workspace-engine/src/indexer.rs` | `ProjectIndexer::walk` deleted; `index_repository` calls `tree_walk::walk` with an index-building visitor. |
| `crates/workspace-engine/src/navigation.rs` | **New.** `NavigationController` with `list_directory` and `search_content`. Owns the caps and the truncation notices. |
| `crates/workspace-engine/src/file_access.rs` | `read_file` gains `range: Option<LineRange>`; returns `FileRead` with `line_range`, `total_lines`, `truncated_by`. |
| `crates/workspace-engine/src/edit.rs` | `RegionEdit` and `region_edits_to_changes` — anchor match, fail closed, splice to `ProposedChange`. |
| `crates/workspace-engine/src/config.rs` | Four `RestrictOnly` caps and the `restrict_only_limit` helper. |
| `crates/workspace-engine/src/chat.rs` | Four tool definitions, four decode arms, four dispatch arms, four `tool_action_marker` arms. |
| `crates/workspace-engine/src/lib.rs` | `pub mod tree_walk;`, `pub mod navigation;`, re-exports. |
| `crates/workspace-engine/tests/agent_tools.rs` | **New.** Every acceptance criterion in proposal §6's first-slice group. |

---

### Task 1: Extract the walk into `tree_walk.rs`

Pure refactor. `ProjectIndexer` must produce a byte-identical index afterwards —
the existing index tests are the guard, which is why this task adds only one new
test of its own.

**Files:**
- Create: `crates/workspace-engine/src/tree_walk.rs`
- Modify: `crates/workspace-engine/src/indexer.rs:152-330`, `crates/workspace-engine/src/lib.rs`
- Test: `crates/workspace-engine/tests/agent_tools.rs` (new file)

**Interfaces:**
- Produces: `tree_walk::walk(root: &Path, directory: &Path, relative_directory: &str, inherited_rules: &[IgnoreRule], visitor: &mut dyn FnMut(&WalkEvent) -> Result<()>) -> Result<()>`; `enum WalkEvent { File(WalkFile), Skipped(WalkSkip) }`; `WalkFile { relative_path: String, absolute_path: PathBuf }`; `WalkSkip { path: String, reason: String }`. Directories are recursed into, not reported — no caller in this slice needs them.

- [x] **Step 1: Write the failing test**

In a new `crates/workspace-engine/tests/agent_tools.rs`, with the same
`temp_dir` / `write_fixture` / `test_config` helpers `foundation.rs` uses
(copy them; the test crates do not share a helper module):

```rust
#[test]
fn the_walk_rejects_a_symlink_that_escapes_the_root() {
    let repo = temp_dir("walk-symlink");
    write_fixture(&repo, "src/lib.rs", "pub fn a() {}\n");
    let outside = temp_dir("walk-outside");
    write_fixture(&outside, "secret.txt", "s3cret\n");
    std::os::unix::fs::symlink(&outside, repo.join("src/escape")).unwrap();

    let mut seen = Vec::new();
    let mut skipped = Vec::new();
    workspace_engine::tree_walk::walk(&repo, &repo, "", &[], &mut |entry| {
        match entry {
            workspace_engine::tree_walk::WalkEvent::File(file) => {
                seen.push(file.relative_path.clone())
            }
            workspace_engine::tree_walk::WalkEvent::Skipped(skip) => {
                skipped.push((skip.path.clone(), skip.reason.clone()))
            }
        }
        Ok(())
    })
    .unwrap();

    assert!(seen.contains(&"src/lib.rs".to_string()));
    assert!(
        !seen.iter().any(|path| path.contains("escape")),
        "a symlink resolving outside the root must not be walked: {seen:?}"
    );
    assert_eq!(
        skipped,
        vec![("src/escape".to_string(), "symlink_outside_root".to_string())]
    );
}
```

- [x] **Step 2: Run it to verify it fails**

Run: `cargo nextest run -p workspace-engine --test agent_tools`
Expected: FAIL to compile — `tree_walk` does not exist.

- [x] **Step 3: Write `tree_walk.rs`**

Move the body of `ProjectIndexer::walk` (`indexer.rs:250-330`) verbatim, replacing
the two accumulators with one visitor. Do not change the order of the checks:
ignore first, symlink second, directory third. `entries.sort_by_key` stays — a
deterministic order is what makes `list_directory`'s output stable across runs.

```rust
use crate::error::Result;
use crate::ignore::{IgnoreRule, is_ignored_by_rules, parse_ignore_patterns};
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct WalkFile {
    pub relative_path: String,
    pub absolute_path: PathBuf,
}

#[derive(Debug, Clone)]
pub struct WalkSkip {
    pub path: String,
    pub reason: String,
}

/// What the walk found. A visitor sees every file and every skip, so a caller
/// that wants to report "12 files ignored" can, and one that does not can
/// ignore the variant.
#[derive(Debug, Clone)]
pub enum WalkEvent {
    File(WalkFile),
    Skipped(WalkSkip),
}

/// The one traversal in this crate.
///
/// Lifted out of `ProjectIndexer::walk` so the navigation tools cannot become a
/// second place the ignore rules and the symlink-escape check live. The escape
/// check is a security property, not a convenience: a symlink that canonicalizes
/// outside the root would otherwise let a walk read anything the user can.
pub fn walk(
    root: &Path,
    directory: &Path,
    relative_directory: &str,
    inherited_rules: &[IgnoreRule],
    visitor: &mut dyn FnMut(&WalkEvent) -> Result<()>,
) -> Result<()> {
    let mut rules = inherited_rules.to_vec();
    let gitignore_path = directory.join(".gitignore");
    if let Ok(content) = fs::read_to_string(gitignore_path) {
        let patterns = content.lines().map(|line| line.to_string()).collect::<Vec<_>>();
        rules.extend(parse_ignore_patterns(&patterns, relative_directory));
    }

    let mut entries = fs::read_dir(directory)?.collect::<std::result::Result<Vec<_>, _>>()?;
    entries.sort_by_key(|entry| entry.file_name());

    for entry in entries {
        let file_name = entry.file_name().to_string_lossy().to_string();
        let relative_path = if relative_directory.is_empty() {
            file_name
        } else {
            format!("{relative_directory}/{}", entry.file_name().to_string_lossy())
        };
        let file_type = entry.file_type()?;
        let is_directory = file_type.is_dir();

        if is_ignored_by_rules(&rules, &relative_path, is_directory) {
            visitor(&WalkEvent::Skipped(WalkSkip {
                path: relative_path,
                reason: "ignored".to_string(),
            }))?;
            continue;
        }

        if file_type.is_symlink() {
            let resolved = fs::canonicalize(entry.path())?;
            if !resolved.starts_with(root) {
                visitor(&WalkEvent::Skipped(WalkSkip {
                    path: relative_path,
                    reason: "symlink_outside_root".to_string(),
                }))?;
                continue;
            }
        }

        if is_directory {
            walk(root, &entry.path(), &relative_path, &rules, visitor)?;
            continue;
        }

        if !file_type.is_file() {
            // `not_regular_file`, exactly as `indexer.rs:321` spells it today.
            // The index's skip reasons are asserted on by existing tests and
            // are part of its output — this extraction must not reword them.
            visitor(&WalkEvent::Skipped(WalkSkip {
                path: relative_path,
                reason: "not_regular_file".to_string(),
            }))?;
            continue;
        }

        visitor(&WalkEvent::File(WalkFile {
            relative_path,
            absolute_path: entry.path(),
        }))?;
    }
    Ok(())
}
```

Add `pub mod tree_walk;` to `lib.rs` in alphabetical position (after
`pub mod session;`... check: it sorts after `secret_scanner`, before any later
module — place it to keep the list sorted).

- [x] **Step 4: Rewrite `ProjectIndexer::walk` as a caller**

The per-file work is already factored out as `ProjectIndexer::add_file`
(`indexer.rs:332`), so nothing has to move: the visitor just calls it. Delete
`ProjectIndexer::walk` (`:250-330`) entirely and replace the `self.walk(...)`
call in `index_repository` (`:166-174`) with:

```rust
let mut files = Vec::new();
let mut skipped = Vec::new();
crate::tree_walk::walk(&root, &root, "", &rules, &mut |event| match event {
    crate::tree_walk::WalkEvent::Skipped(skip) => {
        skipped.push(SkippedFile {
            path: skip.path.clone(),
            reason: skip.reason.clone(),
        });
        Ok(())
    }
    crate::tree_walk::WalkEvent::File(file) => self.add_file(
        &repository_id,
        &file.absolute_path,
        &file.relative_path,
        &mut files,
        &mut skipped,
    ),
})?;
```

`add_file` keeps its `max_file_bytes` and binary checks unchanged — the index
still skips a file it cannot usefully index, which is a different question from
whether a *read tool* may return part of one (Task 2).

If the borrow checker objects to `files` and `skipped` being captured by the
closure while `self.add_file` also takes them, pass them in as the closure's
captured `&mut` and call `add_file` on `self` — `add_file` takes `&self`, so
there is no conflict. Do not restructure `add_file` to work around it.

- [x] **Step 5: Run the index tests to verify nothing moved**

Run: `cargo nextest run -p workspace-engine -E 'test(index) or test(walk)'`
Expected: PASS, every index test plus the one written in Step 1.

These are the tests that pin what the walk produces, so they are the ones that
can detect a bad extraction. The full workspace run is deferred to Task 7.

This task adds exactly one test because the existing index tests are the real
assertion: they already pin what the walk produces, so an extraction that
changed anything fails them. If one of them goes red, the extraction is wrong —
do not edit the test to match.

If your count differs from 653, record the real number in the Progress table
rather than adjusting the plan silently. A count that drifts without explanation
is how a missing test hides.

- [x] **Step 6: Run clippy and fmt on what you touched**

`cargo fmt --all -- --check` and `cargo clippy --workspace --all-targets --locked -- -D warnings`.
These are fast and catch what the narrow test run cannot. The remaining five gate
commands run once in Task 7.

- [x] **Step 7: Show the change and the gate result, and ask before committing**

Suggested subject: `Lift the tree walk out of the indexer so one walker serves both`

---

### Task 2: Ranged reads

**Files:**
- Modify: `crates/workspace-engine/src/file_access.rs:10-18,43-51,63-67`, `crates/workspace-engine/src/config.rs`
- Test: `crates/workspace-engine/tests/agent_tools.rs`

**Interfaces:**
- Consumes: nothing from Task 1.
- Produces: `LineRange { start: usize, end: usize }` (1-based, inclusive); `FileAccessController::read_file(..., range: Option<LineRange>)`; `FileRead` gains `line_range: LineRange`, `total_lines: usize`, `truncated_by: Option<&'static str>` where the value is `"lines"` or `"bytes"`.

- [x] **Step 1: Write the failing tests**

```rust
#[test]
fn an_unranged_read_caps_lines_and_says_what_it_cut() {
    let repo = temp_dir("read-cap");
    let body = (1..=1000)
        .map(|n| format!("line {n}\n"))
        .collect::<String>();
    write_fixture(&repo, "big.rs", &body);
    let config = Config { max_read_lines: 400, ..test_config(&repo) };
    let scanner = SecretScanner::default();
    let access = FileAccessController::new(
        config.clone(),
        test_audit(&repo, scanner.clone()),
        scanner,
        PathPolicy::new(&config),
    );

    let read = access
        .read_file(&repo, "big.rs", None, None, false, false, None)
        .unwrap();

    assert_eq!(read.total_lines, 1000);
    assert_eq!(read.line_range, LineRange { start: 1, end: 400 });
    assert_eq!(read.truncated_by, Some("lines"));
    assert!(read.content.starts_with("line 1\n"));
    assert!(read.content.trim_end().ends_with("line 400"));
    assert!(!read.content.contains("line 401"));
}

#[test]
fn a_ranged_read_returns_only_that_range() {
    let repo = temp_dir("read-range");
    let body = (1..=1000).map(|n| format!("line {n}\n")).collect::<String>();
    write_fixture(&repo, "big.rs", &body);
    let config = test_config(&repo);
    let scanner = SecretScanner::default();
    let access = FileAccessController::new(
        config.clone(),
        test_audit(&repo, scanner.clone()),
        scanner,
        PathPolicy::new(&config),
    );

    let read = access
        .read_file(&repo, "big.rs", None, None, false, false,
                   Some(LineRange { start: 120, end: 180 }))
        .unwrap();

    assert_eq!(read.line_range, LineRange { start: 120, end: 180 });
    assert_eq!(read.total_lines, 1000);
    assert_eq!(read.truncated_by, None);
    assert!(read.content.starts_with("line 120\n"));
    assert!(read.content.trim_end().ends_with("line 180"));
}

/// Proposal §5.2: `max_file_bytes` caps what is *returned*, not what may be
/// *inspected*. Before this spec the same read was refused outright.
#[test]
fn a_file_over_the_byte_limit_is_readable_by_range() {
    let repo = temp_dir("read-oversize");
    let body = (1..=50_000).map(|n| format!("line {n}\n")).collect::<String>();
    write_fixture(&repo, "huge.rs", &body);
    let config = Config { max_file_bytes: 1024, ..test_config(&repo) };
    let scanner = SecretScanner::default();
    let access = FileAccessController::new(
        config.clone(),
        test_audit(&repo, scanner.clone()),
        scanner,
        PathPolicy::new(&config),
    );

    let read = access
        .read_file(&repo, "huge.rs", None, None, false, false,
                   Some(LineRange { start: 10, end: 12 }))
        .unwrap();

    assert_eq!(read.total_lines, 50_000);
    assert_eq!(read.content, "line 10\nline 11\nline 12\n");
}

/// The byte cap sits beside the line cap, not under it: 400 lines of a
/// generated file can exceed any budget a line count implies.
#[test]
fn a_few_enormous_lines_are_cut_by_bytes_and_say_so() {
    let repo = temp_dir("read-bytes");
    let body = (1..=10).map(|_| format!("{}\n", "x".repeat(10_000))).collect::<String>();
    write_fixture(&repo, "minified.js", &body);
    let config = Config { max_file_bytes: 5_000, max_read_lines: 400, ..test_config(&repo) };
    let scanner = SecretScanner::default();
    let access = FileAccessController::new(
        config.clone(),
        test_audit(&repo, scanner.clone()),
        scanner,
        PathPolicy::new(&config),
    );

    let read = access
        .read_file(&repo, "minified.js", None, None, false, false, None)
        .unwrap();

    assert_eq!(read.truncated_by, Some("bytes"));
    assert!(read.content.len() <= 5_000 + 1);
    assert_eq!(read.total_lines, 10);
}
```

- [x] **Step 2: Run to verify they fail**

Run: `cargo nextest run -p workspace-engine --test agent_tools`
Expected: FAIL to compile — `read_file` takes six arguments, not seven.

- [x] **Step 3: Add the config key**

In `config.rs`: add `pub max_read_lines: usize` to `Config` beside
`max_file_bytes` (`:145`), default `400` in the `Default` impl (`:1320`), the
overlay field, the `"max_read_lines"` parse arm, and the serialization row.

Add the restrict-only helper for a non-`Option` limit, beside
`restrict_only_ceiling` (`:2453`):

```rust
/// `restrict_only_ceiling` for a value that always has one. A repository may
/// lower a cap — that is a restriction, and restrictions are always allowed —
/// but raising one would let a cloned repository pull more of the user's files
/// into a model request than the user chose to allow.
fn restrict_only_limit(
    current: &mut usize,
    value: usize,
    key: &str,
    trusted: bool,
    rejected: &mut Vec<RejectedConfigKey>,
) {
    if trusted || value < *current {
        *current = value;
        return;
    }
    rejected.push(RejectedConfigKey::new(key, RepositoryKeyClass::RestrictOnly));
}
```

Call it from the overlay-apply block beside `agent_max_task_tokens` (`:735`).

- [x] **Step 4: Change `read_file`**

Add to `file_access.rs`:

```rust
/// A 1-based, inclusive line range, the representation spec 26 adopts for
/// `ContextItem` rather than defining a second one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LineRange {
    pub start: usize,
    pub end: usize,
}
```

`FileRead` gains `line_range: LineRange`, `total_lines: usize`,
`truncated_by: Option<&'static str>`.

`read_file` takes a seventh parameter `range: Option<LineRange>`. Replace the
`metadata.len() > self.config.max_file_bytes` refusal (`:63-67`) with the
bounded read:

1. Read the file (the binary check at `:70` stays and still applies).
2. Split into lines; `total_lines` is the count.
3. Pick the window: the requested range clamped to `1..=total_lines`, or
   `1..=max_read_lines` when none was given.
4. Join the window. While its byte length exceeds `max_file_bytes`, drop the
   last line and set `truncated_by = Some("bytes")`. If the window was shortened
   by the line cap instead, `truncated_by = Some("lines")`. A range the caller
   asked for and got in full is `None`.
5. Redact as today. The scanner runs on the returned window, not the whole file.

An explicitly requested range that is empty or inverted (`start > end`, or
`start > total_lines`) is `ClientError::InvalidInput` naming `total_lines`, so
the model can correct it in the same round rather than receiving an empty
string it might read as "the file is empty".

Update the four existing `read_file` call sites to pass `None`. Find them with
`grep -rn "\.read_file(" crates/`.

- [x] **Step 5: Run the new tests**

Run: `cargo nextest run -p workspace-engine --test agent_tools`
Expected: PASS, 5 tests in this file.

- [x] **Step 6: Mutation-test the byte cap**

Delete the byte-trim loop from Step 4 and re-run. `a_few_enormous_lines_are_cut_by_bytes_and_say_so`
must fail. Restore it and confirm it passes. A cap that no test can distinguish
from its absence is not a cap.

- [x] **Step 7: Targeted tests, fmt and clippy**

Run: `cargo nextest run -p workspace-engine --test agent_tools` — 5 tests, all
passing. Then `cargo fmt --all -- --check` and
`cargo clippy --workspace --all-targets --locked -- -D warnings`.

- [x] **Step 8: Show the change and the gate result, and ask before committing**

Suggested subject: `Read a file by line range and bound what a read returns`

---

### Task 3: `list_directory`

**Files:**
- Create: `crates/workspace-engine/src/navigation.rs`
- Modify: `crates/workspace-engine/src/lib.rs`, `crates/workspace-engine/src/config.rs`
- Test: `crates/workspace-engine/tests/agent_tools.rs`

**Interfaces:**
- Consumes: `tree_walk::walk` and `WalkEvent` from Task 1.
- Produces: `NavigationController::new(config, audit_log, scanner, path_policy)`; `list_directory(root, dir: Option<&str>, depth: Option<usize>, task_id, repository_id) -> Result<DirectoryListing>`; `DirectoryListing { paths: Vec<String>, total_found: usize, truncated: bool }`.

- [x] **Step 1: Write the failing tests**

```rust
#[test]
fn listing_respects_gitignore_and_caps_with_a_notice() {
    let repo = temp_dir("list-basic");
    for n in 1..=250 {
        write_fixture(&repo, &format!("src/m{n}.rs"), "pub fn a() {}\n");
    }
    write_fixture(&repo, "target/debug/junk", "binary\n");
    write_fixture(&repo, ".gitignore", "target/\n");
    let config = Config { max_list_entries: 200, ..test_config(&repo) };
    let nav = navigation_for(&repo, &config);

    let listing = nav.list_directory(&repo, None, None, None, None).unwrap();

    assert_eq!(listing.paths.len(), 200);
    assert_eq!(listing.total_found, 251);
    assert!(listing.truncated);
    assert!(
        !listing.paths.iter().any(|p| p.starts_with("target/")),
        "an ignored directory must not be listed"
    );
}

#[test]
fn listing_cannot_reach_a_restricted_path() {
    let repo = temp_dir("list-restricted");
    write_fixture(&repo, "src/lib.rs", "pub fn a() {}\n");
    write_fixture(&repo, ".env", "API_KEY=sk_live_0123456789abcdef\n");
    let config = test_config(&repo);
    let nav = navigation_for(&repo, &config);

    let listing = nav.list_directory(&repo, None, None, None, None).unwrap();

    assert!(
        !listing.paths.iter().any(|p| p == ".env"),
        "DEFAULT_RESTRICTED_PATTERNS covers .env; listing must not reveal it"
    );
}

#[test]
fn listing_outside_the_repository_is_denied() {
    let repo = temp_dir("list-escape");
    write_fixture(&repo, "src/lib.rs", "pub fn a() {}\n");
    let config = test_config(&repo);
    let nav = navigation_for(&repo, &config);

    let error = nav
        .list_directory(&repo, Some("../.."), None, None, None)
        .unwrap_err();

    assert!(matches!(error, ClientError::AccessDenied(_)), "got {error:?}");
}
```

Add the helper beside the other test helpers in this file:

```rust
fn navigation_for(repo: &Path, config: &Config) -> NavigationController {
    let scanner = SecretScanner::default();
    NavigationController::new(
        config.clone(),
        test_audit(repo, scanner.clone()),
        scanner,
        PathPolicy::new(config),
    )
}
```

- [x] **Step 2: Run to verify they fail**

Run: `cargo nextest run -p workspace-engine --test agent_tools`
Expected: FAIL to compile — `NavigationController` does not exist.

- [x] **Step 3: Add the config key**

`max_list_entries: usize`, default `200`, restrict-only, same six places as
Task 2 Step 3.

- [x] **Step 4: Write `navigation.rs`**

```rust
/// The listing and content-search tools.
///
/// Both are visitors over `tree_walk::walk`, and both resolve through
/// `PathPolicy` and redact through `SecretScanner` on the same path
/// `FileAccessController::read_file` uses. Requirement 3: these are engine
/// tools, so nothing here touches `command_policy.rs` and the agent gains
/// nothing at the shell.
#[derive(Debug, Clone)]
pub struct NavigationController {
    config: Config,
    audit_log: AuditLog,
    scanner: SecretScanner,
    path_policy: PathPolicy,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirectoryListing {
    pub paths: Vec<String>,
    /// What the walk found before the cap, so a truncated listing can say
    /// "200 of 251" rather than reading as complete.
    pub total_found: usize,
    pub truncated: bool,
}
```

`list_directory`:

1. Resolve the start directory through `path_policy.resolve_existing(root, dir.unwrap_or("."), false)` — this is what refuses `../..`.
2. `assert_not_restricted` on the resolved relative path.
3. Build the default ignore rules exactly as `index_repository` does
   (`DEFAULT_IGNORE_PATTERNS` when `config.ignore_patterns` is empty, else the
   configured list), then `parse_ignore_patterns`.
4. Walk with a visitor that, for each `WalkEvent::File`, skips anything
   `path_policy.assert_not_restricted` rejects, counts it in `total_found`, and
   pushes the path while `paths.len() < config.max_list_entries`.
5. `depth`, when given, prunes by counting `/` in the relative path.
6. `truncated = total_found > paths.len()`.
7. `audit_log.record("directory_listed", …)` with `actor`, `taskId`,
   `repositoryId`, `resourcePath` and the count — the same field names
   `file_read` uses in `file_access.rs`.

Paths only; no file contents are read, so there is nothing to redact here. The
restricted check still matters because a *path* can name a secret
(`deploy/prod-key.pem`).

- [x] **Step 5: Run the new tests**

Run: `cargo nextest run -p workspace-engine --test agent_tools`
Expected: PASS, 8 tests in this file.

- [x] **Step 6: Targeted tests, fmt and clippy**

Run: `cargo nextest run -p workspace-engine --test agent_tools` — 8 tests, all
passing. Then `cargo fmt --all -- --check` and
`cargo clippy --workspace --all-targets --locked -- -D warnings`.

- [x] **Step 7: Show the change and the gate result, and ask before committing**

Suggested subject: `List repository paths as a tool rather than a shell command`

---

### Task 4: `search_content`

**Files:**
- Modify: `crates/workspace-engine/src/navigation.rs`, `crates/workspace-engine/Cargo.toml`, `crates/workspace-engine/src/config.rs`
- Test: `crates/workspace-engine/tests/agent_tools.rs`

**Interfaces:**
- Consumes: `NavigationController` from Task 3.
- Produces: `search_content(root, pattern: &str, path_glob: Option<&str>, max_matches: Option<usize>, task_id, repository_id) -> Result<ContentSearch>`; `ContentSearch { matches: Vec<ContentMatch>, total_found: usize, files_searched: usize, truncated: bool }`; `ContentMatch { path: String, line: usize, text: String }`.

- [x] **Step 1: Write the failing tests**

```rust
#[test]
fn search_finds_call_sites_with_line_numbers() {
    let repo = temp_dir("search-basic");
    write_fixture(&repo, "src/a.rs", "fn one() {}\nfn apply_overlay() {}\n");
    write_fixture(&repo, "src/b.rs", "fn two() {\n    apply_overlay();\n}\n");
    let config = test_config(&repo);
    let nav = navigation_for(&repo, &config);

    let found = nav
        .search_content(&repo, r"apply_overlay", None, None, None, None)
        .unwrap();

    assert_eq!(found.total_found, 2);
    assert_eq!(found.matches[0].path, "src/a.rs");
    assert_eq!(found.matches[0].line, 2);
    assert_eq!(found.matches[1].path, "src/b.rs");
    assert_eq!(found.matches[1].line, 2);
    assert!(!found.truncated);
}

#[test]
fn search_redacts_a_secret_it_would_otherwise_return() {
    let repo = temp_dir("search-secret");
    // Deliberately a readable file, not `.env`: DEFAULT_RESTRICTED_PATTERNS
    // covers `.env`, so a test using one would assert refusal while claiming
    // to test redaction. Spec 18 Task 10 found this exact trap.
    write_fixture(
        &repo,
        "src/telemetry_config.rs",
        "pub const TOKEN: &str = \"AKIAIOSFODNN7EXAMPLE\";\n",
    );
    let config = test_config(&repo);
    let nav = navigation_for(&repo, &config);

    let found = nav
        .search_content(&repo, r"TOKEN", None, None, None, None)
        .unwrap();

    assert_eq!(found.total_found, 1);
    assert!(
        !found.matches[0].text.contains("AKIAIOSFODNN7EXAMPLE"),
        "a match line must be redacted before it leaves the engine: {:?}",
        found.matches[0].text
    );
    assert!(found.matches[0].text.contains("[REDACTED_"));
}

#[test]
fn search_caps_matches_and_says_what_it_cut() {
    let repo = temp_dir("search-cap");
    for n in 1..=100 {
        write_fixture(&repo, &format!("src/m{n}.rs"), "let needle = 1;\n");
    }
    let config = Config { max_search_matches: 50, ..test_config(&repo) };
    let nav = navigation_for(&repo, &config);

    let found = nav.search_content(&repo, r"needle", None, None, None, None).unwrap();

    assert_eq!(found.matches.len(), 50);
    assert_eq!(found.total_found, 100);
    assert_eq!(found.files_searched, 100);
    assert!(found.truncated);
}

#[test]
fn an_overlong_match_line_is_trimmed() {
    let repo = temp_dir("search-longline");
    write_fixture(&repo, "src/min.js", &format!("var needle={};\n", "x".repeat(4000)));
    let config = Config { max_match_line_chars: 500, ..test_config(&repo) };
    let nav = navigation_for(&repo, &config);

    let found = nav.search_content(&repo, r"needle", None, None, None, None).unwrap();

    assert!(found.matches[0].text.chars().count() <= 500);
}

#[test]
fn an_invalid_pattern_is_refused_with_the_compile_error() {
    let repo = temp_dir("search-badpattern");
    write_fixture(&repo, "src/a.rs", "fn one() {}\n");
    let config = test_config(&repo);
    let nav = navigation_for(&repo, &config);

    let error = nav
        .search_content(&repo, r"fn (one", None, None, None, None)
        .unwrap_err();

    assert!(matches!(error, ClientError::InvalidInput(_)), "got {error:?}");
    assert!(
        format!("{error}").contains("unclosed"),
        "the compile error must reach the model so it can fix the pattern: {error}"
    );
}
```

- [x] **Step 2: Run to verify they fail**

Run: `cargo nextest run -p workspace-engine --test agent_tools`
Expected: FAIL to compile — no `search_content`.

- [x] **Step 3: Add the dependency and the config keys**

`crates/workspace-engine/Cargo.toml`, in alphabetical position:

```toml
regex = "1"
```

Run `cargo tree -p workspace-engine -i regex` first to confirm the version
already resolved in `Cargo.lock`, and pin to that major. No network fetch should
occur; if `cargo` wants to update the lockfile, stop and raise it.

`max_search_matches: usize` (default `50`) and `max_match_line_chars: usize`
(default `500`), both restrict-only, same six places as Task 2 Step 3.

- [x] **Step 4: Implement `search_content`**

1. `Regex::new(pattern)` — on `Err`, return `ClientError::InvalidInput(format!("Invalid search pattern: {error}"))`. Do this **before** walking, so a bad pattern costs no I/O.
2. Resolve and check the root exactly as `list_directory` does.
3. Walk. For each file: skip restricted paths; skip anything that fails the
   binary check (`bytes.iter().take(8000).any(|b| *b == 0)`, the same rule
   `file_access.rs:70` uses); read it; count it in `files_searched`.
4. For each matching line: `total_found += 1`; while
   `matches.len() < effective_max`, push a `ContentMatch` whose `text` is the
   line **redacted** through `self.scanner.redact(line).text` and then trimmed to
   `max_match_line_chars` characters (not bytes — trim on a `char` boundary).
5. `effective_max` is `max_matches.unwrap_or(config.max_search_matches)` clamped
   **down** to `config.max_search_matches`: an argument may ask for fewer, never
   more, or the cap would be advisory.
6. `truncated = total_found > matches.len()`.
7. `audit_log.record("content_searched", …)` with the pattern, the counts, and
   the same field names Task 3 used. The pattern is model input, so redact it
   before recording it.

Redact before trimming, never after: trimming first can cut a secret in half and
leave a fragment the scanner no longer recognises.

- [x] **Step 5: Run the new tests**

Run: `cargo nextest run -p workspace-engine --test agent_tools`
Expected: PASS, 13 tests in this file.

- [x] **Step 6: Mutation-test the redaction**

Replace `self.scanner.redact(line).text` with `line.to_string()` and re-run.
`search_redacts_a_secret_it_would_otherwise_return` must fail. Restore it.

- [x] **Step 7: Targeted tests, fmt and clippy**

Run: `cargo nextest run -p workspace-engine --test agent_tools` — 13 tests, all
passing. Then `cargo fmt --all -- --check`,
`cargo clippy --workspace --all-targets --locked -- -D warnings`, and
**`cargo deny check`** — run that one here rather than deferring it, because this
is the task that adds a dependency and it is the check that confirms `regex`'s
license is already on the allow-list.

- [x] **Step 8: Show the change and the gate result, and ask before committing**

Suggested subject: `Search file contents as a tool, capped and redacted`

---

### Task 5: `edit_file` — the anchored splice

**Files:**
- Modify: `crates/workspace-engine/src/edit.rs`
- Test: `crates/workspace-engine/tests/agent_tools.rs`

**Interfaces:**
- Consumes: nothing from Tasks 1–4.
- Produces: `RegionEdit { path: String, old_text: String, new_text: String }`; `region_edits_to_changes(root: &Path, path_policy: &PathPolicy, edits: &[RegionEdit]) -> Result<Vec<ProposedChange>>`.

- [x] **Step 1: Write the failing tests**

```rust
#[test]
fn an_anchor_matching_once_splices_and_leaves_the_rest_alone() {
    let repo = temp_dir("edit-one");
    write_fixture(&repo, "src/lib.rs", "fn a() {}\nfn b() {}\nfn c() {}\n");
    let policy = PathPolicy::new(&test_config(&repo));

    let changes = region_edits_to_changes(
        &repo,
        &policy,
        &[RegionEdit {
            path: "src/lib.rs".to_string(),
            old_text: "fn b() {}".to_string(),
            new_text: "fn b(x: u8) {}".to_string(),
        }],
    )
    .unwrap();

    assert_eq!(changes.len(), 1);
    assert_eq!(changes[0].new_content, "fn a() {}\nfn b(x: u8) {}\nfn c() {}\n");
}

#[test]
fn an_anchor_matching_zero_times_is_refused() {
    let repo = temp_dir("edit-zero");
    write_fixture(&repo, "src/lib.rs", "fn a() {}\n");
    let policy = PathPolicy::new(&test_config(&repo));

    let error = region_edits_to_changes(
        &repo,
        &policy,
        &[RegionEdit {
            path: "src/lib.rs".to_string(),
            old_text: "fn missing() {}".to_string(),
            new_text: "fn other() {}".to_string(),
        }],
    )
    .unwrap_err();

    assert!(matches!(error, ClientError::InvalidInput(_)), "got {error:?}");
    assert!(format!("{error}").contains("did not match"));
}

#[test]
fn an_anchor_matching_twice_is_refused_and_names_the_count() {
    let repo = temp_dir("edit-two");
    write_fixture(&repo, "src/lib.rs", "fn a() {}\nfn a() {}\n");
    let policy = PathPolicy::new(&test_config(&repo));

    let error = region_edits_to_changes(
        &repo,
        &policy,
        &[RegionEdit {
            path: "src/lib.rs".to_string(),
            old_text: "fn a() {}".to_string(),
            new_text: "fn a(x: u8) {}".to_string(),
        }],
    )
    .unwrap_err();

    let message = format!("{error}");
    assert!(message.contains("2 times"), "the count must reach the model: {message}");
    assert!(message.contains("more context"));
}

/// Proposal §5.3: an anchor against a stale view of the file stops matching, so
/// staleness lands in a refusal rather than a wrong-region write. This is the
/// property the line-range alternative could not offer.
#[test]
fn an_anchor_against_a_changed_file_refuses_rather_than_writing_elsewhere() {
    let repo = temp_dir("edit-stale");
    write_fixture(&repo, "src/lib.rs", "fn a() {}\nfn b() {}\n");
    let policy = PathPolicy::new(&test_config(&repo));
    // The model read the file, then someone else edited it.
    write_fixture(&repo, "src/lib.rs", "fn a() {}\nfn renamed() {}\n");

    let error = region_edits_to_changes(
        &repo,
        &policy,
        &[RegionEdit {
            path: "src/lib.rs".to_string(),
            old_text: "fn b() {}".to_string(),
            new_text: "fn b(x: u8) {}".to_string(),
        }],
    )
    .unwrap_err();

    assert!(matches!(error, ClientError::InvalidInput(_)));
    assert_eq!(
        fs::read_to_string(repo.join("src/lib.rs")).unwrap(),
        "fn a() {}\nfn renamed() {}\n",
        "a refused edit must write nothing"
    );
}

/// Acceptance criterion 1: the payload is the size of the change.
#[test]
fn a_ten_line_change_in_a_150kb_file_has_a_ten_line_payload() {
    let repo = temp_dir("edit-payload");
    let mut body = (1..=6000).map(|n| format!("fn f{n}() {{}}\n")).collect::<String>();
    body.push_str("fn target() {}\n");
    write_fixture(&repo, "src/big.rs", &body);
    assert!(body.len() > 150_000, "fixture must exceed 150 KB: {}", body.len());
    let policy = PathPolicy::new(&test_config(&repo));

    let edit = RegionEdit {
        path: "src/big.rs".to_string(),
        old_text: "fn target() {}".to_string(),
        new_text: (1..=10).map(|n| format!("fn target{n}() {{}}\n")).collect(),
    };
    let payload_bytes = edit.old_text.len() + edit.new_text.len();

    let changes = region_edits_to_changes(&repo, &policy, &[edit]).unwrap();

    assert!(
        payload_bytes < 1_000,
        "the edit payload must scale with the change, not the file: {payload_bytes}"
    );
    assert!(changes[0].new_content.len() > 150_000, "the spliced result is still the whole file");
}
```

- [x] **Step 2: Run to verify they fail**

Run: `cargo nextest run -p workspace-engine --test agent_tools`
Expected: FAIL to compile — `RegionEdit` does not exist.

- [x] **Step 3: Implement the splice**

In `edit.rs`:

```rust
/// A change expressed as the text it replaces rather than the file it rewrites.
///
/// Proposal §5.3. The anchor must match exactly once: zero matches and two
/// matches are both refusals, because the alternative — picking one — makes a
/// wrong-region write that only a human reading the diff would catch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegionEdit {
    pub path: String,
    pub old_text: String,
    pub new_text: String,
}

/// Converts anchored edits into the same `ProposedChange` a whole-file proposal
/// produces, so `PatchEngine::create_patch` cannot tell the two apart and diff
/// review, hunk selection, redaction and checkpointing are untouched.
pub fn region_edits_to_changes(
    root: &Path,
    path_policy: &PathPolicy,
    edits: &[RegionEdit],
) -> Result<Vec<ProposedChange>> {
    let mut changes: Vec<ProposedChange> = Vec::new();
    for edit in edits {
        if edit.old_text.is_empty() {
            return Err(ClientError::InvalidInput(format!(
                "{}: old_text is empty; use propose_patch to create a file",
                edit.path
            )));
        }
        let target = path_policy.resolve_for_write(root, &edit.path)?;
        path_policy.assert_not_restricted(&target.relative_path, false)?;

        // A second edit to the same file builds on the first, so two edits to
        // one file in one call cannot silently discard each other.
        let current = match changes.iter().find(|c| c.path == target.relative_path) {
            Some(existing) => existing.new_content.clone(),
            None => fs::read_to_string(&target.absolute_path)?,
        };

        let count = current.matches(&edit.old_text).count();
        if count == 0 {
            return Err(ClientError::InvalidInput(format!(
                "{}: old_text did not match; the file may have changed since you read it",
                target.relative_path
            )));
        }
        if count > 1 {
            return Err(ClientError::InvalidInput(format!(
                "{}: old_text matched {count} times; include more context so it matches once",
                target.relative_path
            )));
        }
        let new_content = current.replacen(&edit.old_text, &edit.new_text, 1);

        match changes.iter_mut().find(|c| c.path == target.relative_path) {
            Some(existing) => existing.new_content = new_content,
            None => changes.push(ProposedChange {
                path: target.relative_path,
                new_content,
                status: Some("modified".to_string()),
                allow_restricted: false,
            }),
        }
    }
    Ok(changes)
}
```

- [x] **Step 4: Run the new tests**

Run: `cargo nextest run -p workspace-engine --test agent_tools`
Expected: PASS, 18 tests in this file.

- [x] **Step 5: Mutation-test the fail-closed rule**

Change `if count > 1` to `if count > 2` and re-run.
`an_anchor_matching_twice_is_refused_and_names_the_count` must fail. Restore it.
This is the assertion the whole design rests on; a test that passes either way
is worse than none.

- [x] **Step 6: Targeted tests, fmt and clippy**

Run: `cargo nextest run -p workspace-engine --test agent_tools` — 18 tests, all
passing. Then `cargo fmt --all -- --check` and
`cargo clippy --workspace --all-targets --locked -- -D warnings`.

- [x] **Step 7: Show the change and the gate result, and ask before committing**

Suggested subject: `Propose an edit as an anchored region instead of a whole file`

---

### Task 6: Wire the four tools into the turn

**Files:**
- Modify: `crates/workspace-engine/src/chat.rs:1231-1238,2033-2055,2771-2798,2807-2830,2920,3088-3099`
- Test: `crates/workspace-engine/tests/agent_tools.rs`

**Interfaces:**
- Consumes: Tasks 2–5.
- Produces: tool names `read_file` (extended), `list_directory`, `search_content`, `edit_file`.

- [x] **Step 1: Write the failing test**

```rust
/// Acceptance criterion 5: navigating requires no allowlist entry and no change
/// to the command trust boundary, because these are engine tools that never
/// reach `command_policy.rs`.
#[test]
fn navigation_needs_no_command_allowlist_entry() {
    let repo = temp_dir("tools-allowlist");
    write_fixture(&repo, "src/lib.rs", "pub fn apply_overlay() {}\n");
    let mut config = test_config(&repo);
    config.command_allowlist = Vec::new();
    config.model_providers.push(native_tool_provider());
    let engine = WorkspaceEngine::new(config);

    // Two navigation calls, then an answer. If either needed approval the turn
    // would stop with a `command_proposal` instead of reaching the third round.
    let mut adapter = MockModelAdapter::new_sequence_with_tool_calls(
        vec![
            String::new(),
            String::new(),
            "apply_overlay is defined in src/lib.rs.".to_string(),
        ],
        vec![
            vec![ToolCall {
                id: "call_1".to_string(),
                name: "list_directory".to_string(),
                arguments_json: "{}".to_string(),
            }],
            vec![ToolCall {
                id: "call_2".to_string(),
                name: "search_content".to_string(),
                arguments_json: "{\"pattern\":\"apply_overlay\"}".to_string(),
            }],
            Vec::new(),
        ],
    );
    let mut on_token = |_token: &str| {};

    let result = engine
        .chat_orchestrator
        .ask(&repo, "Where is apply_overlay defined?", &[], &mut adapter, &mut on_token)
        .unwrap();

    assert!(
        result.command_proposal.is_none(),
        "no approval may be requested for navigation"
    );
    assert!(result.response.contains("src/lib.rs"));
}

/// The trust boundary itself, pinned so a later change has to be deliberate.
#[test]
fn the_low_risk_read_only_set_is_unchanged_by_this_spec() {
    let repo = temp_dir("tools-boundary");
    let policy = CommandPolicy::new(test_config(&repo));
    for command in ["pwd", "ls", "git status", "git diff", "git log", "git show"] {
        assert!(
            !policy.classify(command, &repo).requires_approval,
            "{command} must stay approval-free"
        );
    }
    for command in ["grep -r foo .", "find . -name x", "cat src/lib.rs", "head -n 5 a"] {
        assert!(
            policy.classify(command, &repo).requires_approval,
            "{command} must still need approval: this spec gives navigation as a \
             tool, not at the shell"
        );
    }
}
```

Add this helper beside the others in the test file. It is the provider block
`foundation.rs:2348` uses to turn native tools on; without it the engine falls
back to the text envelope and no tool call is ever dispatched:

```rust
fn native_tool_provider() -> ModelProviderConfig {
    ModelProviderConfig {
        id: "openai".to_string(),
        label: "OpenAI".to_string(),
        base_url: String::new(),
        api_key_env: String::new(),
        models: Vec::new(),
        supports_native_tools: true,
        max_output_tokens: None,
        context_token_budget: None,
        provider_reports_usage: true,
        price_per_million_input_tokens: None,
        price_per_million_output_tokens: None,
    }
}
```

- [x] **Step 2: Run to verify they fail**

Run: `cargo nextest run -p workspace-engine --test agent_tools`
Expected: FAIL — `list_directory` is not a known tool, so the turn does not
dispatch it.

- [x] **Step 3: Add the tool definitions**

Beside the existing ones (`chat.rs:2985`):

```rust
fn list_directory_tool_definition() -> ToolDefinition {
    ToolDefinition {
        name: "list_directory".to_string(),
        description: "List repository-relative file paths, honouring .gitignore. Prefer this over a shell command: it needs no approval.".to_string(),
        parameters_json: "{\"type\":\"object\",\"properties\":{\"dir\":{\"type\":\"string\",\"description\":\"Repository-relative directory; defaults to the repository root\"},\"depth\":{\"type\":\"integer\",\"description\":\"Maximum directory depth to descend\"}},\"required\":[]}".to_string(),
    }
}

fn search_content_tool_definition() -> ToolDefinition {
    ToolDefinition {
        name: "search_content".to_string(),
        description: "Search file contents by regular expression and return path, line number and the matching line. Use this to find call sites; use search_codebase to find which files are about a topic.".to_string(),
        parameters_json: "{\"type\":\"object\",\"properties\":{\"pattern\":{\"type\":\"string\",\"description\":\"Regular expression\"},\"path_glob\":{\"type\":\"string\",\"description\":\"Optional glob limiting which files are searched\"},\"max_matches\":{\"type\":\"integer\",\"description\":\"Fewer matches than the configured cap; it cannot raise it\"}},\"required\":[\"pattern\"]}".to_string(),
    }
}

fn edit_file_tool_definition() -> ToolDefinition {
    ToolDefinition {
        name: "edit_file".to_string(),
        description: "Propose a change to part of a file by replacing an exact snippet. old_text must appear exactly once, or the edit is refused. Nothing is written to disk until the user approves it.".to_string(),
        parameters_json: "{\"type\":\"object\",\"properties\":{\"summary\":{\"type\":\"string\",\"description\":\"Short summary of the change\"},\"edits\":{\"type\":\"array\",\"items\":{\"type\":\"object\",\"properties\":{\"path\":{\"type\":\"string\"},\"old_text\":{\"type\":\"string\",\"description\":\"Exact text to replace; must match once\"},\"new_text\":{\"type\":\"string\"}},\"required\":[\"path\",\"old_text\",\"new_text\"]}}},\"required\":[\"summary\",\"edits\"]}".to_string(),
    }
}
```

Extend `read_file_tool_definition`'s schema with `start_line` and `end_line`
integers, and say in the description that the result states the range and the
total line count.

Push all three into the `native_tools` vector at `chat.rs:1231-1238`.

- [x] **Step 4: Add the enum variants, decode arms and marker arms**

`ToolAction` gains:

```rust
ReadFile { path: String, range: Option<LineRange> },
ListDirectory { dir: Option<String>, depth: Option<usize> },
SearchContent { pattern: String, path_glob: Option<String>, max_matches: Option<usize> },
EditFile { summary: String, edits: Vec<RegionEdit> },
```

`ReadFile` changes from a tuple variant; fix the four existing match sites the
compiler points at.

`tool_action_marker` (`:2807`) gains four arms, **all `false`**:

```rust
ToolAction::ListDirectory { dir, .. } => (
    "list_directory", dir.clone().unwrap_or_default(), false),
ToolAction::SearchContent { pattern, .. } => ("search_content", pattern.clone(), false),
// Proposing writes nothing to the repository — the same reason ProposePatch
// is false. Applying one is bracketed separately in edit.rs.
ToolAction::EditFile { summary, .. } => ("edit_file", summary.clone(), false),
```

`tool_action_from_call` (`:3088`) gains the three decode arms, following the
existing `read_file` arm's shape exactly: parse the JSON, reject a missing or
empty required field with `Ok(None)`, and return the action.

Add progress labels at `:2920`.

- [x] **Step 5: Add the dispatch arms**

Beside `ToolAction::ReadFile` (`:2033`). Each mirrors the existing arm's
`(label, content, outcome)` triple. The content strings are what the model
reads, so they carry the notices:

- `ListDirectory` → `"Listing of {dir} ({shown} of {total} paths):\n{paths}"`, and when `truncated` is false, `"Listing of {dir} ({total} paths):\n{paths}"`.
- `SearchContent` → `"{total} matches for \"{pattern}\" across {files} files (showing {shown}):\n{path}:{line}: {text}"`.
- `ReadFile` → `"Content of {path}, lines {start}–{end} of {total}:\n{content}"`, plus `" (truncated by {lines|bytes}; ask for a narrower range)"` when `truncated_by` is set.
- `EditFile` → `region_edits_to_changes`, then the **same** `PatchEngine::create_patch` call the `ProposePatch` arm makes. Reuse that arm's body rather than writing a second one; if the two bodies differ, requirement 4's promise is broken. A refusal returns `ActionOutcome::Failed` with the error text, so the model can retry within the remaining rounds.

- [x] **Step 6: Run the new tests**

Run: `cargo nextest run -p workspace-engine --test agent_tools`
Expected: PASS, 20 tests in this file.

- [x] **Step 7: Targeted tests, fmt and clippy**

Run: `cargo nextest run -p workspace-engine --test agent_tools` — 20 tests, all
passing. Then `cargo fmt --all -- --check` and
`cargo clippy --workspace --all-targets --locked -- -D warnings`.

- [x] **Step 8: Show the change and the gate result, and ask before committing**

Suggested subject: `Offer ranged reads, listing, search and anchored edits as tools`

---

### Task 7: Acceptance criteria, documentation, and closing the slice

**Files:**
- Modify: `crates/workspace-engine/tests/agent_tools.rs`, `docs/USER_GUIDE.md`, `docs/TROUBLESHOOTING.md`, `CHANGELOG.md`, `docs/specs/47_agent_working_capability/proposal.md`, `docs/specs/47_agent_working_capability/tasks.md`, `docs/specs/README.md`

- [x] **Step 1: Write the equivalence test**

The criterion no earlier task covers: a region edit and a whole-file proposal
must reach disk identically.

```rust
/// Acceptance criterion: "same review, same hunks, same checkpoint".
#[test]
fn a_region_edit_and_a_whole_file_proposal_produce_the_same_patch() {
    let repo = temp_dir("edit-equivalence");
    let original = "fn a() {}\nfn b() {}\nfn c() {}\n";
    let expected = "fn a() {}\nfn b(x: u8) {}\nfn c() {}\n";
    write_fixture(&repo, "src/lib.rs", original);
    let config = test_config(&repo);
    let scanner = SecretScanner::default();
    let engine = PatchEngine::new(
        config.clone(),
        test_audit(&repo, scanner.clone()),
        scanner,
        PathPolicy::new(&config),
    );
    let policy = PathPolicy::new(&config);

    let whole = engine
        .create_patch(&repo, &[ProposedChange {
            path: "src/lib.rs".to_string(),
            new_content: expected.to_string(),
            status: Some("modified".to_string()),
            allow_restricted: false,
        }], None, "change b")
        .unwrap();

    let region_changes = region_edits_to_changes(&repo, &policy, &[RegionEdit {
        path: "src/lib.rs".to_string(),
        old_text: "fn b() {}".to_string(),
        new_text: "fn b(x: u8) {}".to_string(),
    }]).unwrap();
    let region = engine.create_patch(&repo, &region_changes, None, "change b").unwrap();

    assert_eq!(region.files[0].new_content, whole.files[0].new_content);
    assert_eq!(region.files[0].new_hash, whole.files[0].new_hash);
    assert_eq!(region.files[0].base_hash, whole.files[0].base_hash);
    assert_eq!(region.files[0].diff, whole.files[0].diff);
    assert_eq!(region.files[0].hunks, whole.files[0].hunks);
    assert_eq!(region.files[0].status, whole.files[0].status);
}
```

- [x] **Step 2: Write the truncation-notice test**

```rust
/// §5.5: a truncated result that reads as complete is the failure this design
/// exists to prevent. One assertion per read tool.
#[test]
fn every_capped_read_tool_states_what_it_cut() {
    let repo = temp_dir("notices");
    for n in 1..=300 {
        write_fixture(&repo, &format!("src/m{n}.rs"), "let needle = 1;\n");
    }
    let body = (1..=1000).map(|n| format!("line {n}\n")).collect::<String>();
    write_fixture(&repo, "big.rs", &body);
    let config = Config {
        max_list_entries: 10,
        max_search_matches: 10,
        max_read_lines: 10,
        ..test_config(&repo)
    };
    let nav = navigation_for(&repo, &config);
    let scanner = SecretScanner::default();
    let access = FileAccessController::new(
        config.clone(), test_audit(&repo, scanner.clone()), scanner, PathPolicy::new(&config));

    let listing = nav.list_directory(&repo, None, None, None, None).unwrap();
    assert!(listing.truncated && listing.total_found > listing.paths.len());

    let found = nav.search_content(&repo, "needle", None, None, None, None).unwrap();
    assert!(found.truncated && found.total_found > found.matches.len());

    let read = access.read_file(&repo, "big.rs", None, None, false, false, None).unwrap();
    assert_eq!(read.truncated_by, Some("lines"));
    assert!(read.total_lines > read.line_range.end);
}
```

- [x] **Step 3: Write the audit test**

Requirement 7: every new tool records through `AuditLog::record`, on the same
terms as the tools it sits beside. Nothing else in this plan asserts it.

```rust
/// Requirement 7. `read_file` already audits as `file_read`; the two new
/// navigation tools must too, and an `edit_file` proposal must reach the audit
/// log by the same `patch_proposed` event a whole-file proposal produces —
/// which is the audit half of "indistinguishable downstream".
#[test]
fn every_new_tool_records_an_audit_event() {
    let repo = temp_dir("audit-tools");
    write_fixture(&repo, "src/lib.rs", "pub fn apply_overlay() {}\n");
    let config = test_config(&repo);
    let nav = navigation_for(&repo, &config);

    nav.list_directory(&repo, None, None, Some("task-1"), Some("repo-1")).unwrap();
    nav.search_content(&repo, "apply_overlay", None, None, Some("task-1"), Some("repo-1"))
        .unwrap();

    let events = fs::read_to_string(repo.join(".damaian/audit.log")).unwrap();
    assert!(events.contains("directory_listed"), "list_directory must audit");
    assert!(events.contains("content_searched"), "search_content must audit");
    assert!(
        !events.contains("AKIA") && !events.contains("sk_live"),
        "an audit entry must never carry a secret from the search it recorded"
    );
}
```

Confirm the audit log's filename and on-disk shape before writing the
assertion — `grep -n "fn record" -A 20 crates/workspace-engine/src/audit.rs`
shows where it writes. If it is not `.damaian/audit.log`, use the real path
rather than adjusting the expectation to whatever the test happens to find.

- [x] **Step 4: Run the full suite**

Run: `cargo nextest run --workspace --locked` — expected **675**. This is the
**one** full-suite run of the slice, so allow about five minutes for it and use a
`timeout` of at least 600000. Every earlier task deferred to here.

- [x] **Step 5: Documentation**

- `docs/USER_GUIDE.md`: a short section naming the four tools and saying plainly
  that none of them can write to the repository or run a command, and that an
  edit still goes through the same approval as before.
- `docs/TROUBLESHOOTING.md`: the two refusals a user will see quoted back by the
  model — "old_text did not match" (the file changed since it was read; ask it
  to re-read) and "matched N times" (the anchor was too short) — plus what a
  truncation notice means and which config key raises the cap.
- `CHANGELOG.md`: a row under **Unreleased**, component **Engine**.

- [x] **Step 6: Close the slice in all four places**

The rule in `AGENTS.md`, "Before you change a feature":

1. `proposal.md` `Status:` — requirements 1–4 Done, 5/6/8 outstanding.
2. `proposal.md` §7 Implementation Notes — what implementing found, especially
   anything that contradicted §5. Write it as spec 19's §7 is written: the
   corrections, not a summary.
3. This file's Progress table and its `**Started:** … — **Done:** …` header.
4. The `docs/specs/README.md` row.

- [x] **Step 7: Full quality gate**

All seven commands.

- [x] **Step 8: Show the change and the gate result, and ask before committing**

Suggested subject: `Document the agent tool floor and close the first slice`

---

# Agent Working Capability — Later Slices: Requirements 5, 6 and 8

> **For agentic workers:** REQUIRED SUB-SKILL: Use
> superpowers:subagent-driven-development (recommended) or
> superpowers:executing-plans to implement this plan task-by-task. Steps use
> checkbox (`- [ ]`) syntax for tracking.

**Implements:** [`proposal.md`](proposal.md) §3 requirements 5, 6 and 8 · §4
non-goals · §5.6 · §6 "Later slices" · §7 and §7.2 · background and corrections
in [`context.md`](context.md)
**Started:** 2026-09-18 — **Done:** 2026-09-18
**First slice:** requirements 1–4, above, done 2026-09-18.

**Goal:** Close out the working floor the first slice opened: a command that
can be stopped, timed out and watched while it runs (R5); a round that executes
every call the model made, not the first one, and runs the read-only ones
concurrently (R8); and a turn that reaches its round cap continuing under a
budget that is explicit, bounded and recorded instead of stopping dead, with
the within-turn message array bounded rather than carried forward (R6).

**Ordered R5 → R8 → R6.** R5 is independent of the loop and is a prerequisite
for trusting any long-running tool. R8 changes what a round *is* (calls stop
tracking rounds one-to-one), which both R6's budget accounting and eval
`toolRounds` read; it must land first so R6 is designed against the real loop.
R6 is the most speculative slice and the one §7.2 weakened; landing it last
means it is built on a stable loop and can be dropped without unwinding the
other two.

## R6 on today's evidence — read this before judging Tasks 15–17

§7.2 measured the first slice and did **not** support this spec's opening
premise that the tool surface is why sessions run out of rounds: on 14 short
scenarios the slice costs +85% model calls and +106% tokens and buys approval
stops falling 17 → 4. The long-task scenario that could test the premise is
deliberately deferred. So this plan does **not** justify R6 with "sessions run
out of rounds".

R6 is built, if the review accepts it, for two problems that are true on
today's evidence and independent of that premise:

1. **The round cap is a cliff whose only continuation is manual.** At
   `agent_max_tool_rounds` the model is handed no tools and forced to answer;
   if it still needs one, the turn ends `ToolBudgetExhausted` and the user must
   ask again. A new turn starts with a fresh task id, so ongoing in-turn state
   is not carried — the plan carry in `carry_plan_from_the_previous_turn` is
   keyed only on a token stop or a recorded supersession, not on a round stop.
2. **The within-turn message array grows without bound** (`OBSERVATIONS.md`
   entry 6, quoted in `context.md` §2.5): an assistant message and a tool
   result per round, and a tool result can be a whole file. This is the half of
   compaction that actually stops long tasks. A continuation that carries the
   array forward continues the problem rather than the task.

**If the review prefers to wait, drop Tasks 15–17 and say so in the proposal's
`Status:` line and in `docs/PLAN/OBSERVATIONS.md` entry 6** — the continuation
is then deferred to a long-task measurement, which is the honest disposition
given §7.2. That is a legitimate outcome. What is not legitimate is building it
on the opening paragraph.

## Architecture

- **R5** keeps `CommandRunner` the single place a child is spawned, now with
  two reader threads draining the pipes while a poll loop watches a deadline
  and the `CancelToken`. Output is redacted per line for the live stream and
  redacted whole for the persisted `CommandExecution`, so the stream cannot
  step around the scanner. Termination kills the child's own process group
  through the same `CommandGuard` spec 46 introduced. A killed command reports
  `exit_code: None`, which `finish_command_action` already maps to `unknown` —
  never `ok` — which is the fail-closed answer for a command that may have
  mutated the repository before it died.
- **R8** makes one round execute every decodable call. Batching is the
  structural change; concurrency is second. The read-only set is derived from
  one exhaustive effect classification whose side-effect answer is the same one
  spec 17's recovery already depends on. When *every* decoded call in a round
  is read-only, they run on scoped threads and are collected by index; markers,
  messages and results are then written in the original order, so the session
  log's `seq` order is deterministic. A round with any non-read-only call falls
  back to today's sequential, in-order dispatch.
- **R6** uses `agent_max_task_tokens` as the governing budget and invents no
  second one. At the round cap, and only when a ceiling is set and the model
  still wants a tool, the turn takes another round segment and records
  `turn_continued`; the next iteration's existing token check bounds it, and a
  hard total-round constant bounds it even if a provider reports no usage. The
  in-memory array is clamped to a bounded window with a stated elision, so what
  is carried forward is the plan (task state), not the transcript.

**Tech Stack:** Rust 2024. **No new dependencies.** Concurrency is
`std::thread::scope`, the primitive `context.md` §3 names.

## Progress (later slices)

| Task | State | Notes (the plan-correction column) |
|---|---|---|
| 8 · Command runner: stream, time out, cancel | Done | `CommandRunOptions` (approved/approved_by/task_id + cancel + on_output), `classify_wait`, `WaitDecision`, `CommandTermination`, reader threads, poll loop with deadline + kill via `kill_process_group`; `command_timeout_secs` (default 600) restrict-only and in spec 34's table. 4 pure unit tests + 4 `#[ignore]`d shell tests (all four run manually, green). **Plan deviations:** (1) the key is `usize`, not the plan's `u64`, so it reuses `restrict_only_limit` and the `parse_read_lines` shape; (2) `WaitDecision` is returned as `Option` with `Exited` handled by `try_wait`, rather than a three-variant enum, because "keep waiting" is the third state a three-variant enum cannot hold. Deadline mutation-tested (`&& false` fails `a_command_past_its_deadline_is_timed_out`). Workspace `cargo check --all-targets` green; `cargo clippy -p workspace-engine --all-targets -D warnings` green. Not committed. |
| 9 · Thread cancel/timeout/output through the turn | Done | `run_proposal` gained `task_id`/`cancel`/`on_output`; the turn's command arm and resume path forward the sink's cancel and stream output as `PhaseKind::Output`; desktop-shell and CLI pass a never-cancelled token and a no-op sink; `sandbox_command_context` names a killed command's termination instead of printing `-1`; web UI appends output to its own element and style.css gained a rule. Failing-first test `a_timed_out_command_reports_its_termination_not_a_negative_exit_code` (unit, no shell). Clippy on the three changed crates green. Not committed. |
| 10 · R5 acceptance, docs, and the command-lifecycle tests | Done | R5's acceptance is carried by the five `#[ignore]`d tests in `tests/command_lifecycle.rs` (timeout kill, pre-dispatch stop, mid-command stop, streaming, stream redaction), all run manually green; the pure decisions are non-ignored unit tests. Cancel-decision mutation-tested (`&& false` fails both cancel unit tests). **Plan deviation:** the plan's "add a CHANGELOG row under Unreleased" conflicts with `AGENTS.md` and with what the first slice actually did — Unreleased lists *specified but not built* work, and release rows are written by the pipeline. No spec-47 bullet remains to move, so `CHANGELOG.md` is untouched. `TROUBLESHOOTING.md` documents termination and the timeout instead. Not committed. |
| 11 · Execute every decodable call in a round | Done | `first_decodable_tool_action` replaced by `decodable_tool_actions`; the round's calls are decoded in order and each is dispatched, with model messages pushed per call (the first carrying the prose/reasoning, later ones empty so it is not duplicated). **Plan deviation:** `PendingChatTurn.matched_tool_call` was left singular — a terminal call ends the round, so the resumed turn only ever needs that one call, and prior calls were already fed back before the break. Test `a_round_executes_every_read_only_call_it_was_given`; mutation-tested (`actions.truncate(1)` fails it, showing exactly the old drop). |
| 12 · One effect classification; concurrent read-only batch | Done | `ActionEffect { ReadOnly, WritesRepository, ShapesTurn }` is the one exhaustive match; `tool_action_marker`'s recovery bool and `action_is_batchable_read_only` are both derived from it. `dispatch_read_only_action` is the shared dispatch for the six read-only tools; `run_read_only_batch` uses `std::thread::scope` and joins in index order. **Plan note:** this is the one place the review should check that "derive from `tool_action_marker`" is honoured — the bool and the batchable flag now share `action_effect`, which is what the brief asked for, but `tool_action_marker`'s body changed. Ordering mutation-tested (reversing the collected vector fails `a_read_only_batch_records_results_in_the_models_call_order`). `ChatOrchestrator` is `Sync`, so scoped threads needed no clone-per-thread. |
| 13 · R8 acceptance: mixed rounds, stop-in-batch, `seq` determinism | Done | Three tests: mixed read+mutation runs the mutation sequentially and records the read; three distinct reads are recorded in call order; three fresh runs of a read-only round produce an identical session sequence. Batch-boundary mutation-tested (forcing `action_is_batchable_read_only` true sends `Command` into the read-only dispatcher and fails the mixed test). **Limitation, stated rather than hidden:** the pre-dispatch stop check prevents any call starting after a stop, and a stop before the round skips the batch; an already-in-flight read is not interrupted because the read tools take no cancel token (they are bounded local reads). A mid-batch timing test is not deterministically constructible, so it is covered structurally, not by a flaky test. |
| 14 · R8 eval scenario and the two new assertions | Done | New `tool_calls_at_least` and `tool_rounds_at_most` on `Asserts`, and `scenarios/batched_reads.toml` (two reads in one message; asserts `tool_calls_at_least = 2`, `tool_rounds_at_most = 1`, both `deterministic_only`). Both harness count guards moved 15 → 16; spec 18's `Status:` and `docs/specs/README.md` row 18 updated from the stale "thirteen". Full `cargo test -p eval-harness --test harness`: 61 passed, 1 ignored; the deterministic tier runs all sixteen. **Deliberate deferral:** `evals/baseline.json` is not regenerated here — it is a human review gate, so it is done in Task 18 with the counts read, not as a side effect of this task. |
| 15 · Bound the within-turn message array | Done | `agent_max_turn_messages` (default 24) Restrict-only and in spec 34's table; `bounded_messages` keeps the system+user seed, drops the oldest call/result pairs in whole pairs, and inserts a system notice naming the elision. Wired into the `ModelRequest` and the audit estimate. Two unit tests (clamped-and-stated, short-unchanged) and a trust test; clamp mutation-tested (`if true` fails the bounded test at 42 messages). |
| 16 · Bounded, recorded continuation past the round cap | Done | At `force_final`, when the model still wants a tool and `agent_max_task_tokens` is set, the turn records `turn_continued`, takes another round segment and continues; the existing pre-call token check is the budget, and `round < ABSOLUTE_TOOL_ROUND_CAP` (16) is the safety net for a provider whose usage never reaches the ceiling. With no ceiling the old `ToolBudget` stop stands. Three new tests (continues+recorded+Complete, no-ceiling-still-stops, plan-intact). Guard mutation-tested (dropping `is_some()` makes the no-ceiling test run 17 calls and never stop). **Changed an existing spec-21 test:** `a_ceiling_the_turn_never_approaches_changes_nothing` is renamed `…_does_not_stop_it` and now asserts `calls > 8`, because a roomy ceiling no longer "changes nothing" — it continues to the absolute cap. Its old `calls > 2` could no longer tell a continuation from the old single forced-final round. |
| 17 · R6 acceptance and the honest write-up | Done | The three clauses — continues, state intact, bounded and recorded — are asserted together by `a_turn_continues_past_its_round_cap_under_the_ceiling` and `a_continuation_keeps_the_turns_plan_intact` (plan read back from the session store; `turn_continued` read from the audit log). `proposal.md` §7 records R6 as built on the cliff and the unbounded array, explicitly **not** on §7.2's unsupported premise, and states that §7.2 did not support it. `docs/PLAN/OBSERVATIONS.md` is uncommitted and absent from this worktree, so the disposition for entry 6 is written for the user to carry over rather than edited in the shared main checkout. |
| 18 · Close the spec: docs, summaries, full gate | Done | All four summaries moved together: `proposal.md` `Status:` Done with §7.3 added, this file's header/rows, and `docs/specs/README.md` row 47. `docs/USER_GUIDE.md` documents stop/timeout/streaming, the read-only batch order, and the continuation/window keys. **Full seven-command gate green** on the finished state: `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets --locked -- -D warnings`, `cargo nextest run --workspace --locked` (**705 passed, 21 skipped**), `node --check crates/desktop-shell/static/app.js`, `npm run lint:web`, `typos`, `cargo deny check` (all ok). The five `#[ignore]`d command-lifecycle tests were run manually and passed. **`evals/baseline.json` regenerated** — it was stale at 14 records and now holds all 16 (adding `navigated_edit` and `batched_reads`); every added record was read before writing and every assertion passes, with no seeded secret present. **Two carry-overs for the user:** (1) the regenerated baseline is a human review gate and needs their read before any commit; (2) `docs/PLAN/OBSERVATIONS.md` entry 6 is uncommitted and absent from this branch, so its disposition is written in §7.3 item 6 of the proposal and must be pasted into the entry in the shared checkout. `CHANGELOG.md` is deliberately untouched — Unreleased lists *specified but not built* work and the release pipeline writes release rows (see Task 10's note). |

Fill this table as each task lands, and use the Notes cell for what the plan
got wrong — the file is not a record if the column stays empty. Per
`AGENTS.md`, a task is not done until its row is updated.

## Global Constraints (later slices)

The first slice's Global Constraints still bind. These are added or restated
because they are the ones this half can most easily break.

- **Requirements 3 and 7 are constraints, not tasks.** No new
  `command_allowlist` entry, no change to `is_low_risk_read_only`, no
  relaxation of the shell-control gate (`command_policy.rs` is not touched);
  every new tool and every command records through `AuditLog::record`.
- **No new autonomy.** R5 can only stop a command, never start one; R8 runs
  only calls the model already made; R6 changes when a turn stops, not what it
  may do. Nothing here writes to the repository except the same
  `PatchEngine::create_patch` path the user already reviews.
- **Do not remove the tool-round cap.** R6 replaces the fixed count with the
  existing token budget as the governing bound and keeps an absolute
  total-round safety constant. An unbounded turn is not the goal.
- **Trust-boundary classification.** Every new `ConfigOverlay` field is
  classified in `Config::apply_overlay_scoped`'s exhaustive destructuring (a
  missed field is a compile error) **and** added to spec 34's §5.1 table.
  `command_timeout_secs` and `agent_max_turn_messages` are Restrict-only
  ("lower value wins"), like the first slice's four caps.
- **Every capped or elided result states what it cut** (§5.5). An elided round
  or a truncated command stream that reads as complete is the failure this
  rule exists to prevent.
- **Tests that spawn a shell are `#[ignore]`d with a manual command in the doc
  comment** (`AGENTS.md`). R5's real termination tests are of that kind; the
  pure wait/kill/redaction decisions they depend on must still have
  non-ignored unit tests, or R5 has no default-suite coverage at all.
- **New tests use `test_config`** (`tests/foundation.rs`), with
  `enable_index_watcher: false`.
- **Per task run only the targeted tests; run the full gate once, in Task 18.**
  Local costs measured 2026-09-18: full `cargo nextest run --workspace` ~5 min,
  `cargo clippy --workspace --all-targets` up to 18 min cold. Scope with `-p`
  and `--test` until the end.
- **The gate still gates.** All seven `AGENTS.md` commands, run in Task 18.
  `typos` and `node --check crates/desktop-shell/static/app.js` are the two
  most often missed.
- **Commit messages:** one subject line, no body, no `Co-Authored-By`. **Do
  not commit without asking.** Show the change and the gate result; Damijan
  decides. Never cite commit SHAs in documentation.

## File Structure

| File | Responsibility |
|---|---|
| `crates/workspace-engine/src/command_runner.rs` | `run` gains a deadline, a `CancelToken` and an output callback; reader threads drain the pipes; kill via the existing `CommandGuard`; `CommandExecution` gains a termination reason. |
| `crates/workspace-engine/src/validation.rs` | `run_proposal` passes cancel/output/timeout through and threads them from the turn; `CommandExecution` summary records the termination. |
| `crates/workspace-engine/src/chat.rs` | R5: command arm forwards the sink's cancel and output; R8: decode every call, effect classification, concurrent read-only dispatch, ordered recording; R6: continuation at the cap and the message-window clamp. |
| `crates/workspace-engine/src/config.rs` | `command_timeout_secs`, `agent_max_turn_messages`; both Restrict-only; parse, default, overlay, serialization. |
| `crates/desktop-shell/static/app.js` | A new output progress kind rendered as a log line rather than a status badge. |
| `crates/desktop-shell/src/lib.rs` | `PhaseKind::Output` reaches the SSE phase event (R5). |
| `crates/damaian-cli/src/main.rs` | The manual command path passes a never-cancelled token and a no-op output sink. |
| `crates/workspace-engine/tests/command_lifecycle.rs` | **New.** R5 end-to-end termination/streaming/redaction where a real shell is needed. |
| `crates/workspace-engine/tests/agent_tools.rs` | R8 batching and concurrency, beside the tool tests it extends. |
| `crates/workspace-engine/tests/token_ceiling.rs` | R6 continuation and the array bound. |
| `crates/eval-harness/` | R8 scenario, two new assertions, count guards, baseline. |
| `docs/specs/34_repository_config_trust_boundary.md` | The two new Restrict-only keys in §5.1's table. |

---

### Task 8: Command runner — stream, time out, cancel

**Files:**
- Modify: `crates/workspace-engine/src/command_runner.rs`, `crates/workspace-engine/src/config.rs`, `docs/specs/34_repository_config_trust_boundary.md`
- Test: inline `#[cfg(test)]` module in `command_runner.rs`; `crates/workspace-engine/tests/command_lifecycle.rs` (new, ignored shell tests)

**Interfaces (produced):**
- `Config::command_timeout_secs: u64`, default `600`, Restrict-only.
- `CommandRunner::run` takes `options: CommandRunOptions<'_>` instead of the
  bare `approved`/`approved_by`/`task_id` tail, to stay under clippy's
  argument bound and to group the new per-run side channel:
  ```rust
  pub struct CommandRunOptions<'a> {
      pub approved: bool,
      pub approved_by: Option<&'a str>,
      pub task_id: Option<&'a str>,
      pub cancel: &'a CancelToken,
      pub on_output: &'a mut dyn FnMut(&str),
  }
  ```
- `CommandExecution` gains `pub termination: CommandTermination`, where
  `CommandTermination { Exited, TimedOut, Cancelled }`; `serialize_execution_summary`
  writes a `TERMINATION` field. `exit_code` stays `Option<i32>` and stays `None`
  for both killed cases, so `finish_command_action`'s `None → unknown` rule
  (spec 17) is untouched.

- [ ] **Step 1: Write the failing unit tests**

The load-bearing decisions must be testable without spawning a shell. Factor
them as functions over plain data and test those first:

```rust
#[test]
fn a_command_past_its_deadline_is_timed_out_not_exited() {
    let outcome = classify_wait(Some(Duration::from_secs(600)), Instant::now(), false);
    assert_eq!(outcome, WaitDecision::TimedOut);
}

#[test]
fn a_cancelled_command_is_cancelled_before_its_deadline() {
    let outcome = classify_wait(Some(Duration::from_secs(1)), Instant::now(), true);
    assert_eq!(outcome, WaitDecision::Cancelled);
}

#[test]
fn a_reaped_child_is_exited_whatever_the_clock_says() {
    let outcome = classify_wait(None, Instant::now(), false);
    assert_eq!(outcome, WaitDecision::Exited);
}
```

- [ ] **Step 2: Run to verify they fail**

Run: `cargo nextest run -p workspace-engine -E 'test(command_runner)'`
Expected: FAIL to compile — `classify_wait` and `WaitDecision` do not exist.

- [ ] **Step 3: Add `classify_wait` and the termination reason**

A pure function over `(deadline: Option<Instant>, now, cancelled)` returning
`WaitDecision { Exited, TimedOut, Cancelled }`, with a comment saying why it is
separate from the process plumbing: the decision is the part that must be
testable on the default suite. Add `CommandTermination` and the config key.

- [ ] **Step 4: Add the config key and its trust classification**

`command_timeout_secs: u64`, default `600`, a `parse_positive` arm that refuses
zero (a zero timeout would kill every command at spawn), Restrict-only in the
overlay, and the serialization row. Add it to spec 34 §5.1's Restrict-only row
with the reason: a repository may shorten how long its own commands run, never
lengthen one past the user's setting.

- [ ] **Step 5: Rewrite `run` to drain, watch and stream**

Spawn as today, then:
1. Take `child.stdout`/`child.stderr`; spawn one reader thread per pipe that
   sends `StreamChunk { stream, text }` by `BufRead::read_until(b'\n')` over an
   `mpsc` channel and drops the sender when the pipe closes.
2. Poll `child.try_wait()` and the channel on a short sleep. On each chunk,
   redact **per line** through `self.scanner` and call `on_output`. Keep the
   raw bytes so the final whole-output redaction still runs (a secret split
   across a chunk boundary must not survive the persisted copy).
3. Ask `classify_wait` each iteration. On `TimedOut` or `Cancelled`, kill the
   process group through `CommandGuard` and set the termination; on `Exited`,
   read the status.
4. Join the readers, drain the channel, and build `CommandExecution` exactly as
   before — with `exit_code: status.code()` (`None` when signalled),
   `termination`, and the redacted output. `truncate_output` keeps its current
   tail behaviour; changing it is a separate `OBSERVATIONS.md` entry
   (`context.md` §4), not this task.

- [ ] **Step 6: Write the ignored shell tests and run them by hand**

In `tests/command_lifecycle.rs`, each `#[ignore]`d with its manual command:
`a_command_past_its_timeout_is_killed_and_reports_no_exit_code` (a `sleep` with
a one-second timeout), `output_is_streamed_before_the_command_exits` (a command
that prints a line then sleeps; assert the callback saw the line before the
kill), and `a_secret_in_streamed_output_is_redacted` (echo a fake key). Run
them with `cargo test -p workspace-engine --test command_lifecycle -- --ignored`
and record the result in the Progress row.

- [ ] **Step 7: Mutation-test the deadline**

Change `classify_wait` to ignore the deadline (always `Exited` unless
cancelled) and re-run Step 1. `a_command_past_its_deadline_is_timed_out_not_exited`
must fail. Restore it. A timeout the tests cannot distinguish from no timeout is
not a timeout.

- [ ] **Step 8: Targeted tests, fmt and clippy; show the change and ask**

Run `cargo nextest run -p workspace-engine -E 'test(command_runner)'`, then
`cargo fmt --all -- --check` and
`cargo clippy -p workspace-engine --all-targets --locked -- -D warnings`.
Suggested subject: `Give a command a deadline, a stop, and a live output stream`

---

### Task 9: Thread cancel, timeout and output through the turn

**Files:**
- Modify: `crates/workspace-engine/src/validation.rs`, `crates/workspace-engine/src/chat.rs`, `crates/workspace-engine/src/workspace_engine.rs`, `crates/desktop-shell/src/lib.rs`, `crates/damaian-cli/src/main.rs`, `crates/desktop-shell/static/app.js`
- Test: `crates/workspace-engine/tests/command_lifecycle.rs`, `tests/checkpoint_wiring.rs`

**Interfaces:**
- `ValidationOrchestrator::run_proposal(&self, proposal_id, approved, approved_by, cancel: &CancelToken, on_output: &mut dyn FnMut(&str))`.
- A new progress kind so output does not masquerade as a status badge:
  `PhaseKind::Output` (`as_str() == "output"`), carried through the existing
  `TurnProgress::Phase` event. The shell renders it as a log line.

- [ ] **Step 1: Write the failing test**

A native-tool provider, a `CancelToken` the test cancels from a second thread
after the command starts, and a marker assertion: the turn returns
`cancelled: true`, and the session log's `run_command` marker finishes
`unknown` — never `ok`. Add the ignored companion that cancels mid-`sleep`.

- [ ] **Step 2: Run to verify it fails**

Run: `cargo nextest run -p workspace-engine --test checkpoint_wiring` (the test
can live here) or the new file. Expected: FAIL to compile — `run_proposal`
takes three arguments.

- [ ] **Step 3: Thread the side channel**

Update `run_proposal` and its four call sites (`chat.rs` ×2, `desktop-shell`,
`damaian-cli`). The CLI and any non-turn caller pass `&CancelToken::new()` and
a no-op callback. In the chat command arm, build an output closure that calls
`sink.phase(PhaseKind::Output, line, round, max_rounds)` (the borrow may need
`sink.on_progress` bound to a local first). After `run_proposal` returns, check
`sink.cancel.is_cancelled()` before finishing the action/feeding the result,
and route through `finish_cancelled_turn` so a stop during a command is the
same stop the rest of the loop already produces.

- [ ] **Step 4: Shell wiring**

`turn_progress_event` and the SSE phase writer already handle `TurnPhase`;
`PhaseKind::Output` flows through unchanged. Update the web UI's phase handler so
an `output` kind is appended as a line rather than replacing the status label.
Run `node --check crates/desktop-shell/static/app.js` and `npm run lint:web`.

- [ ] **Step 5: Run the targeted tests, fmt and clippy; show the change and ask**

Suggested subject: `Let a running command report progress and take the turn's stop`

---

### Task 10: R5 acceptance, docs, and the command-lifecycle tests

**Files:**
- Modify: `crates/workspace-engine/tests/command_lifecycle.rs`, `docs/TROUBLESHOOTING.md`, `CHANGELOG.md`

- [ ] **Step 1: Cover the acceptance criteria**

The criteria no earlier task proves: (a) a command past its timeout is
terminated and reports no exit code; (b) a stop during a command takes effect
during it rather than after; (c) output before exit. Because all three need a
real shell, they are `#[ignore]`d, and this is stated in the Progress row
rather than hidden.

- [ ] **Step 2: Mutation-test the ordering**

Break the post-`run_proposal` cancel check (remove it) and confirm the
cancelled-turn test fails. Restore it. This is the assertion that a stop is
honoured during a command rather than after it.

- [ ] **Step 3: Docs**

`TROUBLESHOOTING.md`: what a killed/timed-out command looks like and which key
raises the timeout. `CHANGELOG.md`: an `Unreleased` / **Engine** row.

- [ ] **Step 4: Show the change and the gate result, and ask before committing**

Suggested subject: `Prove a command can be stopped, timed out and watched`

---

### Task 11: Execute every decodable call in a round

This is R8's first step (§5.6): batching, no concurrency yet. It must be
transparent for the one-call round, which is every round today.

**Files:**
- Modify: `crates/workspace-engine/src/chat.rs`
- Test: `crates/workspace-engine/tests/agent_tools.rs`

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn a_round_executes_every_read_only_call_it_was_given() {
    // One model turn with two reads, then a final answer.
    // Assert both results reached the next request: today the second is
    // silently dropped, so this fails before the change.
}
```

Use `MockModelAdapter::new_sequence_with_tool_calls` with two `ToolCall`s in
the first turn. Assert the second call's result appears in the following
request (or in the session log) — not just that the turn ends.

- [ ] **Step 2: Run to verify it fails for the intended reason**

Run: `cargo nextest run -p workspace-engine --test agent_tools -E 'test(every_read_only)'`.
Expected: FAIL because only the first call is dispatched — confirm the failure
message says the second result is missing, not a compile error.

- [ ] **Step 3: Replace `first_decodable_tool_action` with `decodable_tool_actions`**

Return every call that decodes, in order, plus the per-call decode errors, plus
whether the text-envelope fallback applies (it applies only when no native call
decoded, preserving today's rule). Keep the single-call path behaviour
identical: one assistant message carrying the calls, one tool message per call,
in order.

- [ ] **Step 4: Widen the pending-turn state**

`PendingChatTurn.matched_tool_call: Option<ToolCall>` becomes
`matched_tool_calls: Vec<ToolCall>` with `#[serde(default)]`, and the old
singular field is kept as a read-only fallback so pending turns written before
the upgrade still resume. Both `resume_after_command_decision` and the plan
review path replay the calls.

- [ ] **Step 5: Run the agent-tool tests; fmt and clippy; show and ask**

Suggested subject: `Run every tool call a round asked for, not just the first`

---

### Task 12: One effect classification, and concurrent read-only dispatch

**Files:**
- Modify: `crates/workspace-engine/src/chat.rs`
- Test: `crates/workspace-engine/tests/agent_tools.rs`

**Design note the reviewer should check.** §5.6 says batchability must be
derived from `tool_action_marker`'s side-effect answer, not a second list.
But `side_effecting == false` also covers `propose_patch`, `edit_file`,
`propose_plan` and `complete_step`, which shape or end the turn. This plan
resolves that by making the *one* place the recovery answer lives an exhaustive
effect classification, with the recovery bool and the batchable bool both
derived from it:

```rust
enum ActionEffect { ReadOnly, WritesRepository, ShapesTurn }
fn action_effect(action: &ToolAction) -> ActionEffect { /* exhaustive, no _ */ }
fn tool_action_marker(action: &ToolAction) -> (&'static str, String, bool) {
    // spec 17's bool, unchanged in value: ShapesTurn is not side-effecting.
    (name(action), reference(action), matches!(action_effect(action), ActionEffect::WritesRepository))
}
fn action_is_batchable_read_only(action: &ToolAction) -> bool {
    matches!(action_effect(action), ActionEffect::ReadOnly)
}
```

That keeps one source of truth but changes `tool_action_marker`'s body; spec
17's tests are the guard that the bool did not change value.

- [ ] **Step 1: Write the failing equivalence test**

A round whose calls are all read-only must produce byte-identical results and
session-log order to the same calls run sequentially. Assert on the tool
results, the action-marker order, and the appended messages; do not assert on
wall-clock alone (§6 says so explicitly).

- [ ] **Step 2: Run to verify it fails for the intended reason**

Expected: FAIL because there is one shared result path, or because ordering
differs — not a compile error.

- [ ] **Step 3: Add the effect classification and dispatch**

If every decoded call is `ActionEffect::ReadOnly`, run them on
`std::thread::scope` collecting `Vec<Result<...>>` by index; otherwise dispatch
sequentially in order exactly as today (which also satisfies "a round mixing
read-only and mutating calls runs the mutating ones sequentially"). Record
markers, messages and results in index order after the batch returns, so the
session-log `seq` order is deterministic by construction. Do not start markers
before the batch: a crash mid-batch has no external side effect because the
batch is read-only, and recording after keeps the sequential log shape.

- [ ] **Step 4: Falsify the ordering guarantee**

Make the collection read into arbitrary order (e.g. collect by completion) and
confirm the equivalence test fails. Restore index-order collection.

- [ ] **Step 5: Run the targeted tests; fmt and clippy; show and ask**

Suggested subject: `Dispatch a round's read-only calls concurrently, in order`

---

### Task 13: R8 acceptance — mixed rounds, stop-in-batch, `seq` determinism

**Files:**
- Test: `crates/workspace-engine/tests/agent_tools.rs`

- [ ] **Step 1: Mixed round**

A round with a read and a command/`propose_patch` runs the mutating one
sequentially. Assert the mutating action's marker is the same shape as today
and the read's result still appears.

- [ ] **Step 2: Stop during a concurrent batch**

With the token cancelled before dispatch, no tool result is fed back and the
turn is `cancelled`. The mid-batch case needs a slow tool and is an
`#[ignore]`d timing test with its manual command; state that in the row.

- [ ] **Step 3: `seq` determinism**

Run the same all-read-only round five times and assert the session log's
`seq`-ordered marker/message sequence is identical each time.

- [ ] **Step 4: Mutation-test the batch boundary**

Make `action_is_batchable_read_only` return true for `ActionEffect::ShapesTurn`
and confirm the mixed-round test fails (a patch proposal ran concurrently with
a read). Restore it.

- [ ] **Step 5: Show the change and the gate result, and ask before committing**

Suggested subject: `Pin the concurrent batch's ordering and its boundaries`

---

### Task 14: R8 eval scenario and the two new assertions

The deterministic tier scripts every call, so it can prove the batch executed
both calls but cannot prove fewer rounds; that is a live-tier question and must
be said in the scenario's comment, as `navigated_edit` already does.

**Files:**
- Modify: `crates/eval-harness/src/scenario.rs`, `crates/eval-harness/src/assertions.rs`, `crates/eval-harness/scenarios/` (new scenario), `crates/eval-harness/tests/harness.rs`, `docs/specs/18_local_evaluation_harness/proposal.md`, `docs/specs/README.md`, `evals/baseline.json`

- [ ] **Step 1: Add the two assertions**

`tool_calls_at_least: Option<u64>` and `tool_rounds_at_most: Option<u64>` on
`Asserts`, evaluated against the run record (which already reads the session
log, so they are not script-derived). A scenario using them lists them in
`deterministic_only` because they depend on the scripted calls.

- [ ] **Step 2: Add the scenario**

A deterministic scenario whose first turn carries two read-only calls in one
message, with `tool_calls_at_least = 2` and `tool_rounds_at_most = 1`. Before
Task 11 this scenario fails (the second call is dropped), which is the
falsification that it measures batching.

- [ ] **Step 3: Update the two count guards and the summaries**

`tests/harness.rs` currently asserts 15 in two places; update both to 16 and
the doc comments. Update spec 18's `Status:`/§5.4 count (already stale at
"thirteen") and `docs/specs/README.md` row 18, per `AGENTS.md`'s four-places
rule. Tests whose numbers change because of the scenario count are part of
this task.

- [ ] **Step 4: Regenerate the baseline — deliberately**

Only now, and only because a scenario was added. Run
`cargo run -q -p eval-harness --bin damaian-eval -- --tier deterministic --format json > evals/baseline.json`,
then read every changed number before accepting it; a baseline nobody read is
worse than none (spec 18 §5.7). Record in the Progress row whether a human
reviewed it.

- [ ] **Step 5: Show the change and the gate result, and ask before committing**

Suggested subject: `Measure that a round runs every read-only call it was given`

---

### Task 15: Bound the within-turn message array

This is the half of R6 the spec did not originally know about (`context.md`
§2.5). It is independently defensible: the array grows on every turn with a
tool result, not only on continuation.

**Files:**
- Modify: `crates/workspace-engine/src/config.rs`, `crates/workspace-engine/src/chat.rs`, `docs/specs/34_repository_config_trust_boundary.md`
- Test: `crates/workspace-engine/tests/token_ceiling.rs`

**Interfaces:**
- `Config::agent_max_turn_messages: usize`, default `24`, Restrict-only.
- A clamp applied before each `ModelRequest`: keep the initial system + user
  messages, keep the most recent whole round-pairs up to the cap, and insert a
  single notice naming how many rounds were elided. Never split an assistant
  `tool_calls` message from its `tool` results.

- [ ] **Step 1: Write the failing test**

Drive a turn with many tool rounds and capture the messages the adapter was
asked to send. Assert the array never exceeds the cap, that the first two
messages are intact, and that the elision notice names a nonzero count. Assert
the session log still holds every round — only the request is bounded.

- [ ] **Step 2: Run to verify it fails for the intended reason**

Expected: FAIL because no clamp exists, so the request grows past the cap.

- [ ] **Step 3: Add the key, the classification, and the clamp**

Add the key exactly as Task 8 added its own, including the spec 34 row. Apply
the clamp in the loop before building the `ModelRequest`. Elide in whole
round-pairs and state the count.

- [ ] **Step 4: Mutation-test the clamp**

Set the cap to a value larger than the array and confirm the test fails
(nothing is elided when it should be), then set it to a value that would split
a pair and confirm the pairing assertion fails. Restore.

- [ ] **Step 5: Show the change and the gate result, and ask before committing**

Suggested subject: `Bound the in-turn message array and say what was elided`

---

### Task 16: Bounded, recorded continuation past the round cap

**Files:**
- Modify: `crates/workspace-engine/src/chat.rs`
- Test: `crates/workspace-engine/tests/token_ceiling.rs`, `crates/workspace-engine/tests/plan_turn.rs`

**Design.** At `force_final`, when `model_output_requests_tool` and
`agent_max_task_tokens.is_some()` and the total round count is below a hard
safety constant, do not take the `ToolBudget` break: record
`turn_continued` (session, task, round, continuation number, ceiling, spent),
extend `max_rounds` by another default segment, and continue. The existing
pre-call token check then bounds the turn, and `StopReason::TokenBudget` /
`TaskStatus::TokenBudgetExhausted` are the stop. With no ceiling set, today's
`ToolBudget` behaviour is unchanged. A `const ABSOLUTE_CONTINUED_ROUND_CAP`
bounds total rounds even if a provider reports no usage, so a mock cannot loop
forever.

- [ ] **Step 1: Write the failing tests**

1. **Continuation happens and is recorded.** A token ceiling high enough not to
   bind, a low `agent_max_tool_rounds`, and a mock that requests tools for two
   segments then answers. Assert the turn answered instead of
   `ToolBudgetExhausted`, and that the audit log contains `turn_continued` with
   the round and the ceiling.
2. **The budget bounds it.** A ceiling that binds mid-continuation stops the
   turn with `TokenBudget`/`TokenBudgetExhausted`, and the plan is intact.
3. **No ceiling, no continuation.** With `agent_max_task_tokens = None` the
   turn still stops `ToolBudgetExhausted` exactly as before.
4. **State intact across a continuation** (in `plan_turn.rs`): a plan proposed
   before the cap still has its current step after the continuation, and a
   `complete_step` across the boundary advances it.

- [ ] **Step 2: Run to verify they fail for the intended reason**

Expected: test 1 fails with `ToolBudgetExhausted` today; tests 2–4 encode
existing behaviour and must pass before and after (they are the regression
guard).

- [ ] **Step 3: Implement the continuation**

Add the `turn_continued` audit event, the continuation branch at `force_final`,
the extended `max_rounds`, and the hard constant. Do not reset `round`, the
task id, or the plan.

- [ ] **Step 4: Falsify the bound**

Remove the `agent_max_task_tokens.is_some()` guard and confirm test 3 fails
(continuation without a ceiling would make the turn unbounded). Remove the hard
constant and confirm a no-usage mock loops past the intended bound. Restore
both.

- [ ] **Step 5: Show the change and the gate result, and ask before committing**

Suggested subject: `Continue past the round cap under the token budget`

---

### Task 17: R6 acceptance and the honest write-up

**Files:**
- Modify: `crates/workspace-engine/tests/token_ceiling.rs`, `docs/specs/47_agent_working_capability/proposal.md`, `docs/PLAN/OBSERVATIONS.md` (local-only)

- [ ] **Step 1: Cover the acceptance criterion**

"A task exceeding its tool-round budget continues with its state intact, and
the continuation is bounded and appears in the audit log": assert all three
clauses in one test, reading the plan from the session store after the turn and
the continuation from the audit log.

- [ ] **Step 2: Write what was actually decided**

In `proposal.md` §7, record R6 as built on today's evidence (the cliff and the
array), **not** on the opening premise, and state plainly that §7.2 did not
support the premise. Resolve `OBSERVATIONS.md` entry 6 with a disposition:
bounded in this slice by `agent_max_turn_messages`, recording that the in-turn
array and conversation compaction remain separate problems.

- [ ] **Step 3: Show the change and the gate result, and ask before committing**

Suggested subject: `Carry a turn past its round cap under the token budget`

---

### Task 18: Close the spec

**Files:**
- Modify: `docs/specs/47_agent_working_capability/proposal.md`, `docs/specs/47_agent_working_capability/tasks.md`, `docs/specs/README.md`, `docs/USER_GUIDE.md`, `CHANGELOG.md`

- [ ] **Step 1: Fill in the Progress table and header**

Every task's row with what actually landed, the test count, and any deviation;
the `**Started:**` date and, when the last task lands, `— **Done:** …`.

- [ ] **Step 2: Update the four summaries**

`proposal.md` `Status:` (requirements 5, 6 and 8 done; if R6 was dropped, say
so and why), its §7 notes, this file's row/header, and the `docs/specs/README.md`
row 47. If R6 was deferred, `README.md` must say that, not imply it shipped.

- [ ] **Step 3: Docs**

`USER_GUIDE.md`: commands can be stopped and timed out; reads-only rounds may
run together; a turn may continue past the round cap when a token ceiling is
set. `CHANGELOG.md`: Engine rows under `Unreleased`.

- [ ] **Step 4: Full quality gate — all seven commands**

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo nextest run --workspace --locked
node --check crates/desktop-shell/static/app.js
npm run lint:web
typos
cargo deny check
```

Run them once, here, on the finished state. Quote the test count in the
Progress row. If `evals/baseline.json` changed, state who read it.

- [ ] **Step 5: Show the change and the gate result, and ask before committing**

Suggested subject: `Close the agent working capability spec`

---

**Note to the reviewer.** The two decisions most worth a second look before
code: (a) whether the effect-classification refactor in Task 12 satisfies "no
second list" or should instead change `tool_action_marker`'s signature; and
(b) whether R6 ships at all, given §7.2. Both are called out where they occur.
No commit will be made without being asked.
