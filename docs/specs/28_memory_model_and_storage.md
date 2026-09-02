# Feature Spec: Memory Model and Storage

Status: Not started
Order: 28 of 30
Roadmap: `docs/ROADMAP/03b_phase_3b_persistent_memory.md`, Phase 3b, Work
Package 1 (Must). That directory is local-only and not committed, so the
reference is a name rather than a link; this spec is self-contained.
Related spec sections: `ai_coding_assistant_specification.md` section 7.3 (path
and secret policy), section 7.10 (secret detection). Related implementation
specs: [`11_agents_md_support.md`](11_agents_md_support.md) (the inspectable,
user-owned model memory should imitate),
[`26_context_assembly.md`](26_context_assembly.md) and
[`27_context_inspector.md`](27_context_inspector.md) (memory is delivered and
inspected through these, never around them),
[`29_memory_creation_and_consent.md`](29_memory_creation_and_consent.md),
[`30_memory_retrieval_and_lifecycle.md`](30_memory_retrieval_and_lifecycle.md).
See also [`SECURITY.md`](../../SECURITY.md).

## 1. Motivation

Damaian forgets everything between sessions. A user who explained last week that
this repository's integration tests need Docker running, or that the team decided
against adding a runtime dependency, explains it again today.

`AGENTS.md` is the current substitute and is the right model to imitate: a file
the user owns, can read, can edit, and whose precedence is documented
([spec 11](11_agents_md_support.md)). Memory should be at least as inspectable as
that — not less.

This work package is the storage layer only, and it is deliberately the first of
three because the shape of the record determines whether the safety properties in
[spec 29](29_memory_creation_and_consent.md) and
[spec 30](30_memory_retrieval_and_lifecycle.md) are expressible at all. A record
with no provenance cannot be attributed; one with no state cannot be superseded;
one with no scope cannot be isolated. Those fields are not bookkeeping, they are
the mechanism.

The risk is worth naming here rather than only in the roadmap. Every other
feature in this product either constrains the agent or gives it better evidence.
Memory gives it persistent, invisible, cross-session influence, which is the
hidden-instruction behaviour the product direction rejects. The storage design is
where that either becomes controllable or does not.

## 2. Current State

- **No persistent memory exists.** `SessionStore`
  (`crates/workspace-engine/src/session.rs:68`) persists sessions, tasks, and
  messages as an append-only event log. Nothing carries knowledge between
  sessions.
- **`repository_id` is derived from the absolute path.**
  `repository_id_for_root` (`crates/workspace-engine/src/indexer.rs:382-385`) is
  `sha256(canonicalized path)[..16]`, prefixed `repo_`, reached through
  `ProjectIndexer::repository_id_for_path` (`indexer.rs:191-194`), which
  canonicalizes first. **This matters: see §5.2.**
- **`repository_id` keys almost everything.** It appears in `indexer.rs`,
  `context_manager.rs`, `session.rs`, `index_cache.rs`, `chat.rs`,
  `vector_index.rs`, `edit.rs`, and `file_access.rs` — the keyword index, the
  vector index, sessions, and (per this roadmap) checkpoints and the repository
  map. Changing how it is computed invalidates all of it.
- **The secret scanner already supports refusal, not only redaction.**
  `SecretScanner::scan(text) -> Vec<SecretFinding>`
  (`crates/workspace-engine/src/secret_scanner.rs:27`),
  `contains_secrets(text) -> bool` (`secret_scanner.rs:65`), and
  `redact(text) -> Redaction` (`secret_scanner.rs:42`). `SecretFinding` carries
  `{ category, start, end, placeholder }` (`secret_scanner.rs:4-10`).
- **The audit log takes arbitrary redacted fields.**
  `AuditLog::record(event_type, fields)`
  (`crates/workspace-engine/src/audit.rs:42`), redacting every field value on
  the way in (`audit.rs:50`), with retention via `audit_retention_days`.
- **`DAMAIAN_DATA_DIR`** (`crates/workspace-engine/src/config.rs:192`) redirects
  all app data.
- **Per-repository persistence has a working precedent**:
  `<data_dir>/vector-index/<repository_id>.bin`
  (`crates/workspace-engine/src/vector_index.rs:149-152`), with `load`/`save`
  and **no schema version** — an omission this spec does not copy.
- **`git_service.rs` is read-only and narrow**: `status`, `diff`,
  `suggest_commit_message` (`git_service.rs:37`, `:77`, `:110`). There is no
  helper for repository identity.

## 3. Requirements

1. Memory is stored locally, per user and per project, following the existing
   data-directory conventions and honouring `DAMAIAN_DATA_DIR`.
2. Project memory is keyed on a repository identity such that two checkouts of
   the same repository share memory and two different repositories cannot.
3. The storage schema is versioned and migrated.
4. Creation, confirmation, recall, edit, supersession, expiry, and deletion are
   recorded through `AuditLog::record`, without exposing sensitive content
   unnecessarily.
5. Three scopes exist — session, project, user — with user scope disabled by
   default.
6. Every entry carries: stable ID; scope and project identity where applicable;
   category; a concise normalized statement; provenance; creation and update
   timestamps; confidence or verification status; state; optional expiry or
   revalidation rule; and sensitivity classification.

## 4. Non-goals

- Cloud, cross-device, or cross-user memory synchronization.
- Deciding what to remember — [spec 29](29_memory_creation_and_consent.md).
- Retrieval, ranking, or staleness detection —
  [spec 30](30_memory_retrieval_and_lifecycle.md).
- A management UI, export, or import — Phase 3b WP3, Should-tier and outside
  this phase's minimum slice.
- Contradiction detection — also WP3.
- Embedding memory into model weights, or any form of fine-tuning.
- Changing `repository_id` or how the index, vector index, or sessions are
  keyed. §5.2 introduces a separate identity rather than altering that one.
- Memory that outranks `AGENTS.md` or a current user instruction.

## 5. Design

### 5.1 The record

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryScope { Session, Project, User }

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryCategory {
    Preference, Decision, Convention, Environment, Command, Fact,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryState { Active, Superseded, Expired, Deleted }

/// How much to trust the statement, and why.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Verification {
    /// The user stated it and confirmed it.
    UserConfirmed,
    /// Derived from repository evidence that still holds.
    EvidenceCurrent,
    /// Derived from repository evidence that has since changed.
    EvidenceStale,
}

/// What the statement is about, for handling and display.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Sensitivity {
    /// Safe to show anywhere, including a shared diagnostic report.
    Ordinary,
    /// Local paths, machine names, environment details. Local display only.
    Environmental,
    /// Personal preference. Never leaves the machine, never in a report.
    Personal,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum MemoryProvenance {
    /// A user message asked for this. Carries session and message id.
    UserRequest { session_id: String, message_id: String },
    /// Suggested after a task and confirmed by the user.
    SuggestedAndConfirmed { session_id: String, task_id: String },
    /// Observed from repository content. Carries the evidence, per §5.5.
    RepositoryEvidence { paths: Vec<EvidenceRef> },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoryEntry {
    pub id: String,
    pub scope: MemoryScope,
    /// Set for Project scope; None for Session and User. See §5.2.
    pub project_key: Option<String>,
    /// Set for Session scope only.
    pub session_id: Option<String>,
    pub category: MemoryCategory,
    /// One sentence, normalized. Never a file excerpt.
    pub statement: String,
    pub provenance: MemoryProvenance,
    pub verification: Verification,
    pub sensitivity: Sensitivity,
    pub state: MemoryState,
    pub created_at_ms: u128,
    pub updated_at_ms: u128,
    /// Set when this entry replaced another.
    pub supersedes: Option<String>,
    pub expiry: Option<MemoryExpiry>,
}
```

Two shapes are worth their reasoning:

**`MemoryProvenance` is an enum, not a string.** Every variant requires the
identifiers needed to answer "why does this exist?" — and there is deliberately
**no variant for a model-inferred memory with no user action.** A model
suggestion that the user has not confirmed is not a `MemoryEntry` at all; it is a
proposal ([spec 29](29_memory_creation_and_consent.md)). Requirement 1 of that
spec is enforced by this type having nowhere to put an unconfirmed entry.

**`Verification` folds confidence and staleness into one enum** rather than a
float plus a boolean. A stale evidence-derived memory and a user-confirmed one
are categorically different, not different by degree, and
[spec 30](30_memory_retrieval_and_lifecycle.md) needs exactly this distinction to
decide what to label as uncertain. A numeric confidence would invite arithmetic
that means nothing.

### 5.2 Project identity: the roadmap's key does not work

**Requirement 2 is unsatisfiable using `repository_id_for_path`, and this is the
most important correction in this spec.**

`repository_id_for_root` is `sha256(canonicalized absolute path)`
(`indexer.rs:382-385`). So:

- Two checkouts of the same repository at `~/work/damaian` and
  `~/review/damaian` produce **different** IDs. Requirement 2's "two checkouts
  of the same repository share memory" is false.
- Two genuinely different repositories produce different IDs — but only because
  their paths differ, which is incidental rather than a property of identity.
- A repository the user moves or renames loses its memory entirely, silently.

The roadmap asserts `repository_id_for_path` gives the desired property. It does
not. So memory needs its own key, and `repository_id` must not be changed —
it keys the keyword index, the vector index, sessions, patches, file access, and
(per Phase 1 and Phase 3) checkpoints and the repository map. Re-deriving it
would invalidate every one of those stores for every existing user, to fix a
property only memory needs.

Introduce a separate `project_key`, resolved in order:

| Order | Source | Stable across |
|---|---|---|
| 1 | Root commit hash: `git rev-list --max-parents=0 HEAD`, first entry | Clones, moves, renames, remote changes, forks |
| 2 | Normalized `origin` remote URL (host + path, scheme and credentials stripped, `.git` suffix removed) | Clones and moves; not remote renames |
| 3 | `repository_id` — the existing path hash | Nothing. Per-checkout fallback |

The root commit is chosen first because it is a property of the repository's
history rather than of where it sits or what it is called, and it survives
everything a user is likely to do. A repository with no commits, or no Git at
all, falls to 2 then 3, and the fallback is recorded on the entry so a later
upgrade can tell path-keyed entries from identity-keyed ones.

Two consequences to state plainly rather than discover later:

- **A fork shares its upstream's root commit**, so a fork inherits the same
  `project_key`. That is defensible — a fork is largely the same codebase, and
  its conventions usually still apply — but it is a real sharing decision, not
  an accident, and the UI names the project by its resolved key source so a user
  can see it.
- **Two unrelated repositories that both have no commits and no remote** fall to
  the path hash, so they stay isolated. The failure mode of the fallback is
  over-isolation, which is the safe direction.

Requirement 2's second half — "two different repositories cannot share memory" —
holds at every level, and is the half that carries the security weight. It is
asserted by test at each resolution tier.

### 5.3 Layout

```text
<data_dir>/memory/schema.conf                    # schema_version=1
<data_dir>/memory/project/<project_key>.jsonl    # project scope
<data_dir>/memory/user.jsonl                     # user scope, may not exist
```

Session-scope memory is **not** a separate store: it lives in the session event
log alongside everything else session-scoped, following the pattern
`browser_diagnostics_allowed_for_session` (`session.rs:261-291`) and
[spec 20](20_working_modes.md) §5.4 already use. A session-scoped fact dies with
its session, so a store that outlives the session would be a leak to clean up
rather than a feature.

Files are append-only JSONL with the same discipline
[spec 17](17_durable_task_state_and_crash_recovery.md) §5.2 establishes: one
complete record per line, terminated with `\n`, replayed newest-state-wins by
`id`, and a torn final line discarded on read. This gives supersession and
deletion history for free — requirement 4's audit trail is the file itself —
and means no update ever rewrites a file.

Mode `0700` on the directory, consistent with the rest of the data directory.

### 5.4 Secrets are refused, not redacted

Everywhere else in Damaian, secret handling means **redaction**: context assembly
redacts (`context_manager.rs:260`), the audit log redacts (`audit.rs:50`),
command output redacts. Memory is different, and the difference must be
deliberate:

**A candidate memory containing a detected secret is refused, not stored
redacted.**

`SecretScanner::contains_secrets` (`secret_scanner.rs:65`) and `scan`
(`secret_scanner.rs:27`) already provide exactly this — the API supports refusal,
and only `redact` (`secret_scanner.rs:42`) transforms. Memory writes use `scan`;
they must not use `redact`.

The reasoning: a redacted memory is a permanently useless one. "The database
password is `[REDACTED]`" persists forever, is recalled forever, costs context
forever, and tells nobody anything. Worse, it records *that* there is a
credential and where, which is information a memory store should not accumulate.
Refusal with a clear message — "that looks like a credential; Damaian will not
store it" — is both safer and more useful.

The refusal is checked on the write path in the store itself, not only at the
call site, so no future caller can add an entry without passing it.

**Every scope goes through that one write path, including session scope.**
§5.3 persists session-scope entries in the session event log rather than a
memory file, and that is a storage detail *inside* `MemoryStore` — not a second
caller. `MemoryStore::insert` runs the secret check and then chooses its backing
store by scope; nothing writes a memory by calling
`SessionStore::append_session_event` directly. Stated explicitly because this is
the one place the guarantee could be lost while still appearing to hold: a
session-scope write that reached the session log by its own path would bypass
the check entirely, and the code would look correct.

**What this does and does not guarantee.** `SecretScanner` is pattern-based, so
the honest claim is that a *detected* secret is refused — not that no credential
can ever be stored. "The deploy token is hunter2" matches no pattern and would be
accepted. Requirement 4's "never stored" is therefore delivered by three layers,
and the second is the one that actually covers the undetectable case:

1. **Pattern refusal**, on the store's write path and again at proposal time
   ([spec 29](29_memory_creation_and_consent.md) §5.5) — catches
   recognisable credentials.
2. **User confirmation.** No entry exists that the user did not confirm
   ([spec 29](29_memory_creation_and_consent.md)), and confirmation shows the
   exact sentence that will persist. A human looking at "the deploy token is
   hunter2" will not confirm it. This is why the consent gate is a
   confidentiality control and not only a consent control.
3. **Redaction on the way out.** Memory reaches the model only as a
   `ContextItem` ([spec 30](30_memory_retrieval_and_lifecycle.md) §5.1), and
   context assembly redacts every item before measuring or storing it
   ([spec 26](26_context_assembly.md) §5.6). So a credential that somehow got
   stored — written before a scanner pattern existed, say — is still redacted
   before it is sent anywhere.

Layer 3 is the reason a scanner improvement is retroactively useful: new
patterns apply to already-stored entries at recall time, without a migration.

### 5.5 Evidence references

```rust
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EvidenceRef {
    pub path: String,
    /// Content hash when the memory was created.
    pub content_hash: String,
}
```

The hash is what makes staleness detectable
([spec 30](30_memory_retrieval_and_lifecycle.md) §5.5): a memory derived from
`docker-compose.yml` is `EvidenceStale` once that file's hash changes. This is
the same hash-comparison mechanism `patch_engine.rs:291` uses for conflict
detection, reused rather than reinvented.

`EvidenceRef` holds a path and a hash — never file content. A memory's
`statement` is a normalized sentence the user confirmed, not an excerpt, which is
requirement 6's "concise normalized statement" doing real work: it bounds what a
memory store can accumulate.

**An evidence path must be eligible, and the store enforces it.**
[Spec 29](29_memory_creation_and_consent.md) §5.5a prevents a proposal from ever
being generated for a protected or ignored file. `MemoryStore::insert` checks the
same two conditions again — `PathPolicy::is_restricted` (`path_policy.rs:138`)
and `is_ignored_by_rules` (`ignore.rs:44`) with per-directory `.gitignore` rules
— and refuses the entry if any `EvidenceRef` path fails either.

Checking in both places is the same layering as the secret check (§5.4): the
proposal check is what stops the content being surfaced, and the store check is
the guarantee. A future creation path added without the proposal check still
cannot write an entry citing a protected file, and the store is the one place
every scope's writes converge.

### 5.5a Sensitivity is assigned by rule, never by the model

`Sensitivity` (§5.1) decides whether a statement may appear in the audit log
(§5.8) and in a diagnostic report, so it must not be model-supplied. A model
that labelled a credential-bearing statement `Ordinary` would place it in an
append-only retained log — turning a display hint into a leak.

It is derived by Damaian from the confirmed entry:

| Condition | Sensitivity |
|---|---|
| Statement contains an absolute path, hostname, port, or user name | `Environmental` |
| Category is `Preference`, or scope is `User` | `Personal` |
| Anything else | `Ordinary` |

Evaluated in that order, so the most restrictive matching rule wins, and an
uncertain case resolves to the more restrictive value rather than the more
useful one. The user can raise an entry's sensitivity but not lower it below the
derived value: lowering is the only direction that can cause a disclosure, and a
user who mislabelled a personal preference as ordinary has no way to un-write
the audit line afterwards.

### 5.6 Scope defaults and the user scope

Session and project scopes are enabled. **User scope is disabled by default** and
`<data_dir>/memory/user.jsonl` does not exist until it is enabled.

`memory_user_scope_enabled=false` in user config. While false:

- No user-scope entry can be written. The store rejects the write rather than
  the UI hiding the option.
- No user-scope entry is read, even if the file exists from a previous enabling.

Both halves are needed. Reading-while-disabled is the subtler bug: a user who
turns the scope off expects it to stop influencing their sessions, not merely to
stop growing.

Enabling it is a separate explicit action with its own explanation, per Phase 3b
WP2 requirement 7 — the explanation belongs to
[spec 29](29_memory_creation_and_consent.md), the enforcement belongs here.

### 5.7 Schema version and migration

`<data_dir>/memory/schema.conf` holds `schema_version=1` in the same flat
`key=value` format as `config/user.conf`, so no new parser is needed.

This is the memory store's own version, separate from the data-directory version
in [spec 15](15_install_and_update_verification.md) §5.2 — the two evolve
independently, and a memory schema change should not require a data-directory
version bump.

Behaviour mirrors [spec 15](15_install_and_update_verification.md)'s rules: an
older version migrates, an equal version proceeds, and a **newer or unparsable
version refuses** — reporting the path and both versions, and leaving the files
untouched. Refusing to read is the right failure for memory specifically: a
newer schema misread by an older build could recall entries with the wrong scope
or the wrong state, and a wrongly-scoped recall is the exact boundary violation
this phase exists to prevent.

Because the files are append-only JSONL, a migration appends rewritten records
rather than editing in place, and the pre-migration lines remain for audit.

### 5.8 Audit

Requirement 4: every lifecycle transition — created, confirmed, recalled,
edited, superseded, expired, deleted — records through `AuditLog::record`.

Fields are the entry id, scope, category, state transition, and provenance type.
**The `statement` is not an audit field by default.** Requirement 4 says "without
exposing sensitive content unnecessarily", and a `Personal`- or
`Environmental`-sensitivity statement copied into an append-only retained log is
exactly that exposure — the audit log has its own retention and its own
"safe to share" guidance in `docs/TROUBLESHOOTING.md`. `Ordinary`-sensitivity
statements are included, because a project convention is the case where having
the text in the audit trail is genuinely useful.

Recall events are audited too, which is what makes "when was this last used?"
answerable — a WP3 requirement this spec's data supports without implementing
the view.

### 5.9 Documentation

`docs/USER_GUIDE.md`: what memory is, the three scopes, why user scope is off by
default, and that memory never outranks `AGENTS.md` or a current instruction.
`docs/TROUBLESHOOTING.md`: where memory files live, how to read one, how
`project_key` was resolved for a repository, what a refused secret write looks
like, and that memory files are **not** safe to share — alongside the existing
note about session files.

## 6. Acceptance Criteria

- An entry round-trips through storage with every field intact.
- Project memory written in repository A is not readable from repository B —
  asserted at each `project_key` resolution tier: two repositories with
  different root commits, two with different remotes, and two with neither.
- Two checkouts of the same repository at different paths share project memory —
  the requirement that fails with `repository_id_for_path`, asserted directly.
- A repository moved or renamed retains its project memory when a root commit or
  remote is available.
- `project_key` records which resolution tier produced it, and the UI can report
  it.
- `repository_id` is unchanged, and no existing index, vector index, or session
  store is invalidated by this work package.
- A candidate memory containing a seeded fake secret is **refused**, not stored
  redacted — asserted against the store's write path, not the caller.
- The secret check runs for **every** scope, session scope included: a
  session-scope write goes through `MemoryStore::insert` and is refused, and no
  memory reaches the session log by calling `SessionStore::append_session_event`
  directly — asserted by a session-scope write carrying a seeded fake secret.
- A credential that no pattern detects is still redacted before it is sent,
  because memory reaches the model only through context assembly's redaction
  path — asserted by writing an undetectable secret directly into the store file
  and checking the rendered request.
- An entry whose `EvidenceRef` cites a `restricted_patterns` path or a path
  excluded by a per-directory `.gitignore` is refused by
  `MemoryStore::insert`, independently of the proposal-stage check in
  [spec 29](29_memory_creation_and_consent.md) §5.5a — asserted by calling the
  store directly.
- `Sensitivity` is derived by rule and cannot be set from model output. A
  statement containing a path or hostname derives `Environmental`, a
  `Preference` or user-scope entry derives `Personal`, and a user may raise but
  not lower the derived value — asserted by test.
- With `memory_user_scope_enabled=false`, no user-scope entry can be written,
  **and** no existing user-scope entry is read.
- Session-scope memory does not outlive its session.
- A torn final line in a memory file is discarded and the rest replays
  correctly.
- A schema change migrates existing entries without loss, appending rather than
  rewriting.
- A newer or unparsable `schema_version` refuses to load, reports both versions,
  and modifies nothing.
- Every lifecycle transition produces an audit event, and a `Personal`- or
  `Environmental`-sensitivity statement does not appear in any audit field.
- There is no way to construct a `MemoryEntry` whose provenance is a
  model inference with no user action — asserted by the absence of such a
  variant.
- The five quality-gate commands from `AGENTS.md` pass.

## 7. Implementation Notes

To be completed during implementation. Record:

- Which `project_key` tier resolved for each repository tested, and whether any
  repository fell through to the path-hash fallback unexpectedly.
- Whether the fork-sharing consequence in §5.2 caused any surprise in real use.
  If a fork inheriting upstream memory proves wrong, the fix is a per-project
  opt-out, not a change to the resolution order — record the case first.
- The measured cost of resolving `project_key` on repository open, since it runs
  a Git command on a path that may be large or on a slow filesystem.
