# Feature Spec: Session Checkpoints and Rewind

Status: Not started
Order: 16 of 19
Roadmap: `docs/ROADMAP/01_phase_1_trust_and_recovery.md`, Phase 1, Work
Package 1 (Must). That directory is local-only and not committed, so the
reference is a name rather than a link; this spec is self-contained.
Related spec sections: `ai_coding_assistant_specification.md` section 7.7 (Diff
and Patch Engine), section 7.3 (path policy), section 7.10 (secret detection).
Related implementation specs:
[`04_hunk_level_patch_apply.md`](04_hunk_level_patch_apply.md),
[`07_generated_secret_override.md`](07_generated_secret_override.md),
[`08_stop_and_progress.md`](08_stop_and_progress.md),
[`17_durable_task_state_and_crash_recovery.md`](17_durable_task_state_and_crash_recovery.md)
(shares the session event log and its migration).

## 1. Motivation

Damaian can undo an applied patch. It cannot undo a turn.

`PatchEngine::rollback_patch` restores the files one patch touched, and that is
the whole of the recovery story. A user who realises three turns in that the
agent took a wrong direction has no single action that puts the repository and
the conversation back where they were. They can roll back patches one at a time,
in reverse order, if they can work out which patches to roll back — and nothing
restores the conversation position, so the next turn still carries the context
that produced the wrong direction.

Two gaps make the existing rollback insufficient as a foundation:

- **It covers applied patches only.** Files a user changed by approving a
  command are invisible to it. Damaian will happily run an approved
  `npm run codegen` or `cargo fmt` and has no record of what that wrote.
- **Its snapshots are redacted.** `RollbackSnapshot.content` passes through
  secret redaction at capture time, which the type's own doc comment
  (`crates/workspace-engine/src/patch_engine.rs:57-64`) records as making a
  pre-existing secret unrestorable. Rolling back a file that contained a real
  credential writes back a redaction placeholder where the credential was. The
  existing code is honest about this and warns, but a checkpoint feature that
  inherits the behaviour would silently corrupt exactly the files users care
  most about.

Rewind is also the feature that makes the rest of Phase 1 safe to use. Durable
task state ([spec 17](17_durable_task_state_and_crash_recovery.md)) lets a user
resume after a crash; checkpoints are what let them decline to.

## 2. Current State

- **Patch rollback exists and works.** `PatchEngine::apply_patch`
  (`crates/workspace-engine/src/patch_engine.rs:376`) writes a per-file
  `RollbackSnapshot` under `<data_dir>/rollback/<patch_id>/`, with the path
  flattened by replacing `/` with `__` (`patch_engine.rs:425`).
  `PatchEngine::rollback_patch` (`patch_engine.rs:525`) restores them and
  returns a `PatchRollbackResult { patch_id, restored_files, deleted_files,
  warnings }` (`patch_engine.rs:50`).
- **Snapshot content is redacted at capture.** `patch_engine.rs:57-64` documents
  that a genuine secret present before the patch cannot be restored, and
  `patch_engine.rs:632` emits a warning when restored content still contains a
  redaction placeholder.
- **Deleted and created files are already modelled.** `RollbackSnapshot.existed`
  (`patch_engine.rs:70-72`) distinguishes "write this content back" from "delete
  the file the patch created".
- **Conflict detection already exists, twice.** Apply compares the current file
  hash against `base_hash` and refuses to overwrite external work
  (`patch_engine.rs:291`). Rollback compares against `applied_hash` rather than
  `new_hash`, so a partial-hunk apply is still safely reversible
  (`patch_engine.rs:600-611`). `AGENTS.md` makes this a product guarantee.
- **Sessions are a single append-only event log.** `SessionStore`
  (`crates/workspace-engine/src/session.rs:68`) writes one JSONL event per
  action to `session_log_path(session_id)`. Messages are recovered by filtering
  for `"eventType":"message_appended"` (`session.rs:219`), and tasks are
  replayed from `task_created` and `task_status_updated` events rather than
  stored as records (`session.rs:237-260`). Nothing is ever rewritten in place.
- **There is no conversation-position concept.** `read_messages` returns every
  message in the log. Nothing marks a message as superseded, withdrawn, or
  rewound past.
- **Nothing records what a command changed.** `CommandRunner` executes an
  approved command and captures stdout and stderr. No scan of the working tree
  happens before or after, so a command's file effects are unrecorded.
- **Retention has an established pattern.** `audit_retention_days` in
  `crates/workspace-engine/src/config.rs` with cleanup in
  `crates/workspace-engine/src/audit.rs:58-74`.
- **Path policy exists** in `crates/workspace-engine/src/path_policy.rs` and is
  applied to reads and patch targets.
- **The audit log takes arbitrary redacted fields.**
  `AuditLog::record(event_type, fields)` (`crates/workspace-engine/src/audit.rs:42`)
  redacts every field value through `SecretScanner` before writing.

## 3. Requirements

1. A checkpoint is created before every user turn that may change files or
   session state, and is identified by a stable checkpoint ID.
2. Checkpoint metadata records: checkpoint ID; session ID and task ID; creation
   time; the user message it precedes; every in-scope path with its content hash
   and whether it existed; conversation position; pending approvals and task
   status; and a human-readable summary.
3. Checkpoint scope is: files changed by accepted patches; files created,
   modified, or deleted by an approved command, when the path is inside the
   repository and permitted by `path_policy.rs`; and untracked files only when
   Damaian can attribute them to an approved command or accepted patch. Paths
   excluded by policy are recorded as excluded, not snapshotted.
4. Deleted files are represented explicitly, so restore can recreate an
   agent-deleted file and remove an agent-created one without inferring intent
   from absence.
5. Four restore operations are supported: agent-managed file changes only;
   conversation state only; both; and a single tracked file.
6. Restore never overwrites a file changed externally after the checkpoint
   without explicit conflict handling, using the same hash comparison
   `patch_engine.rs:291` already performs.
7. Symlinks are not followed outside the repository, and `path_policy.rs` is
   applied to every snapshot and restore path.
8. Every restore states exactly which files were restored, skipped, or
   conflicted. Best-effort behaviour is reported, never silent.
9. Checkpoint content is stored faithfully. A file containing a real credential
   is restored byte-identical, or the checkpoint explicitly declines to cover it
   — never restored with a redaction placeholder in place of the secret.
10. Rewinding conversation state does not destroy the session log. History
    remains auditable after a rewind.
11. Checkpoints are session recovery, not version control, and the UI says so
    where a user could otherwise assume Git-like guarantees.
12. Retention is configurable with safe cleanup, following the
    `audit_retention_days` pattern.
13. Create, restore, conflict, skip, and cleanup events are recorded through
    `AuditLog::record`.
14. Existing `<data_dir>/rollback/<patch_id>/` snapshots keep working, or are
    migrated.
15. Checkpoint storage for a 100-turn session on a mid-sized repository stays
    within a documented bound.

## 4. Non-goals

- Replacing or wrapping the user's Git history. Damaian never writes to the
  user's index, `HEAD`, refs, reflog, or stash.
- Restoring files outside the selected repository, or outside `path_policy.rs`.
- Undoing effects that are not files: pushed commits, network calls, database
  writes, container state, or anything a command did outside the repository.
- Undoing a command's side effects on files it changed *outside* the repository
  root, even when Damaian can see them.
- Branching or forking a conversation into alternative histories. Rewind moves
  one conversation backwards; alternative implementations are Phase 6.
- A visual timeline or diff-between-checkpoints browser. This spec provides the
  store, the metadata, and the restore operations, and one list view.
- Cross-machine or remote checkpoint sync.
- Deduplicating against the user's existing Git objects.

## 5. Design

### 5.1 Store: shadow Git object store

Write checkpoint content as dangling Git objects in a Damaian-owned object
store, the way `git stash create` builds a commit without moving anything:

```text
<data_dir>/checkpoints/<repository_id>/objects/    # GIT_DIR for snapshots
<data_dir>/checkpoints/<repository_id>/manifests/<checkpoint_id>.json
```

Snapshot writes run Git with `GIT_DIR` pointed at the Damaian store and
`--work-tree` at the user's repository, so blobs and trees land in Damaian's
store and the user's `.git` is never touched, read for state, or locked. Content
addressing, deduplication across turns, packing, and `git gc` come for free,
which is what makes requirement 15 achievable: 100 turns that each touch three
files store three blobs per turn and share everything unchanged.

The alternative — extending `<data_dir>/rollback/` into a SHA-256 content store
with manifests — is the roadmap's fallback. Choose it only if the shadow-Git
approach proves impractical, and record the reason in §7. Two things make it
worse rather than merely different: deduplication, packing, and garbage
collection all have to be written and tested, and the flattened
`path.replace('/', "__")` naming that `patch_engine.rs:425` uses collides for
paths that differ only by a separator (`a/b` and `a__b`), which is tolerable for
one patch's files and not for a whole-session store.

Damaian requires Git already (`docs/MACOS_INSTALLATION.md` system
requirements), so this adds no dependency.

### 5.2 Content is stored unredacted, and that is a deliberate boundary

Requirement 9 conflicts with how `RollbackSnapshot` behaves today, so state the
rule plainly: **the checkpoint store holds the user's file bytes as they are.**

This is not a weakening of the secret-redaction guarantee. `AGENTS.md` scopes
redaction to what leaves the user's control — model context, command output,
diffs, and the audit log. A checkpoint is a local copy of a file the user
already has, made so it can be put back. Redacting it does not protect the
secret; it destroys the user's file on restore, which is the failure mode
requirement 9 exists to prevent.

The obligations that follow are explicit, and each is a test:

- The checkpoint store is never read into model context, never included in a
  patch, never attached to diagnostic output ([spec 15](15_install_and_update_verification.md)
  and Phase 1 WP5), and never uploaded.
- Manifests carry paths, hashes, and counts — never content. Only the object
  store holds bytes, and only restore reads it.
- Audit fields are paths and hashes, which already pass through
  `SecretScanner` on the way into `AuditLog::record`.
- The store lives under `<data_dir>`, mode `0700`, alongside the sessions and
  audit data that are already local-only.
- `docs/TROUBLESHOOTING.md`'s "safe to share" guidance names the checkpoint
  store as **not** safe to share, next to the existing note about session files
  containing prompts and file content.

Because this changes a documented behaviour, `patch_engine.rs`'s redaction of
`RollbackSnapshot.content` is reconsidered in the same change: patch rollback
moves to the same faithful-capture rule, and the
"contained secrets that were redacted before rollback capture"
warning at `patch_engine.rs:632` is removed along with the cause. Leaving two
capture paths with opposite secret behaviour is worse than either rule applied
consistently.

### 5.3 Manifest

```json
{
  "checkpointId": "checkpoint_...",
  "sessionId": "session_...",
  "taskId": "task_...",
  "createdAtMs": 1788000000000,
  "userMessageId": "msg_...",
  "summary": "Before: add retry to the upload client",
  "conversation": { "lastEventSeq": 412, "taskStatus": "waiting_for_approval" },
  "pendingApprovals": [{ "kind": "command", "proposalId": "cmdprop_..." }],
  "treeOid": "…",
  "files": [
    { "path": "src/upload.rs", "hash": "…", "existed": true, "oid": "…",
      "origin": "patch" },
    { "path": "src/generated.rs", "hash": null, "existed": false,
      "origin": "command" }
  ],
  "excluded": [{ "path": ".env", "reason": "restricted_pattern" }]
}
```

`existed: false` with a null hash is the create-then-delete case, carried over
from `RollbackSnapshot.existed` rather than invented. `origin` is what makes
requirement 3's attribution auditable: a path is in scope because a patch or a
command put it there, and the manifest says which.

`excluded` is recorded rather than dropped, so a user who wonders why `.env` was
not restored gets an answer instead of silence.

### 5.4 Determining what a command changed

Requirement 3's command coverage needs information nothing currently collects.
Take a working-tree census immediately before and after each approved command:

```sh
git status --porcelain=v1 --untracked-files=all
```

Run it through the same shadow `GIT_DIR`, hash the paths it reports as modified
or untracked, and diff the two censuses. Paths that appear, disappear, or change
hash are the command's file effects, recorded with `origin: "command"`.

Bounds, because this runs on every approved command:

- Only paths inside the repository root and permitted by `path_policy.rs` are
  hashed. Everything else is not censused at all.
- The census records paths and hashes, not content. Content is snapshotted only
  for paths that actually changed.
- A repository whose census exceeds a configured path ceiling records
  `origin: "command_uncensused"` for the turn and reports in the UI that command
  effects are not covered for this repository, rather than silently covering
  some of them. A checkpoint that claims coverage it does not have is worse than
  one that admits the gap.

This is honest about a real limit: a command that changes a file *and* a user
who edits a file during the same command produce one census diff, and Damaian
cannot separate them. Attribution is best-effort and labelled as such, which is
why requirement 3 restricts untracked-file coverage to attributable paths and
acceptance requires that an unattributed generated file is never silently
removed.

### 5.5 Conversation rewind is an append, not a truncation

The session log is append-only and tasks are replayed from it
(`session.rs:237`). Deleting or rewriting lines to rewind would destroy the
audit trail and break replay, so rewind appends:

```json
{"eventType":"conversation_rewound","checkpointId":"checkpoint_...",
 "throughEventSeq":412,"restoredAtMs":…}
```

`read_messages` and `read_task_statuses` gain a replay rule: events after the
newest `conversation_rewound`'s `throughEventSeq` are inert — they stay in the
log, are still auditable, and are not part of the active conversation. A later
rewind to an earlier point supersedes an earlier one, so the newest marker wins.

This requires the log to have a per-event sequence number, which it does not
have today. Add a monotonic `seq` to every appended event.
[Spec 17](17_durable_task_state_and_crash_recovery.md) needs the same field for
crash classification, so the two specs share one migration: events without
`seq` are numbered by line order on read, which is exactly their append order,
so existing sessions need no rewrite.

Rewinding conversation state alone leaves files untouched, and vice versa —
requirement 5's four operations are two independent switches, not four code
paths.

### 5.6 Restore and conflicts

```rust
pub struct CheckpointRestoreResult {
    pub checkpoint_id: String,
    pub restored_files: Vec<String>,
    pub deleted_files: Vec<String>,
    pub skipped_files: Vec<String>,
    pub conflicted_files: Vec<String>,
    pub conversation_restored: bool,
    pub warnings: Vec<String>,
}
```

Deliberately shaped like `PatchRollbackResult` (`patch_engine.rs:50`), with
`skipped` and `conflicted` split apart — the existing type folds both into
`warnings`, which makes "the file was changed under you" indistinguishable from
"there was nothing to do".

Per file, restore compares the file's current hash against the manifest hash:

| Current state | Action |
|---|---|
| Matches manifest hash | Restore, or delete when `existed: false` |
| File absent, manifest says it existed | Restore |
| Differs from manifest hash | Conflict. Do not write. Report the path |
| Absent, manifest says it did not exist | Nothing to do. Skipped |
| Excluded by policy at restore time | Skipped, with the policy reason |

Conflicts are reported as a set before anything is written, and restore is
all-or-nothing per invocation for the non-conflicting files: partial writes with
a conflict in the middle leave a state that is neither the checkpoint nor the
current tree. The user's choices are to restore the non-conflicting files
explicitly, restore a single file, or resolve the conflict by hand.

### 5.7 Retention and cleanup

`checkpoint_retention_days` in `Config`, defaulting to the same value as
`audit_retention_days`, plus a `checkpoint_max_total_bytes` ceiling. Cleanup
drops manifests older than the retention window, then runs `git gc --prune` in
the shadow store so orphaned blobs go with them.

Cleanup never deletes the newest checkpoint of a session that is still active,
regardless of age — the one a user is most likely to want is the one a
time-based rule would remove during a long session.

### 5.8 Migration of existing rollbacks

`<data_dir>/rollback/<patch_id>/` stays readable and `rollback_patch` keeps
working unchanged; existing snapshots are not converted. New applies write both
the existing rollback snapshot and a checkpoint entry until a later version
retires the former. Requirement 14 is satisfied by keeping the old path working,
which is cheaper and lower-risk than a migration that must handle the flattened
`__` path collisions described in §5.1.

### 5.9 UI

A `Rewind` control on each user turn, offering: files only, conversation only,
or both. The confirmation names the file count and the turn, lists conflicts if
any, and carries one line stating that checkpoints cover Damaian's own changes
to this repository and are not a substitute for Git or for a commit.

The checkpoint list view shows time, summary, file count, and whether a
checkpoint has been restored from.

### 5.10 Documentation

`docs/USER_GUIDE.md`: what rewind does and does not cover, and that it is not
version control. `docs/TROUBLESHOOTING.md`: where checkpoints live, how to read
a manifest, why a file was reported as conflicted, and that the checkpoint store
is not safe to share.

## 6. Acceptance Criteria

- Rewinding a turn restores agent-managed file changes and conversation position
  independently or together.
- A file the user edited after the checkpoint is reported as conflicted and is
  not overwritten.
- Restoring a single tracked file leaves every other file in the checkpoint
  untouched.
- Files created, modified, or deleted by an approved command are restored when
  they were inside allowed repository scope and recorded in the manifest.
- An untracked generated file with no recorded agent action is not removed by
  rewind.
- A file containing a real credential is restored byte-identical, with no
  redaction placeholder — asserted by test with a seeded fake secret.
- A repository too large to census reports that command effects are not covered,
  rather than reporting partial coverage.
- Rewinding conversation state leaves the session log intact and still
  auditable, and `read_messages` reflects the rewound position.
- A second rewind to an earlier point supersedes the first.
- Existing `<data_dir>/rollback/<patch_id>/` snapshots still roll back after the
  change.
- Sessions written before the `seq` migration load and replay correctly.
- Checkpoint storage for a 100-turn session on a mid-sized fixture repository
  stays within the documented bound, recorded in §7.
- Create, restore, conflict, skip, and cleanup events appear in the audit log,
  with no file content in any field.
- Cleanup respects the retention window and never removes the newest checkpoint
  of an active session.
- `path_policy.rs` is applied to every snapshot and restore path, and a symlink
  pointing outside the repository is not followed — asserted by test.
- The five quality-gate commands from `AGENTS.md` pass.

## 7. Implementation Notes

To be completed during implementation. Record:

- Whether the shadow-Git store was used, or the SHA-256 fallback and why.
- The measured storage bound for the 100-turn fixture, and the repository size
  it was measured against.
- The census path ceiling chosen for §5.4, and the measured cost of a census on
  a mid-sized repository.
- Whether patch rollback's capture was switched to faithful content in this
  change, per §5.2, or deferred with a reason.
