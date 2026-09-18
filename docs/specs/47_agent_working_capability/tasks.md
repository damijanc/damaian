# Agent Working Capability Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Implements:** [`proposal.md`](proposal.md) §5, requirements 1–4 · background and
corrections in [`context.md`](context.md)
**Started:** 2026-09-18

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
| 3 · `list_directory` | Not started | |
| 4 · `search_content` | Not started | |
| 5 · `edit_file` splice | Not started | |
| 6 · Wire the four tools | Not started | |
| 7 · Acceptance criteria, docs, close the slice | Not started | |

**Baseline measured 2026-09-17:** `cargo nextest run --workspace --locked` is
**652 passed, 16 skipped**, and takes about five minutes locally — see
`OBSERVATIONS.md` entry 8 for why, and the Global Constraints for what that means
for this plan.

The running check per task is the count in `tests/agent_tools.rs` — 1, 5, 8, 13,
18, 20, 23 as Tasks 1–7 land. Task 7 verifies the whole workspace once, at
**675**. A task whose file count comes out wrong has a missing or duplicated
test; find it then rather than at the end.

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

- [ ] **Step 1: Write the failing tests**

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

- [ ] **Step 2: Run to verify they fail**

Run: `cargo nextest run -p workspace-engine --test agent_tools`
Expected: FAIL to compile — `NavigationController` does not exist.

- [ ] **Step 3: Add the config key**

`max_list_entries: usize`, default `200`, restrict-only, same six places as
Task 2 Step 3.

- [ ] **Step 4: Write `navigation.rs`**

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

- [ ] **Step 5: Run the new tests**

Run: `cargo nextest run -p workspace-engine --test agent_tools`
Expected: PASS, 8 tests in this file.

- [ ] **Step 6: Targeted tests, fmt and clippy**

Run: `cargo nextest run -p workspace-engine --test agent_tools` — 8 tests, all
passing. Then `cargo fmt --all -- --check` and
`cargo clippy --workspace --all-targets --locked -- -D warnings`.

- [ ] **Step 7: Show the change and the gate result, and ask before committing**

Suggested subject: `List repository paths as a tool rather than a shell command`

---

### Task 4: `search_content`

**Files:**
- Modify: `crates/workspace-engine/src/navigation.rs`, `crates/workspace-engine/Cargo.toml`, `crates/workspace-engine/src/config.rs`
- Test: `crates/workspace-engine/tests/agent_tools.rs`

**Interfaces:**
- Consumes: `NavigationController` from Task 3.
- Produces: `search_content(root, pattern: &str, path_glob: Option<&str>, max_matches: Option<usize>, task_id, repository_id) -> Result<ContentSearch>`; `ContentSearch { matches: Vec<ContentMatch>, total_found: usize, files_searched: usize, truncated: bool }`; `ContentMatch { path: String, line: usize, text: String }`.

- [ ] **Step 1: Write the failing tests**

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

- [ ] **Step 2: Run to verify they fail**

Run: `cargo nextest run -p workspace-engine --test agent_tools`
Expected: FAIL to compile — no `search_content`.

- [ ] **Step 3: Add the dependency and the config keys**

`crates/workspace-engine/Cargo.toml`, in alphabetical position:

```toml
regex = "1"
```

Run `cargo tree -p workspace-engine -i regex` first to confirm the version
already resolved in `Cargo.lock`, and pin to that major. No network fetch should
occur; if `cargo` wants to update the lockfile, stop and raise it.

`max_search_matches: usize` (default `50`) and `max_match_line_chars: usize`
(default `500`), both restrict-only, same six places as Task 2 Step 3.

- [ ] **Step 4: Implement `search_content`**

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

- [ ] **Step 5: Run the new tests**

Run: `cargo nextest run -p workspace-engine --test agent_tools`
Expected: PASS, 13 tests in this file.

- [ ] **Step 6: Mutation-test the redaction**

Replace `self.scanner.redact(line).text` with `line.to_string()` and re-run.
`search_redacts_a_secret_it_would_otherwise_return` must fail. Restore it.

- [ ] **Step 7: Targeted tests, fmt and clippy**

Run: `cargo nextest run -p workspace-engine --test agent_tools` — 13 tests, all
passing. Then `cargo fmt --all -- --check`,
`cargo clippy --workspace --all-targets --locked -- -D warnings`, and
**`cargo deny check`** — run that one here rather than deferring it, because this
is the task that adds a dependency and it is the check that confirms `regex`'s
license is already on the allow-list.

- [ ] **Step 8: Show the change and the gate result, and ask before committing**

Suggested subject: `Search file contents as a tool, capped and redacted`

---

### Task 5: `edit_file` — the anchored splice

**Files:**
- Modify: `crates/workspace-engine/src/edit.rs`
- Test: `crates/workspace-engine/tests/agent_tools.rs`

**Interfaces:**
- Consumes: nothing from Tasks 1–4.
- Produces: `RegionEdit { path: String, old_text: String, new_text: String }`; `region_edits_to_changes(root: &Path, path_policy: &PathPolicy, edits: &[RegionEdit]) -> Result<Vec<ProposedChange>>`.

- [ ] **Step 1: Write the failing tests**

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

- [ ] **Step 2: Run to verify they fail**

Run: `cargo nextest run -p workspace-engine --test agent_tools`
Expected: FAIL to compile — `RegionEdit` does not exist.

- [ ] **Step 3: Implement the splice**

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

- [ ] **Step 4: Run the new tests**

Run: `cargo nextest run -p workspace-engine --test agent_tools`
Expected: PASS, 18 tests in this file.

- [ ] **Step 5: Mutation-test the fail-closed rule**

Change `if count > 1` to `if count > 2` and re-run.
`an_anchor_matching_twice_is_refused_and_names_the_count` must fail. Restore it.
This is the assertion the whole design rests on; a test that passes either way
is worse than none.

- [ ] **Step 6: Targeted tests, fmt and clippy**

Run: `cargo nextest run -p workspace-engine --test agent_tools` — 18 tests, all
passing. Then `cargo fmt --all -- --check` and
`cargo clippy --workspace --all-targets --locked -- -D warnings`.

- [ ] **Step 7: Show the change and the gate result, and ask before committing**

Suggested subject: `Propose an edit as an anchored region instead of a whole file`

---

### Task 6: Wire the four tools into the turn

**Files:**
- Modify: `crates/workspace-engine/src/chat.rs:1231-1238,2033-2055,2771-2798,2807-2830,2920,3088-3099`
- Test: `crates/workspace-engine/tests/agent_tools.rs`

**Interfaces:**
- Consumes: Tasks 2–5.
- Produces: tool names `read_file` (extended), `list_directory`, `search_content`, `edit_file`.

- [ ] **Step 1: Write the failing test**

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

- [ ] **Step 2: Run to verify they fail**

Run: `cargo nextest run -p workspace-engine --test agent_tools`
Expected: FAIL — `list_directory` is not a known tool, so the turn does not
dispatch it.

- [ ] **Step 3: Add the tool definitions**

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

- [ ] **Step 4: Add the enum variants, decode arms and marker arms**

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

- [ ] **Step 5: Add the dispatch arms**

Beside `ToolAction::ReadFile` (`:2033`). Each mirrors the existing arm's
`(label, content, outcome)` triple. The content strings are what the model
reads, so they carry the notices:

- `ListDirectory` → `"Listing of {dir} ({shown} of {total} paths):\n{paths}"`, and when `truncated` is false, `"Listing of {dir} ({total} paths):\n{paths}"`.
- `SearchContent` → `"{total} matches for \"{pattern}\" across {files} files (showing {shown}):\n{path}:{line}: {text}"`.
- `ReadFile` → `"Content of {path}, lines {start}–{end} of {total}:\n{content}"`, plus `" (truncated by {lines|bytes}; ask for a narrower range)"` when `truncated_by` is set.
- `EditFile` → `region_edits_to_changes`, then the **same** `PatchEngine::create_patch` call the `ProposePatch` arm makes. Reuse that arm's body rather than writing a second one; if the two bodies differ, requirement 4's promise is broken. A refusal returns `ActionOutcome::Failed` with the error text, so the model can retry within the remaining rounds.

- [ ] **Step 6: Run the new tests**

Run: `cargo nextest run -p workspace-engine --test agent_tools`
Expected: PASS, 20 tests in this file.

- [ ] **Step 7: Targeted tests, fmt and clippy**

Run: `cargo nextest run -p workspace-engine --test agent_tools` — 20 tests, all
passing. Then `cargo fmt --all -- --check` and
`cargo clippy --workspace --all-targets --locked -- -D warnings`.

- [ ] **Step 8: Show the change and the gate result, and ask before committing**

Suggested subject: `Offer ranged reads, listing, search and anchored edits as tools`

---

### Task 7: Acceptance criteria, documentation, and closing the slice

**Files:**
- Modify: `crates/workspace-engine/tests/agent_tools.rs`, `docs/USER_GUIDE.md`, `docs/TROUBLESHOOTING.md`, `CHANGELOG.md`, `docs/specs/47_agent_working_capability/proposal.md`, `docs/specs/47_agent_working_capability/tasks.md`, `docs/specs/README.md`

- [ ] **Step 1: Write the equivalence test**

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

- [ ] **Step 2: Write the truncation-notice test**

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

- [ ] **Step 3: Write the audit test**

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

- [ ] **Step 4: Run the full suite**

Run: `cargo nextest run --workspace --locked` — expected **675**. This is the
**one** full-suite run of the slice, so allow about five minutes for it and use a
`timeout` of at least 600000. Every earlier task deferred to here.

- [ ] **Step 5: Documentation**

- `docs/USER_GUIDE.md`: a short section naming the four tools and saying plainly
  that none of them can write to the repository or run a command, and that an
  edit still goes through the same approval as before.
- `docs/TROUBLESHOOTING.md`: the two refusals a user will see quoted back by the
  model — "old_text did not match" (the file changed since it was read; ask it
  to re-read) and "matched N times" (the anchor was too short) — plus what a
  truncation notice means and which config key raises the cap.
- `CHANGELOG.md`: a row under **Unreleased**, component **Engine**.

- [ ] **Step 6: Close the slice in all four places**

The rule in `AGENTS.md`, "Before you change a feature":

1. `proposal.md` `Status:` — requirements 1–4 Done, 5/6/8 outstanding.
2. `proposal.md` §7 Implementation Notes — what implementing found, especially
   anything that contradicted §5. Write it as spec 19's §7 is written: the
   corrections, not a summary.
3. This file's Progress table and its `**Started:** … — **Done:** …` header.
4. The `docs/specs/README.md` row.

- [ ] **Step 7: Full quality gate**

All seven commands.

- [ ] **Step 8: Show the change and the gate result, and ask before committing**

Suggested subject: `Document the agent tool floor and close the first slice`
