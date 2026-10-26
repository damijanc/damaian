# Context: Memory Model and Storage

Background for [`tasks.md`](tasks.md). Read this before Task 1, with
[`proposal.md`](proposal.md) §5 beside it. Everything below was checked against
the tree on 2026-10-26, after #26, #27 and #66 landed. Line numbers drift:
re-read the function before editing near it.

## 1. What exists, and what does not

No memory exists. There is no `memory` module, no store, no record type, no
audit event, and no config key. `WorkspaceEngine` (`workspace_engine.rs:38`)
holds no memory field. The proposal's record is entirely new code.

What #28 can build on is real and was verified:

- `SecretScanner::scan` (`secret_scanner.rs:27`) and
  `contains_secrets` (`secret_scanner.rs:65`) return findings without
  transforming; only `redact` (`secret_scanner.rs:42`) transforms. The API
  already supports refusal.
- `AuditLog::record` (`audit.rs:42`) takes `&[( &str, String )]`, redacts every
  value on the way in, and is disabled cleanly when `audit_enabled` is false.
- `SessionStore` (`session.rs:460`) is a cloneable handle over
  `<data_dir>/sessions/<session_id>.jsonl`, with append-only events replayed by
  `active_events` (`session.rs:3276`). `append_session_event`
  (`session.rs:3071`) is **private** to the module.
- `PathPolicy::is_restricted` (`path_policy.rs:138`) applies
  `restricted_patterns`; `is_ignored_by_rules` (`ignore.rs:44`) applies parsed
  ignore rules; `tree_walk.rs:57-64` shows how per-directory `.gitignore` files
  are accumulated (`parse_ignore_patterns` with the directory as base).
- The repository-map store (`repository_map.rs:827`) is the closest precedent
  for a per-repository file under the data directory, with a schema version
  read before the shape and `write_replacing` for atomic replacement. Memory is
  JSONL and appends instead, so it borrows the versioning pattern, not the
  writer.
- `data_schema.rs` establishes the flat `key=value` marker, the
  older-migrates/equal-proceeds/newer-refuses rule, and the refusal wording.

## 2. Corrections to the proposal's line references

The proposal was written against an earlier tree. Every reference below was
re-checked; use the current location.

| Proposal says | Current location | Note |
|---|---|---|
| `SessionStore` `session.rs:68` | `session.rs:460` | The struct moved behind the event/record types. |
| `repository_id_for_root` `indexer.rs:382-385` | `hash.rs:113-118` | Moved out of the indexer; still `sha256(canonicalized path)[..16]` prefixed `repo_`. |
| `repository_id_for_path` `indexer.rs:191-194` | `indexer.rs:215-218` | Now canonicalizes then calls `repository_id_for_root`. |
| `DAMAIAN_DATA_DIR` `config.rs:192` | `config.rs:930-934` | `Config::default_data_dir`. |
| vector-index path `vector_index.rs:149-152` | `vector_index.rs:149-152` is `semantic_search`; the path helper is `vector_index.rs:144-148` | Still no schema version. |
| `git_service.rs:37`, `:77`, `:110` (status, diff, suggest) | `:326` (`status`), `:507` (`diff`), `:999` (`suggest_commit_message`) | The module is no longer narrow: it now has worktree, log, commit and summary reads. There is still **no** repository-identity helper. |
| `path_policy.rs:138` | unchanged | `is_restricted`. |
| `ignore.rs:44` | unchanged | `is_ignored_by_rules`. |
| `patch_engine.rs:291` for hash comparison | `patch_engine.rs:334-335` | `sha256(current) != file.base_hash`; the mechanism is the same. |
| `session.rs:261-291` for session-scoped approval | `session.rs:1077-1108` | `allow_browser_diagnostics_for_session` / `browser_diagnostics_allowed_for_session`. |
| `context_manager.rs:260` redaction | `context_manager.rs:2252` (and `:1298`) | Redaction happens on the way into the item/request, not at the line the proposal names. |

The substantive corrections in the proposal still hold: `repository_id` is a
path hash and cannot satisfy requirement 2, so memory needs a separate
`project_key`; and the secret path must refuse rather than redact.

## 3. What #28 must not touch

- **`chat.rs`.** #30 owns retrieval and the turn path. #28 adds a store and
  record types; nothing in the chat orchestrator changes. If a change appears
  necessary, stop and report rather than starting a parallel `chat.rs` track.
- **`context_manager.rs` / `ContextCategory`.** #30 adds the `Memory` category
  and the `ContextItem` rendering. #28 must not add a category that has no
  producer, and must not deliver memory around assembly.
- **`repository_id` and every store keyed by it.** The keyword index, vector
  index, sessions, checkpoints, repository map, patches and file access all use
  it. `project_key` is additional, not a replacement.

## 4. Contracts from the neighbours

- **#26 §5.5:** the manifest is content-free. Memory arrives later as one more
  `IncludedRef` category; #28 must not add a side channel or a manifest field.
- **#27:** inspection shows statement, scope, provenance, date, verification and
  match reason. The record must carry all of them (it does), and removal is a
  manifest decision for that turn, not a store operation.
- **#29 §1-3:** `MemoryEntry` must have nowhere to put an unconfirmed model
  inference. `MemoryProvenance`'s three variants are the whole set; adding a
  fourth would break #29's requirement 1 by construction.
- **#30 §5.6:** an entry may carry `MemoryExpiry` for a user-set expiry. #30
  §5.5 wants evidence hashes for staleness. #28 records both; #30 interprets
  them.
- **#34:** repository config is untrusted. `memory_user_scope_enabled` decides
  whether cross-project statements may be recalled, so it is **forbidden** at
  repository and profile scope.

## 5. What adding one config key costs

`Config`/`ConfigOverlay` are exhaustively destructured in several places on
purpose, so adding `memory_user_scope_enabled` is not a one-line change. All of
these compile-fail until updated, and the tests enforce the rest:

1. `Config` field and `Config::default()`.
2. `ConfigOverlay` field and `ConfigOverlay::default()`.
3. `apply_overlay_scoped`'s exhaustive destructure, plus a `scoped(...)` arm with
   `RepositoryKeyClass::Forbidden` so repository and profile config cannot set
   it.
4. `to_policy_text`'s exhaustive destructure and a `push_line`.
5. `ConfigOverlay::parse_line` for the key.
6. `classify_overlay_fields!` (Capability).
7. `profile.rs::split_profile_keys`'s exhaustive destructure and refusal list.
8. `effective_policy::rule_label`.
9. `permission_profiles.rs`: the `weakening_cases()` table is asserted to cover
   **exactly** every Capability field, so a new Capability needs a case there.
   `EVERY_PROFILE_KEY` need not carry it, because a profile must not carry it.

A test in `permission_profiles.rs` (`every_overlay_field_is_classified_exactly_once`
and `every_capability_key_resists_weakening_from_repository_scope`) holds the
partition together.

## 6. Decisions taken during planning

**D1 — `project_key` encodes its tier in the string.** The record has no
`source` field (the proposal's shape is fixed), yet the acceptance criteria
require the tier to be reportable. Keys are therefore
`root_<40-hex>`, `remote_<16-hex>` and `path_<repository_id>`. The prefix is the
tier, the suffix is filesystem-safe, and two checkouts of one repository
produce the identical string at tier 1. A remote URL is hashed rather than
embedded so a credential-bearing or unusual URL cannot become a path.

**D2 — "previous schema version" is 0, files without a marker.** The store is
new, so there is no shipped v1 to migrate from. Version 0 is a `memory/`
directory with JSONL files and no `schema.conf` — the shape a pre-versioned
writer would leave, mirroring `data_schema.rs`'s `Adopted` case. Opening it
appends a normalized record for every reusable entry (deriving `sensitivity`
and defaulting `state` to `Active`), then writes `schema_version=1`. This makes
the migration path real and testable now rather than a stub.

**D3 — `MemoryExpiry` is a struct, not an enum.** An entry may have a time
expiry, a revalidation rule, or both:
`{ expires_at_ms: Option<u128>, revalidate_on_evidence_change: bool }`. #30
§5.6 needs the time case; §5.5 needs the revalidation case; neither forces the
other out.

**D4 — session-scope writes go through a narrow `SessionStore` method.**
`append_session_event` is private, and making it public would let any caller
write a memory into the session log without the store's checks. #28 adds
`pub(crate) fn append_memory_record` / `pub(crate) fn memory_records` to
`SessionStore`. `MemoryStore::insert` is their only caller in production.

**D5 — evidence paths must be repository-relative.** An absolute path or one
containing `..` is refused before either the policy check or the ignore check
runs, so the checks cannot be pointed outside the root.

## 7. Test fixtures

Follow the existing integration-test conventions
(`tests/context_inspector.rs:1-60`): a per-test temp directory with an atomic
counter, `enable_index_watcher: false`, `data_dir` inside the temp tree, and no
real user application-support state. Identity tests need real Git repositories:
`git init`, configure `user.name`/`user.email` locally, commit, and mint a
second commit for "different history". Never kill a process by name and never
bind port 4765 — neither is needed here.

The store's own tests belong in `crates/workspace-engine/tests/memory.rs`; the
sensitivity and remote-normalization rules also get inline unit tests in
`memory.rs` where the inputs are pure.
