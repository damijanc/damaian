# Feature Spec: Redaction Markers Never Reach a Patch

Status: Done 2026-10-31 (specified and built the same day; local checks in §6,
the full gate is left to CI).
Order: 84 of 84
Plan: none. A defect reported in use, like #73–#83: an exception to the rule
in [`README.md`](README.md).
Depends on: [#07](07_generated_secret_override.md), built.

## 1. Motivation

On 2026-10-30 a worker session wrote the brace-glob change for #47 §7.4. Its
new test in `crates/workspace-engine/tests/agent_tools.rs` held this line:

```
let mut on_token = [REDACTED_CREDENTIAL_ASSIGNMENT_<digest>] &str| {};
```

The line it was copied from reads `let mut on_token = |_token: &str| {};`.
`scan_credential_assignments` saw the keyword `token`, a `=`, and an unquoted
eight-character value `|_token:`. It took that for a credential, so the model
read every such line with a marker in its place. The model then copied the
marker into its patch, the patch was applied, and the file no longer
compiled. None of the change's tests could run.

Two things went wrong:

1. **The detector was wrong.** A Rust closure is code, not a credential. The
   runtime-expression exemption (#07 §4.1) already covers values with `.`,
   `(`, `[`, `::` or `->`, but not a value that starts with `|`.
2. **Nothing stopped a marker from being written.** The model only ever sees
   redacted text, so wherever it saw a marker the file holds other text.
   Writing the marker back replaces that text. A false positive costs a
   broken line. A real secret, such as a key in a config file the user asked
   the model to edit, would be silently replaced, and the user loses it.

## 2. Requirements

1. An unquoted credential-assignment value that starts with `|` (a closure)
   or `&` (a borrow) is a runtime expression and is not flagged. Quoted
   values are still flagged as before.
2. A patch is refused when the new content of a file holds a redaction
   marker, `[REDACTED_<CATEGORY>_<10 hex digits>]`, that the file's current
   content does not. Markers are counted per occurrence, so a file that
   already holds one marker (a doc showing what redaction looks like) may
   keep it, but a second copy is refused.
3. The refusal names the file and line, and tells the model what happened and
   how to proceed: leave the lines that held the marker unchanged, or write
   the line without the hidden text. In the agent loop it comes back as the
   tool's result (as any `create_patch` error does), so the model can retry
   in the same turn.
4. The refusal is checked when the patch is created, before anything is
   stored or shown, so no review card ever shows a patch that would write a
   marker.

## 3. Non-goals

- Showing the model the hidden text. Redaction stays a guarantee (#07,
  `SECURITY.md`).
- A user override. Unlike a generated secret (#07), a marker is never what
  the user wants written: it is neither the secret nor code.
- Repairing files that already hold a copied marker. The guard stops new
  ones. An existing one is fixed by hand, as the brace-glob test line is.

## 4. Design

- **`secret_scanner.rs`:**
  - `redaction_placeholders(text)` returns every marker in `text`, matching
    exactly the shape `add_finding` writes.
  - `is_runtime_expression_value` also accepts a value that starts with `|`
    or `&`.
- **`patch_engine.rs`:** `create_patch` calls
  `refuse_copied_placeholders(path, old, new)` for each change right after
  reading the current file. It returns `ClientError::InvalidInput`. The guard
  covers every caller of `create_patch`:
  - the agent loop's `propose_patch`;
  - its `edit_file`, whose region edits become whole-file changes first;
  - the one-shot `propose_edit`.

## 5. Acceptance

- `ignores_runtime_credential_assignments_in_source_code` covers the closure
  and the borrow (`tests/foundation.rs`).
- `refuses_a_patch_that_copies_a_redaction_placeholder_into_a_file`: a
  patch built from the redacted view of a file that holds a real AWS key is
  refused, names line 1, and leaves the file unchanged.
- `keeps_a_redaction_placeholder_the_file_already_holds`: a marker the file
  already holds may stay, and a second copy is refused on its line.

## 6. Verification

Local checks, 2026-10-31:
- `scripts/quality-gate.sh fmt node lint typos deny`;
- `cargo clippy -p workspace-engine --all-targets --locked -- -D warnings`;
- `cargo nextest run -p workspace-engine --locked`.

The workspace clippy and test suite are left to CI.
