# Spec 30 — Context and Corrections

Pre-implementation state, verified against `main` at `d352004` on 2026-10-28.
This file records what the code actually looks like, where the specification's
assumptions no longer hold, and the decisions that follow. It is not a second
proposal: where the two disagree, [`proposal.md`](proposal.md) states the intent
and this file says how the current code forces that intent to be expressed.

Line numbers were read from the tree on the date above. Treat them as a starting
point, not a contract; re-grep before editing.

## 1. What exists before this spec

### 1.1 The memory store (specs 28 and 29, both Done)

`crates/workspace-engine/src/memory.rs` (2,942 lines) is the whole read/write
surface:

| Item | Where | What it gives this spec |
|---|---|---|
| `Verification::{UserConfirmed, EvidenceCurrent, EvidenceStale}` | `memory.rs:102` | The three states requirement 4 must render differently. |
| `MemoryProvenance::RepositoryEvidence { paths }` | `memory.rs:154` | The only provenance carrying `EvidenceRef`. |
| `EvidenceRef { path, content_hash }` | `memory.rs:195` | Hash comparison is the staleness mechanism. |
| `MemoryEntry` | `memory.rs:235` | `#[non_exhaustive]`, no constructor outside `confirm`/deserialization. |
| `MemoryStore::project_entries` / `user_entries` / `session_entries` | `memory.rs:1577`, `:1615`, `:2239` | The three scoped readers; each already refuses a wrongly scoped record. |
| `MemoryStore::mark_state` | `memory.rs:1970` | Appends a state transition and audits it. Every `Expired`/`Deleted` change goes through it. |
| `MemoryStore::record_recall` | `memory.rs:2039` | One `memory_recalled` audit event per entry. **Currently called from nowhere.** |
| `MemoryStore::insert` / `supersede` / `edit_statement` | `memory.rs:1650`, `:1999`, `:1935` | The only writes. Retrieval must not add a fourth. |

`MemoryStore` is constructed once in `workspace_engine.rs:145` with the data
directory, `memory_user_scope_enabled`, `ignore_patterns`, the audit log, the
secret scanner, the path policy and the session store. It therefore already has
everything retrieval needs except a file root and the task prompt.

### 1.2 Context assembly (spec 26, Done)

`crates/workspace-engine/src/context_manager.rs`:

- `ContextCategory` is an ordered enum (`:37`) whose declaration order is the
  truncation priority. It ends at `ConversationSummary` (`:54`).
- `ContextCategory::BUDGETED` is a fixed `[ContextCategory; 10]` (`:76`) and
  `ContextBudget::shares` is `[u16; 10]` (`:118`), defaulted at `:124` and
  validated for oversubscription (`config.rs:2134`).
- `ContextKind` (`:240`) is the prompt header spelling, and `ContextKind::category`
  (`:285`) is the one kind-to-category decision.
- `ContextRequest` carries `plan`, `pinned`, `restriction` and
  `conversation_summary` — all read by the caller from `SessionStore`. Assembly
  itself performs no I/O and has no store.
- `ContextItem` (`:578`) is the unit that reaches the prompt; `IncludedRef`
  (`:473`) is its content-free manifest projection; `ContextManifest` (`:537`)
  explicitly holds no item content because it is written to the audit log and
  shown in the UI.
- `PreparedContext` (`:647`) is the paused-review half; `remove_optional`
  (`:703`) refuses `Required` and rewrites the manifest from the remaining items.
- `build_context` is at `:1270`; callers are `chat.rs:1250` (the turn),
  `chat.rs:2627` (the repair/continuation path) and `edit.rs:526`.

### 1.3 The inspector (spec 27, Done)

- The shell receives the manifest only, never `items`
  (`chat.rs:1278` `sink.context(&context.manifest)`).
- Removal is by item index: `POST /api/context-remove`
  (`desktop-shell/src/lib.rs:338`, handler `:1173`) → `remove_context_item`
  (`chat.rs:1627`) → `PreparedContext::remove_optional`. `manifest.included` is
  built from `items` in order (`context_manager.rs:733`), which is why the index
  the shell holds is meaningful.
- The summary has a separate row and route because it is not an item
  (`chat.rs:1660`, `lib.rs:339`, `app.js:2448`).
- `app.js` renders categories from `manifest.categories` (`app.js:2311`,
  `:2420`) and items from `manifest.included` (removal at `:2373`).

### 1.4 The turn path

- Assembly, the manifest stream, the pause and `PreparedContext::from_plan` are
  in `chat.rs:1250–1420`; `pause_for_context_review` is `:1351`.
- `resume_after_context_decision` (`:1479`) rebuilds the request from the stored
  `AgentContextReview` and sends it without re-reading the world.
- `PendingChatTurn.context_review` (`:587`) is the persisted review.

### 1.5 Configuration

- `ContextBudget` lives in `config.rs` (`Context::context_budget`, `:273`) and is
  filled from eleven individual overlay fields (`ConfigOverlay`,
  `config.rs:2941–2951`), each classified by the overlay macro and validated by
  name in `:1785`.
- Adding a key therefore touches `config.rs`, `profile.rs`
  (`split_profile_keys`, `:325`), `effective_policy.rs` (`rule_label`, `:141`)
  and `tests/permission_profiles.rs` — the same four files specs 25, 32 and 50
  used, exactly as the README records.
- `memory_user_scope_enabled` (`config.rs:2942`) is the precedent for a
  memory switch that is user-only and forbidden to repository and profile
  (`profile.rs:428`).

### 1.6 The watcher

`index_cache.rs` spawns one `notify` watcher per repository (`spawn_watcher`,
`:105`), handles each event synchronously in `apply_single_path_change`
(`:147`), and forces a full rescan on error or every five minutes
(`FULL_RESCAN_INTERVAL_MS`, `:15`). The watcher thread holds only the
`ProjectIndexer`, the root, and the cache entry. It does not know the data
directory, the audit log, or that a memory store exists.

### 1.7 Shared helpers the spec assumes

- `file_hash` — `hash.rs:120` (`pub`).
- `PathPolicy::is_restricted` — `path_policy.rs:149`.
- `is_ignored_by_rules` — `ignore.rs:44`.
- `tokenize` — `indexer.rs:331`, **private**.

### 1.8 The evaluation harness

- Scenarios are TOML (`crates/eval-harness/scenarios/*.toml`) with an `[assert]`
  block deserialized into `Asserts` (`scenario.rs:65`). Existing memory
  assertions are `memory_entries`, `memory_proposals`, `memory_user_entries`,
  `memory_absent` and `memory_audit_contains` (`scenario.rs:185–203`).
- Six memory scenarios exist for spec 29. The fixtures live under
  `crates/eval-harness/fixtures/memory-injection`.
- `memory_recall_usefulness` and `memory_correction_rate` exist as metric keys
  and are deliberately `notApplicable: "phase-3b"`
  (`docs/specs/18_local_evaluation_harness/proposal.md:190`). This spec is what
  fills them.

## 2. Corrections

**C1 — `tokenize` is private, and its line moved.** The proposal cites
`indexer.rs:387`; today the function is `indexer.rs:331` and is not `pub`. Making
it `pub(crate)` is the smallest change; a new shared `text` module would be a
refactor this spec does not need.

**C2 — The budget arrays are fixed-size.** Adding `Memory` is not one enum
variant: `BUDGETED` and `shares` are `[_; 10]` (`context_manager.rs:76`, `:118`),
the default array and the oversubscription validator assume ten shares, and the
overlay has one field per share. Every exhaustive match over `ContextCategory`
(`as_str`, `budget_index`, the `CategoryUsage` order, the JS label map) must be
updated in the same change, or the compiler will not let the array change land
alone. `Memory` goes **after** `ConversationSummary` so it is last in `Ord`, per
proposal §5.1.

**C3 — The inspector cannot show a memory statement from the manifest.** The
manifest is content-free by design and is written to the audit log
(`context_manager.rs:537`, spec 26 §5.5). Proposal §5.4 and the acceptance
criteria require the inspector to show each recalled entry's statement, scope,
provenance, date and verification status. Putting statements into the manifest
would copy `Personal` and `Environmental` statements into a retained, append-only
audit log — the exposure spec 28 §5.8 explicitly refuses.

**Decision:** the manifest gains a memory row that carries the entry **id**, the
match reason and the verification state, and nothing else. A new read route
resolves an id against the memory store for display. The row exists because the
stale count must be visible without expanding (proposal §5.4), and the match
reason is derived from the prompt rather than from the entry. A deleted entry
resolves to "deleted" rather than disappearing from the history of what was sent.

**C4 — One item per entry, not one per category.** `remove_optional` removes by
item index and the shell's index is the position in `manifest.included`. A single
`Memory` item holding three entries would give the user one remove control for
three memories. Each recalled entry therefore becomes its own `ContextItem` with
`kind: memory`, no path, and content rendered as proposal §5.2's frame.

**C5 — Retrieval belongs in `memory.rs`, not in `ContextManager`.**
`ContextManager` holds no store, no data directory, no audit log and performs no
I/O; `plan`, `pinned` and `conversation_summary` are all read by the caller and
passed in. Retrieval needs the store, hashing, `PathPolicy`, the ignore rules,
`record_recall` and the config cap. Following the existing seam keeps assembly
pure and means `edit.rs:526`'s call site only gains an empty slice.

**C6 — The watcher cannot drive revalidation without a new seam.**
`spawn_watcher` (`index_cache.rs:105`) owns its thread and knows nothing about
memory; there is no process-wide registry and `IndexCache` is constructed before
`MemoryStore` in `workspace_engine.rs`. Proposal §5.5 asks for an eager
transition on watcher events *and* a lazy check at retrieval, and the acceptance
criteria require the lazy check ("even when the watcher missed the event").

**Decision:** retrieval-time revalidation is the load-bearing implementation —
hash comparison, `is_restricted`, `is_ignored_by_rules`, and the retention/expiry
transitions all run there, and each transition is audited. The eager
watcher-triggered transition is not implemented, because it buys no observable
behaviour the lazy check does not already give and would require a new global
seam between the index cache and the store. The gap is recorded in the
proposal's Implementation Notes, including the one difference it leaves: a stale
entry's `EvidenceStale` audit line is written on next recall rather than at the
moment of the file change.

**C7 — `EvidenceStale` is not a `MemoryState`.** `mark_state` handles
`Active|Superseded|Expired|Deleted`; staleness is `Verification`. A stale entry is
still `Active`, and `record_recall`/`mark_state` do not have a way to write the
verification field. The transition therefore needs a small store method that
appends the entry with a changed `verification` and audits
`memory_verification_changed`; it must not go through `mark_state`.

**C8 — Retention has two mechanisms, and only one is enforced.** Entries may
carry `MemoryExpiry` (written at creation), and `memory_retention_days` is new.
The proposal says retention defaults to no expiry. Implement both in one place:
an entry is `Expired` if its own expiry has passed, or if `memory_retention_days`
is set and its `created_at_ms` is older. The transition is appended through
`mark_state(.., Expired)`, so requirement 9 ("never presented as active") is a
state filter, not a render-time hide.

**C9 — Recall must be audited at send, not at assembly.** A paused context review
may be declined; nothing was sent, so nothing was recalled. `record_recall`
exists but has no caller. The turn path gains one call after the send is
committed, in both the default and the resume paths, and the send-time state
re-check (proposal §5.6) happens immediately before that call.

**C10 — The manifest's `IncludedRef` has no free-form detail field.** Adding a
per-entry `detail` string there would put memory text into every category's
manifest rows. The memory row is a separate, typed `Vec<MemoryRecallRef>` on the
manifest (`entryId`, `matchReason`, `verification`), mirroring how
`conversation_summary` and `symbol_provenance` were added for #27 and #25. It is
serialized with `#[serde(default)]` so manifests written before this spec still
parse.

**C11 — The two memory metrics are not free.** `memory_recall_usefulness` and
`memory_correction_rate` are `notApplicable` placeholders today. Filling them
needs a definition that can be measured from a run rather than asserted: recall
usefulness as the share of recalled entries whose statement terms appear in the
final answer or in files the run touched, and correction rate as the share of
recalled entries the user removed or that were superseded/edited in the session.
Both are proxies and must say so in spec 18's §5.6 and the harness docs; the
phase's real validation (proposal §7) is the elapsed-time observation, not the
metric.

## 3. Open questions for the owner

1. **`memory_max_entries` default.** Proposal §5.3 asks for "a small maximum".
   The store's ceiling is 280 characters per statement; five entries is roughly
   1,400 characters, under 1% of a 128k window. The plan assumes five and a
   restrict-only key, so a user can lower it but not raise it above what the
   budget can hold.
2. **`context_budget.memory` default share.** The plan assumes 300 basis points
   (3%), the smallest share in the default set, and restrict-only like every
   other share. It is enough for five short entries and cannot displace code.
   The 300 must be taken out of an existing share, because the default total is
   already at 10,000. Spec 51 adds `external_reference` to the same array on a
   parallel branch and funds it by lowering `search_results`; re-read the
   defaults when #51 has landed rather than trusting the numbers here.
3. **`memory_retention_days` scope.** The plan makes it user-only, forbidden to
   repository and profile, for the same reason `memory_user_scope_enabled` is: a
   repository must not extend how long its own derived statements are kept.
   Shortening retention is safe in principle, but the key is not worth a second
   classification rule.
4. **Matching against "paths already in context".** Proposal §5.3 counts paths
   in context as relevance terms. The plan uses the task prompt plus
   `explicit_paths`, because extension-loaded paths are not known until after
   retrieval and making retrieval depend on assembly would invert the seam in
   C5. The difference is measurable later if recall usefulness proves poor.

## 4. What the first task needs

`tasks.md` Task 1 adds the category, the kind, the two config keys and the
overlay classification. It is deliberately self-contained: no store, no turn
path, no UI. Its acceptance is the compiler plus the config tests, because every
exhaustive match over `ContextCategory` fails to compile until it is updated —
which is the point of C2.
