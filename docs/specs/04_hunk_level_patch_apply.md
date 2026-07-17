# Feature Spec: CLI Parity for Hunk-Level Patch Apply

Status: Not started
Order: 4 of 5
Related spec sections: `ai_coding_assistant_specification.md` §7.7 (Diff and Patch Engine — "Support hunk-level acceptance when feasible"), §13 (MVP Release Scope — "Basic hunk-level support if implementation cost is acceptable").

## 1. Correction to initial gap analysis

The initial codebase survey for this feature set assumed hunk-level patch apply was entirely missing. That was **wrong for the desktop app** — it is already fully implemented there:

- `PatchEngine::apply_patch` (`crates/workspace-engine/src/patch_engine.rs:200`) takes a `hunk_selection: Option<&HashMap<String, Vec<String>>>` parameter and reconstructs per-file content from only the accepted hunk IDs via `reconstruct_content` (`patch_engine.rs:260-266`, backed by `Hunk`/`diff_file` in `crates/workspace-engine/src/diff.rs`).
- The desktop UI renders per-hunk checkboxes (`crates/desktop-shell/static/app.js:2103-2126`) and sends the selection as a JSON map to `/api/apply-patch` (`app.js:2310-2323`).
- The desktop server route parses it (`crates/desktop-shell/src/lib.rs:473-492`, `parse_hunk_selection` at `lib.rs:1474`) and passes it straight into `apply_patch`.
- Rollback already accounts for partial-hunk applies specifically (`RollbackSnapshot.applied_hash`, `patch_engine.rs:65-70`, comment: "Rollback's conflict check compares against this instead of `new_hash` so partial-hunk applies can still be safely rolled back").

So this spec is much narrower than originally scoped. The real, remaining gaps are:

1. **The CLI has no hunk-level apply.** `damaian apply-patch <repo> <patch-id> [file...]` (`crates/damaian-cli/src/main.rs:307-322`) only supports per-file selection (`approved_paths`) and always passes `None` for `hunk_selection` — there is no way to accept a subset of hunks within a file from the CLI.
2. **Rejection is whole-patch or whole-file only**, never per-hunk. `PatchStore::mark_rejected` (`edit.rs:59`) and `EditOrchestrator::reject_stored_patch_files` (`edit.rs:350`) record rejection at the patch or file level; there is no audit record of *which specific hunks* were excluded when a patch is applied with a partial hunk selection — that information exists implicitly (whatever wasn't in `hunk_selection`) but isn't recorded as an explicit "hunk rejected" event the way file rejection is.

## 2. Goals

- Add hunk-level selection to the CLI's `apply-patch` command, so scripted/terminal workflows have the same granularity as the desktop UI.
- Record which hunks were excluded from a partial-hunk apply as an explicit audit event (mirroring the existing `stored_patch_files_rejected` event at `edit.rs:360-369`), so the audit trail (§7.11) fully reflects what was and wasn't applied, not just what was.

## 3. Non-Goals

- Rebuilding hunk computation, reconstruction, or rollback — all already correct and reused as-is.
- Changing the desktop UI's existing hunk-selection interaction.
- Nested/sub-hunk (single-line) granularity — the spec only asks for hunk-level ("basic hunk-level support," §13), which is what already exists.

## 4. Design

### 4.1 CLI command surface

Extend `apply-patch` argument parsing (`main.rs:307`) to accept an optional hunk-selection argument, e.g.:

```
damaian apply-patch <repo> <patch-id> [--file <path>]... [--hunks <path>=<hunk-id>,<hunk-id>,...]...
```

or, simpler and consistent with how the desktop server already accepts it as a JSON blob (`parse_hunk_selection`, `lib.rs:1474`), accept a `--hunk-selection <json>` flag taking the same `{path: [hunkId, ...]}` shape already used over HTTP, so both entry points share one parsing function. Recommend factoring `parse_hunk_selection` out of `desktop-shell/src/lib.rs` and into `workspace-engine` (e.g. alongside `PatchEngine`) so the CLI can reuse it directly instead of duplicating JSON-parsing logic in a second crate.

A prerequisite for either syntax: the CLI needs a way to show hunk IDs to the user before selection, since right now `patch_diff_text` (`main.rs:295`) presumably prints the unified diff without exposing per-hunk IDs. Add a CLI diff view (or extend the existing one) that prints each hunk's `id` alongside its `@@ ... @@` header, matching what `Hunk.id` already carries (`diff.rs`).

### 4.2 Audit event for excluded hunks

In `PatchEngine::apply_patch` (`patch_engine.rs:200`), when `hunk_selection` is `Some(...)` and a file's accepted hunk IDs are a strict subset of `file.hunks`, record an audit event (e.g. `patch_hunks_rejected`) listing the excluded hunk IDs per file, alongside the existing per-apply audit trail — consistent with how file-level rejection is already audited (`edit.rs:360-369`).

## 5. Acceptance Criteria

- `damaian apply-patch <repo> <patch-id>` with a hunk-selection argument applies only the specified hunks per file, leaving the rest of that file's content untouched relative to its pre-patch state — verified by diffing the file after apply.
- The resulting on-disk content matches exactly what the desktop UI would produce for the same hunk selection (both paths call the same `apply_patch`/`reconstruct_content`, so this should hold by construction once the CLI passes the selection through correctly).
- Rollback after a CLI-driven partial-hunk apply restores the file to its pre-patch state, same as it already does for desktop-driven partial applies.
- Applying a patch with some hunks excluded produces an audit log entry listing which hunks were excluded, for both the CLI and desktop paths.

## 6. Open Questions / Decisions Needed

- Preferred CLI syntax for specifying hunk selection (repeated `--hunks path=ids` flags vs a single `--hunk-selection <json>` blob) — recommend the JSON form for exact parity with the existing desktop wire format and to avoid inventing a second grammar.
- Whether the "excluded hunks" audit event should fire only for CLI-driven applies (to close the gap) or be added uniformly for both entry points — recommend uniformly, since the desktop path has the same silent gap today.
