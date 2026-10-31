# Feature Spec: Flag Only Secrets a Patch Adds

Status: Done 2026-10-31 (specified and built the same day; local checks in §6,
the full gate is left to CI).
Order: 88 of 92
Plan: none. A defect reported in use, like #73–#87: an exception to the rule
in [`README.md`](README.md).
Depends on: [#07](07_generated_secret_override.md), built.

## 1. Motivation

On 2026-10-31 a 62-line patch to `crates/workspace-engine/src/chat.rs` was
held behind "1 file may contain a hardcoded secret — 3 matches
(aws_access_key, generic_api_key)". The patch added no secret. `chat.rs`
already holds fake tokens in its tests (`ghp_abcdefghijklmnopqrstuvwxyz…`),
and #07's check scanned the whole file the patch would write, so every patch
to such a file was flagged. The user reported getting false positives "a
lot".

## 2. Requirements

1. The generated-secret preview and the apply-time block flag only what the
   patch adds: matches in the content to be written, less the matches the
   file already holds.
2. Values are compared per occurrence, so a second copy of a value already in
   the file is still flagged.
3. A new file, or one the patch replaces whole, has nothing to subtract, and
   is scanned as before.

## 3. Non-goals

- The commit flow's own secret scan (#35), which also scans whole files.
  Subtracting what `HEAD` holds would need a read of the committed blob per
  file, and it is left for a later change.
- Changing any detector.

## 4. Design

`PatchEngine::added_secrets(prepared_file)` scans both the current content
and the content to be written, and drops each new match whose value is among
the current file's matches, removing it from that list as it goes. Both
`preview_generated_secrets` and `apply_patch` call it, so the warning and the
block see the same matches.

## 5. Acceptance

`only_secrets_a_patch_adds_are_flagged` (`tests/foundation.rs`):
- a patch that edits another line of a file holding an AWS key gets no
  warning, and applies without the override;
- a patch that adds a second, different key gets one warning.

## 6. Verification

Local checks, 2026-10-31: the fast checks, scoped clippy for workspace-engine
and desktop-shell, and `cargo nextest run -p workspace-engine --locked`. The
workspace clippy and test suite are left to CI.
