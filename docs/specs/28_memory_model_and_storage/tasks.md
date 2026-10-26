# Memory Model and Storage — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Implements:** [`proposal.md`](proposal.md) (requirements 1-6 and acceptance
criteria) · corrections and planning decisions in [`context.md`](context.md)
**Started:** 2026-10-26 — **Done:** 2026-10-26

## Progress

| Task | State | Notes |
|---|---|---|
| 1 · Folder conversion, status and links | Done 2026-10-26 | `git mv` of the flat spec; `proposal.md` keeps §1 and §3-§7 with its `§5.x` numbering intact, `context.md` holds the former §2 plus a corrections table for every drifted line reference, and `tasks.md` is new. Status In progress in the proposal, the README current-state bullets and both implementation-order rows. Every link naming the flat file in specs 27, 29, 30, 34, 50, 53, 55 and the README now names `28_memory_model_and_storage/proposal.md`. A relative-link scan over `docs/**/*.md` found no broken link in this folder; the one failure it reported is pre-existing in spec 66. `npm run specs:check` was deferred to Task 8, which ran it (66 specs agree); scoped docs-only change, no Rust touched. |
| 2 · Record types and sensitivity by rule | Done 2026-10-26 | `memory.rs` defines the seven enums, `MemoryProvenance` (three variants only), `EvidenceRef`, `MemoryExpiry` and `MemoryEntry`, and exports them from `lib.rs`. `Sensitivity`'s declaration order is its restrictiveness order; `derived_sensitivity` applies the §5.5a table with the most restrictive match winning, and `effective_sensitivity` raises a lower caller value but keeps a higher one. Environmental markers are an absolute path, `~/`, URL, `localhost`, `dot.host.tld`, `host:port` or `:port`, IPv4, and the local `USER`/`LOGNAME`/`HOME` name. **Deviation:** the proposal's `rename_all = "camelCase"` on `MemoryProvenance` renames variants only, so the variant fields carry explicit `sessionId`/`messageId`/`taskId` renames to keep the JSON camelCase throughout. The write path also enforces the total provenance/verification mapping (user request or confirmed suggestion → `UserConfirmed`; repository evidence → `EvidenceCurrent`/`EvidenceStale`), so a label cannot misrepresent trust. Fifteen inline tests pass after the review fixes; scoped clippy clean. |
| 3 · Project identity resolution | Done 2026-10-26 | `GitService::root_commit` (`rev-list --max-parents=0 HEAD`, first line) and `origin_url` are unaudited read helpers beside `head_commit`. `ProjectIdentity::resolve` takes the root commit first, then a normalized origin, then the path hash, and keys are `root_<sha>`, `remote_<sha256[..16]>`, `path_<sanitized repository id>` (the `repository_id` colon mapped to `_`); `source_of_key` reads the tier back. Remote normalization strips scheme, credentials, port and `.git`, lowercases the host, handles scp syntax, and returns `None` for a local path so identity falls to the path tier rather than pretending a machine path is portable. Nine integration tests cover: two clones sharing (with different `repository_id`s), a rename keeping the key, different root commits not sharing, a fork sharing deliberately, remotes without history (different and same), neither falling to the path hash, a non-repository, and no credential or slash in a key. The review found two identity defects and both are fixed: a shallow clone's `rev-list` root is the grafted boundary, so tier 1 is skipped when `git rev-parse --is-shallow-repository` is true (two `--no-local --depth 1` clones share through the remote; a full clone uses `RootCommit`; `fetch --unshallow` moves to `RootCommit` with the full clone's key), and a `file:` remote now returns no tier-2 identity because unrelated local repositories can share a path tail. **Measured cost:** about 12 ms per Git process on this machine (20 iterations, 0.246 s), dominated by process launch; a full-clone tier-1 resolution is two processes (shallow check + `rev-list`), a shallow one is the check plus `origin`, and tier 3 runs none. |
| 4 · Store layout, schema version and migration | Done 2026-10-26 | `MemoryStore` + `MemoryStoreConfig` write `<data_dir>/memory/{schema.conf,project/<key>.jsonl,user.jsonl}` with the directory at `0700`. Append-only JSONL: one complete record per line, last record per id wins, first appearance fixes order, a torn final line is discarded, and an unparsable middle line is counted and skipped so one bad record cannot make the store unreadable. The flat marker mirrors `data_schema`: absent means version 0 if files exist (migrate) or fresh (initialise), equal proceeds, newer/unparsable/0 refuses with path and versions. Version-0 migration appends a complete record for every parseable legacy line — deriving sensitivity, defaulting `state` to `Active` — keeps the old line, writes the marker, and audits `memory_schema_migrated`. A project entry whose `scope` or `project_key` differs from its file's is a hard error, not a filtered row. Reads do not create the directory; writes will. `DAMAIAN_DATA_DIR` is honoured through `Config::default_data_dir` (asserted with nextest's per-test process isolation). The review added the missing write-side half of torn-tail handling: before appending, a file that does not end in `\n` gets a lone newline, so a crash fragment becomes a skipped middle line instead of swallowing the next record, and the fragment is retained. Ten new integration tests at the time (19 in the file) passed; the fix has its own test. Scoped clippy clean. |
| 5 · Refusal path: secrets, evidence, scopes | Done 2026-10-26 | `MemoryStore::insert(root, identity, entry)` is the single write path and runs shape/scope, secret, evidence, sensitivity and persistence in that order. Secrets are refused with a category and never redacted; the statement and every evidence path are scanned. Evidence must be repository-relative (absolute, `..` and `./` refused), not `PathPolicy::is_restricted`, and not ignored by configured patterns or any per-directory `.gitignore` from the root down. Project writes require the resolved identity and a matching key; session writes require an existing session and reach the log only through `SessionStore::append_memory_record`/`memory_records` (`pub(crate)`, leaving `append_session_event` private); user writes refuse while disabled and reads return nothing without opening `user.jsonl`. Sensitivity is stored as `max(supplied, derived)`, so it can be raised but never lowered. The review added three guards: the project key shape (`root_`/`remote_`/`path_` plus non-empty ASCII alphanumerics, `_` or `-`) is enforced on write and read, so a constructed `../escape` key cannot become a path; `insert` refuses an id already present in the target scope, because replay is last-wins and a second creation would be an un-audited edit; and a statement is trimmed before storage. The path-tier key also maps the colon inside `repository_id` to `_`, which the shape rule exposed. Fourteen new integration tests at the time (33 in the file) passed, including a session-scope write carrying a seeded fake AWS key that leaves no `memory_recorded` event and no secret in the log; the review guards have their own tests. Scoped clippy clean. |
| 6 · Lifecycle transitions and audit | Done 2026-10-26 | `MemoryTarget { Project, Session, User }` and append-only transitions: `edit_statement` (re-runs shape, secret, evidence and sensitivity checks; refuses a non-active entry), `mark_state` (Superseded/Expired/Deleted/Active, idempotent on the same state), `supersede` (inserts the replacement first and only then retires the old entry), and `record_recall` (one event per entry). `AuditLog::record` gets `memory_created`, `memory_confirmed` (for `SuggestedAndConfirmed`), `memory_recalled`, `memory_edited`, `memory_superseded`, `memory_expired`, `memory_deleted`, and `memory_state_changed` for reactivation; recall has no state fields, so the log does not claim a transition. Fields are `entryId`, `scope`, `category`, `provenance`, `sensitivity` and `fromState`/`toState`, plus `statement` only when sensitivity is `Ordinary`. Six new tests at the time (39 in the file) cover the full walk, confirmation, sensitivity absence from the whole audit file, a disabled audit log writing nothing, a refused edit leaving the old statement active, and the supersede link. The review added a same-session guard to `supersede`: a replacement naming another existing session is refused instead of being written into that log while the original is retired in this one. Scoped clippy clean. |
| 7 · User-scope config key and engine wiring | Done 2026-10-26 | `memory_user_scope_enabled` (default `false`) added to `Config`/`ConfigOverlay` and carried through every exhaustive site named in `context.md` §5: apply (Forbidden), policy text, parse, `classify_overlay_fields!` (Capability), `split_profile_keys` (refused), and the rule label "Remember facts across projects". `WorkspaceEngine` now builds one `MemoryStore` from the resolved config (data dir, flag, ignore patterns) and exposes it as `memory_store`; `lib.rs` exports the record, store and identity types. The capability partition test required a new `weakening_cases()` entry, and a focused test covers default-off, user-on, repository-refused in both directions, profile-split refusal and the label. Scoped runs: `--test permission_profiles` 80 passed (1 ignored), `--test repository_config_trust` 50, lib `config::` 2, `--test memory` 40 (including an engine-wiring test); scoped clippy clean. |
| 8 · Documentation and close-out | Done 2026-10-26 | **Docs.** `USER_GUIDE.md` gains a **Memory** section (what it is, the three scopes, user scope off and how to enable it in user config, the repository-config refusal, precedence below `AGENTS.md` and a current instruction, secret refusal, not safe to share, and that creation/recall land in #29/#30 so no memory is created yet). `TROUBLESHOOTING.md` gains three state-table rows, a **Memory** section (layout, how to read one with `jq`, the `root_`/`remote_`/`path_` tier prefixes, a refused secret write, a newer-schema refusal, the lifecycle audit events, not-safe-to-share) and a symptom-index row. Proposal §7 records the tiers observed, fork sharing, the shallow-clone case, the measured ~12 ms per-Git-process cost, the deviations (camelCase provenance fields, `pub(crate)` session methods, `MemoryExpiry` shape, version 0 adoption, `./` evidence refusal) and the one acceptance criterion deferred to #30. **Close-out.** #29's `Depends on:` line flipped to built (Done 2026-10-26), #30's #28 entry likewise; README ready set promotes #29 and leaves #30 blocked on it, the current-state bullets and priority row are updated, #28's row is Done, and a "Re-derived on 2026-10-26" history entry records what #28 leaves. #28's line is gone from `CHANGELOG.md` Unreleased; no release row added. **Review round.** A second review added eight corrections, all landed and tested: crash-torn tails are separated before the next append (Task 4); provenance and verification must agree (Task 2); the project-key shape is enforced on write and read, with the path-tier colon mapped (Task 5); shallow clones skip the root-commit tier and `file:` remotes return no identity (Task 3); `supersede` cannot cross sessions (Task 6); `insert` refuses an existing id (Task 5); and the records below were corrected (Task 8). Test counts rose from 44 integration plus 13 inline to 52 plus 15. **Checks after the review round.** `scripts/quality-gate.sh fmt node lint typos deny` passed (fmt 1.67 s, node 0.30 s, lint 1.18 s, typos 0.41 s, deny 1.41 s). `cargo clippy -p workspace-engine --all-targets --locked -- -D warnings` passed. `cargo nextest -p workspace-engine` passed 1861 tests (27 skipped), including 52 in `--test memory` and 15 in `--lib memory::`; the focused runs were `--test permission_profiles` 80 passed (1 ignored), `--test repository_config_trust` 50 passed and lib `config::` 2 passed. `cargo check --workspace --all-targets --locked` passed. `npm run specs:check` reports 0 disagreements. **CI outstanding:** the full seven-check gate (workspace clippy and the whole nextest suite) is left to CI, so the spec is Done with CI green still to be recorded when the branch is pushed. **Not verified in this slice:** the undetectable-credential redaction criterion, which needs #30's recalled item and rendered request. |

**Goal:** Give Damaian a local, versioned, per-project memory store whose record
shape makes #29's consent gate and #30's retrieval possible — with a project
identity that survives moves and clone paths, a secret path that refuses rather
than redacts, and no way to persist an unconfirmed model inference.

**Architecture:** A new `memory` module in `workspace-engine` holds the record
types, the project identity resolver, and `MemoryStore`. Project and user entries
are append-only JSONL under `<data_dir>/memory/`; session entries are appended to
the existing session log through a narrow `SessionStore` method. `MemoryStore` is
the single write path, so the secret check, evidence-eligibility check, scope
check and sensitivity derivation cannot be bypassed by a future caller.
`WorkspaceEngine` exposes one store; nothing in `chat.rs` or `context_manager.rs`
changes.

**Tech Stack:** Rust 2024, existing `serde`/`serde_json`, existing `SecretScanner`,
`AuditLog`, `PathPolicy`, `SessionStore`, `GitService`, the existing JSONL and
schema-marker conventions. No new dependencies. No JavaScript.

## Global Constraints

Every task's requirements implicitly include this section.

- **Read [`context.md`](context.md) first.** Its decisions override the
  proposal where they differ:
  - D1: `project_key` is `root_…`, `remote_…` or `path_…`; the prefix names the
    tier and the suffix is filesystem-safe.
  - D2: schema version 0 is "files with no marker"; migration to 1 appends
    normalized records and then writes the marker.
  - D3: `MemoryExpiry` is a struct with an optional timestamp and a
    revalidation flag.
  - D4: session-scope writes use `SessionStore`'s `pub(crate)` memory methods;
    `append_session_event` stays private.
  - D5: evidence paths must be repository-relative; absolute paths and `..` are
    refused before any policy check.
- **One write path.** `MemoryStore::insert` (and its transition methods) run the
  secret check, evidence check, scope check and sensitivity derivation for
  **every** scope. Nothing may write a memory by another route.
- **Secrets are refused, never redacted.** Use `SecretScanner::scan` (or
  `contains_secrets`); never `redact` on a write. A refusal names no content and
  no secret.
- **Memory is data, never instruction.** `MemoryProvenance` has exactly three
  variants; do not add a model-inference variant. Do not add instruction parsing,
  allow-list changes, mode effects, or a path that persists an unconfirmed
  candidate. #29 owns proposals and consent.
- **`repository_id` and every store keyed by it stay unchanged.**
- **Do not edit `chat.rs` or `context_manager.rs`.** #30 owns retrieval and the
  memory context category.
- **Repository config is untrusted.** `memory_user_scope_enabled` is Forbidden at
  repository and profile scope, and the classification must go through the
  existing exhaustive machinery — no wildcard.
- **No security boundary is weakened:** redaction, path policy, command policy,
  patch hashes, questions-are-not-approvals, repository-config trust.
- **Audit without content.** Audit fields are entry id, scope, category,
  provenance type, state transition, and — only for `Ordinary` sensitivity — the
  statement. `Personal` and `Environmental` statements never appear in an audit
  field.
- **Fixtures** use throwaway data directories, `enable_index_watcher: false`,
  real local Git repositories where identity is under test, and no credentials.
  Never take port 4765 and never kill a process by name.
- **Scoped checks only.** `scripts/quality-gate.sh fmt node lint typos deny`,
  `cargo clippy -p workspace-engine --all-targets --locked -- -D warnings`, and
  the scoped nextest suites. The full workspace gate is CI's job.

## Task 1 · Folder conversion, status and links

- [x] `git mv docs/specs/28_memory_model_and_storage.md
      docs/specs/28_memory_model_and_storage/proposal.md`.
- [x] Move the pre-implementation state out of the proposal into
      `context.md`, with every line reference re-verified and a corrections
      table.
- [x] Write this `tasks.md` with Global Constraints, tasks, progress table and
      verification.
- [x] Set `Status: In progress` in `proposal.md`; update the README current-state
      paragraph and the implementation-order row.
- [x] Repoint every link that named `28_memory_model_and_storage.md`, in specs
      29, 30, 34, 50, 53, 55, the README and this spec's neighbours.
- [x] Confirm no relative link in the new folder is broken and no stale flat
      path remains (`grep -rn "28_memory_model_and_storage.md" docs/`). The only
      matches left are this task's own record.

**Verification:** `git status` shows the rename; the grep above finds only
intentional folder-path references (e.g. a link to `proposal.md` inside the
folder); `npm run specs:check` reports no new mismatch for #28.

## Task 2 · Record types and sensitivity by rule

- [x] Add `crates/workspace-engine/src/memory.rs` and export it from `lib.rs`.
- [x] Define `MemoryScope`, `MemoryCategory`, `MemoryState`, `Verification`,
      `Sensitivity`, `MemoryProvenance`, `EvidenceRef`, `MemoryExpiry` and
      `MemoryEntry` exactly as the proposal's §5.1 and §5.5 shapes require,
      with `serde(rename_all = "camelCase")` on the structs and
      `serde(rename_all = "snake_case")` on the enums.
- [x] Give `Sensitivity` a total restrictiveness order
      (`Ordinary < Environmental < Personal`) and derive it by rule, in the
      proposal's order: an environmental marker anywhere, then a `Preference`
      category or `User` scope, else `Ordinary`; the most restrictive match wins.
- [x] Implement environmental-marker detection: absolute path, `~/` path,
      hostname (including `localhost`, `.local`, dotted names), `host:port` /
      `:port`, IPv4, and the local user name from `USER`/`LOGNAME`/`HOME`.
- [x] Add `MemoryEntry::effective_sensitivity()` returning the maximum of the
      stored value and the derived one, plus `with_derived_sensitivity`.
- [x] Unit tests for each derivation row, for the restriction order, and for a
      lowering attempt being raised.

**Verification:** `cargo nextest run -p workspace-engine --lib memory::` passes;
`cargo clippy -p workspace-engine --all-targets --locked -- -D warnings`.

## Task 3 · Project identity resolution

- [x] Add read-only identity helpers to `git_service.rs`: the first root commit
      (`git rev-list --max-parents=0 HEAD`, first line) and the `origin` URL
      (`git config --get remote.origin.url`). Neither is audited, matching
      `head_commit`.
- [x] Add `ProjectKeySource { RootCommit, RemoteUrl, RepositoryId }` and
      `ProjectIdentity { key, source, repository_id }` to `memory.rs`.
- [x] `ProjectIdentity::resolve(root, repository_id, &GitService)` resolves in
      order: root commit → `root_<sha>`; normalized origin URL →
      `remote_<sha256[..16]>`; `repository_id` → `path_<sanitized repository id>`.
      A shallow clone skips the root-commit tier, and a `file:` remote yields no
      remote tier.
- [x] Normalize a remote URL: strip scheme, credentials, port and a trailing
      `.git`, lowercase the host, keep host and path; handle `git@host:owner/repo`
      scp syntax. Return `None` for a URL with no host or path.
- [x] Test each tier, including two checkouts of one repository sharing a key
      (clone), a renamed checkout keeping its key, two unrelated repositories
      differing at every tier, and a fork sharing the root commit deliberately.
- [x] Record the measured cost of resolution (a `git` process per tier) in the
      progress row and in the proposal's §7.

**Verification:** `cargo nextest run -p workspace-engine --test memory
project_identity`; scoped clippy.

## Task 4 · Store layout, schema version and migration

- [x] `MemoryStore` over `<data_dir>/memory/`:
      `schema.conf`, `project/<project_key>.jsonl`, `user.jsonl`; create the
      directory with mode `0700`.
- [x] Append one complete JSON record per `\n`-terminated line; replay by id with
      the last record winning; discard a torn final line; skip and count an
      unparsable middle line rather than failing the whole store.
- [x] Read/write the flat `schema_version=1` marker. Equal proceeds; older
      migrates; newer or unparsable refuses with the path and both versions and
      changes nothing.
- [x] Migration from 0 (no marker): for each project and user file, append a
      normalized record for every parseable entry — derive `sensitivity`, default
      a missing `state` to `Active`, keep the original line — then write the
      marker. Audit `memory_schema_migrated` with `from`/`to` only.
- [x] Tests: round-trip every field; append-only growth (a rewrite never
      truncates); torn final line; a v0 directory read after upgrade with the
      marker now 1 and the pre-migration line still present; a newer and an
      unparsable version each refusing and leaving bytes untouched.
- [x] Test `DAMAIAN_DATA_DIR` is honoured by constructing the store from
      `Config::default_data_dir`.

**Verification:** `cargo nextest run -p workspace-engine --test memory store_`;
scoped clippy.

## Task 5 · Refusal path: secrets, evidence, scopes

- [x] Define `MemoryError`/`MemoryRefusal` with stable variants for a detected
      secret, ineligible evidence (restricted / ignored / not-relative), a
      disabled scope, an invalid shape and an unsupported schema. Implement
      `Display` with the stated sentences and `From<MemoryError> for ClientError`.
- [x] `MemoryStore::insert(root, entry)` validates scope/key consistency: Project
      requires `project_key` equal to the root's resolved key and no session id;
      Session requires a session id and no project key; User requires neither.
- [x] Secret refusal: scan the statement and every evidence path with
      `SecretScanner::scan`; any finding refuses the write. Never call `redact`.
- [x] Evidence eligibility: refuse an absolute path or one containing `..`; then
      `PathPolicy::is_restricted(path, false)`; then ignore rules accumulated
      from configured `ignore_patterns` plus per-directory `.gitignore` files
      from the root down.
- [x] Scope enforcement: a User write refuses when
      `memory_user_scope_enabled=false`; a User read returns nothing without
      opening `user.jsonl` when false.
- [x] Session writes append through `SessionStore::append_memory_record`
      (`pub(crate)`), never by calling `append_session_event`; reads go through
      `memory_records`. A session entry is invisible from another session and
      disappears with `delete_session`.
- [x] Sensitivity is applied on the write path: the stored value is the maximum
      of what the caller supplied and the derived rule, so a caller cannot lower
      it.
- [x] Tests: session-scope write carrying a seeded fake secret is refused and
      nothing reaches the session log; a project entry citing a
      `restricted_patterns` path is refused when calling the store directly; the
      same for a per-directory `.gitignore` path; user scope refused on write and
      unread on read while disabled; session isolation and deletion.

**Verification:** `cargo nextest run -p workspace-engine --test memory refusal`
and `memory::session`; scoped clippy.

## Task 6 · Lifecycle transitions and audit

- [x] Implement transitions as appended records, never rewrites: edit statement,
      mark state (`Superseded`, `Expired`, `Deleted`, back to `Active`), and
      supersede (new entry carries `supersedes`; old entry becomes `Superseded`).
      Re-run the secret, evidence, sensitivity and scope checks whenever the
      statement or scope changes.
- [x] `record_recall(entries)` appends one `memory_recalled` event per entry,
      with ids only plus the statement when `Ordinary`.
- [x] Audit every transition through `AuditLog::record`: `memory_created`,
      `memory_confirmed` (when provenance is `SuggestedAndConfirmed`),
      `memory_recalled`, `memory_edited`, `memory_superseded`, `memory_expired`,
      `memory_deleted`. Fields: `entryId`, `scope`, `category`, `provenance`,
      `fromState`, `toState`, and `statement` only when sensitivity is
      `Ordinary`.
- [x] Tests: one lifecycle walk asserting one event per transition; a
      `Personal` statement and an `Environmental` statement each absent from the
      whole audit file; an `Ordinary` statement present; a disabled audit log
      returns the event JSON but writes nothing.

**Verification:** `cargo nextest run -p workspace-engine --test memory audit`;
scoped clippy.

## Task 7 · User-scope config key and engine wiring

- [x] Add `memory_user_scope_enabled: bool` (default `false`) to `Config` and
      `ConfigOverlay`, and carry it through every exhaustive destructure listed
      in `context.md` §5: overlay apply (Forbidden), policy text, parse line,
      `classify_overlay_fields!` (Capability), `split_profile_keys` (refused),
      and `effective_policy::rule_label` ("Remember facts across projects").
- [x] Add a `weakening_cases()` entry in `tests/permission_profiles.rs` proving a
      repository cannot set the key either way, and that the profile split
      refuses it.
- [x] Construct `MemoryStore` in `WorkspaceEngine::build` and expose
      `pub memory_store: MemoryStore`; export the module and record types from
      `lib.rs`. Nothing else consumes it yet.
- [x] Test: default is false; user config true enables it; repository config
      cannot set it in either direction; profile export does not carry it.

**Verification:** `cargo nextest run -p workspace-engine --test
permission_profiles` (the two partition tests plus the new case) and
`cargo nextest run -p workspace-engine --lib config::`; scoped clippy.

## Task 8 · Documentation and close-out

- [x] `docs/USER_GUIDE.md`: what memory is, the three scopes, why user scope is
      off by default and how to enable it, and that memory never outranks
      `AGENTS.md` or a current instruction.
- [x] `docs/TROUBLESHOOTING.md`: where memory lives, how to read a file, how the
      `project_key` tier is read from the filename prefix, what a refused secret
      write looks like, and that memory files are **not** safe to share.
- [x] Fill the proposal's §7 implementation notes: tier resolved per fixture,
      fork-sharing observations, and measured `project_key` resolution cost.
- [x] Mark the spec Done in all four places: `proposal.md` `Status`, README
      current state, README implementation-order row, `tasks.md` header and
      progress table.
- [x] Promote #29: flip its `Depends on:` line to built, update the README ready
      set, and add a "Re-derived on 2026-10-26" history entry naming what #28
      leaves for #29 and #30.
- [x] Remove #28's line from `CHANGELOG.md` → Unreleased. Do not add a release
      row.
- [x] Walk the acceptance criteria in proposal §6 against the tests and record
      any that could not be verified.
- [x] Run the fast checks and scoped suites; say plainly that the workspace
      clippy and test suite are left to CI.

**Verification:** `scripts/quality-gate.sh fmt node lint typos deny`;
`cargo clippy -p workspace-engine --all-targets --locked -- -D warnings`;
`cargo nextest run -p workspace-engine --test memory`; `npm run specs:check`.
