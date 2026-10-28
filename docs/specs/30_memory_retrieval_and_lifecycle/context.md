# Spec 30 — Context and Corrections

Pre-implementation state, verified against `main` at `d352004` on 2026-10-28,
and re-checked against `main` at `426b00a` (2026-10-28) immediately before
Task 1. This file records what the code actually looks like, where the
specification's assumptions no longer hold, and the decisions that follow. It is
not a second proposal: where the two disagree, [`proposal.md`](proposal.md)
states the intent and this file says how the current code forces that intent to
be expressed.

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
| `MemoryStore::insert` / `supersede` / `edit_statement` | `memory.rs:1650`, `:1999`, `:1935` | The only creation and edit paths. Retrieval may append state and verification transitions (`mark_state`, C7) but must never create or rewrite an entry. |

`MemoryStore` is constructed once in `workspace_engine.rs:145` with the data
directory, `memory_user_scope_enabled`, `ignore_patterns`, the audit log, the
secret scanner, the path policy and the session store. It therefore already has
everything retrieval needs except a file root and the task prompt.

### 1.2 Context assembly (spec 26, Done)

`crates/workspace-engine/src/context_manager.rs`:

- `ContextCategory` is an ordered enum (`:38`) whose declaration order is the
  truncation priority. Spec 51 added `ExternalReference` after
  `ConversationSummary`, so it ends at `ExternalReference` (`:58`).
- `ContextCategory::BUDGETED` is a fixed `[ContextCategory; 11]` (`:81`) and
  `ContextBudget::shares` is `[u16; 11]` (`:125`), defaulted at `:134` and
  validated for oversubscription (`config.rs:2229`). The default shares sum to
  exactly 10,000, with `external_reference` at 500 funded from `search_results`
  (1,500 → 1,000).
- `ContextKind` (`:258`) is the prompt header spelling, and `ContextKind::category`
  (`:296`) is the one kind-to-category decision.
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
  filled from twelve individual overlay fields (`ConfigOverlay`,
  `config.rs:3139–3151`), each classified by the overlay macro and applied by
  name in `:1866`.
- Adding a key therefore touches `config.rs`, `profile.rs`
  (`split_profile_keys`, `:333`), `effective_policy.rs` (`rule_label`, `:129`)
  and `tests/permission_profiles.rs` — the same four files specs 25, 32 and 50
  used, exactly as the README records.
- `memory_user_scope_enabled` (`config.rs:269`) is the precedent for a memory
  switch that is user-only and forbidden to repository and profile
  (`profile.rs:443`).
- Restrict-only keys are `Capability`, not `Preference`: spec 31's rule is
  "capability ⇔ repository scope does not apply the key freely", and the
  preference test requires every `Preference` field to apply from repository
  scope unchanged. All twelve `context_budget.*` shares are `Capability` today,
  so the memory share and cap follow that pattern.

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
`indexer.rs:387`; today the function is `indexer.rs:358` and is not `pub`. Making
it `pub(crate)` is the smallest change; a new shared `text` module would be a
refactor this spec does not need.

**C2 — The budget arrays are fixed-size, and `Memory` goes after `ExternalReference`.**
Adding `Memory` is not one enum variant: `BUDGETED` and `shares` are `[_; 11]`
(`context_manager.rs:81`, `:125`), the default array and the oversubscription
validator assume eleven shares, the overlay has one field per share, and
`apply_budget`'s per-category index array is `[_; 12]` (`:1694`), sized to the
whole enum. Every exhaustive match over `ContextCategory` (`as_str`,
`budget_index`, the `CategoryUsage` order) must be updated in the same change,
or the compiler will not let the array change land alone. The JS label map is
not exhaustive and is updated with the inspector work in Tasks 3 and 5.

The plan said "after `ConversationSummary`", which was true when the plan was
written. Spec 51 has since added `ExternalReference` in that position and
declared it last so a fetched page drops before repository content. Spec 30's
own acceptance criterion is stronger — "`Memory` is the lowest-priority category
and is truncated before any other when the budget is contested" — so `Memory`
goes **after `ExternalReference`**, making it the thirteenth variant and the
twelfth budgeted category. ExternalReference keeps its guarantee (it still drops
before every repository-content category) and loses only the claim to be the
very last one. `indices`/`categories` become `[_; 13]` and capacity 13.

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

**C12 — Task 1's draft mislabelled the classification and omitted two consequences.**
The draft called the memory share and entry cap "restrict-only `Preference`
keys". That combination does not exist: a `Preference` is applied freely from
repository scope by the partition's own test, so a key with one-way direction is
a `Capability` (spec 31's "capability ⇔ repository scope does not apply the key
freely"). Both are therefore classified `Capability`, and each needs a weakening
case in `tests/permission_profiles.rs`. Two edits outside the draft's file list
also follow from the array growth: `apply_budget`'s `[Vec<usize>; 12]` and
`Vec::with_capacity(12)` must become 13 (the compiler does not catch an
out-of-range index here), and `tests/context_assembly.rs` pins the old category
count, defaults and last category in four tests:
`the_default_share_total_stays_ten_thousand`,
`each_category_reports_whether_its_tokens_are_measured_or_estimated`,
`an_external_reference_is_the_last_category_and_drops_first` (renamed to
`an_external_reference_drops_before_repository_content`) and
`a_category_orders_before_the_categories_after_it`, whose "exhaustive on
purpose" array had already missed `ExternalReference`.

**C13 — Task 2's plan left six details unnamed, and they are recorded here
because Task 3 and Task 4 depend on them.**

1. **Retention had no home in the store.** `MemoryStore` carries
   `user_scope_enabled` and `ignore_patterns`, but nothing from
   `memory_retention_days`. The setting is a property of the store rather than
   of one retrieval, so `MemoryStoreConfig` gains `retention_days` and
   `workspace_engine.rs:146` fills it from `config.memory_retention_days`,
   exactly as it already fills the user-scope switch. Task 2's file list
   therefore gains `crates/workspace-engine/src/workspace_engine.rs`.
2. **Recall redacts, and says so.** Task 2's test list requires a credential
   added directly to the memory file to be redacted "on the way out", while
   Task 3 is where a `ContextItem` is rendered. Both are true, and they are
   different boundaries: `recall` removes a detected secret from the returned
   `entry.statement` (the stored line is not rewritten) and reports
   `RecalledMemory.redacted`. Task 3 still scans the rendered frame on the way
   into the item, per the global constraint; it can use the flag so the item
   reports `redaction_status: "redacted"` rather than `"clean"` after the
   statement was already cleaned. Without the flag the second scan would see
   only the replacement marker and call it clean.
3. **Withdrawal needs a report with no excluded list in the return type.**
   `recall` returns `Vec<RecalledMemory>`; a withdrawn entry is not in it.
   The report is the `memory_recall_withdrawn` audit event, carrying entry id,
   scope, category, path and reason (`evidence_not_relative`,
   `evidence_restricted`, `evidence_ignored`). It is written once per entry and
   reason per process run, so an entry that stays withdrawn does not append a
   line every turn. The entry remains `Active` in the store and recall returns
   it again once the rule narrows.
4. **Lexical scoring needs edge handling that `tokenize` alone does not give.**
   `tokenize` keeps `.` and `/` inside a term, which is right for paths and
   wrong for sentence-final punctuation: `commas.` would never match a prompt
   saying `commas`. Recall therefore trims a trailing `.` or `/` from each
   scoring term and filters a small English stopword list. Without the stopword
   filter, grammatical overlap (`the`, `project`) would make unrelated entries
   "relevant", contradicting requirement 1. Explicit paths contribute the whole
   path and its `/`- and `.`-separated parts, so a statement naming
   `docker-compose` matches an explicit `docker-compose.yml`. Ranking quality is
   still out of scope; this only makes relevance mean what §5.3 says.
5. **The ignore check reuses the store's helper, not `ignore.rs` directly.**
   The task's reference to `is_ignored_by_rules` resolves to the store's
   existing `is_ignored_in_repository` helper, which applies the configured
   patterns plus every per-directory `.gitignore`; calling `ignore.rs` directly
   would have lost the per-directory rules. Evidence path shape is checked
   before anything is hashed: a memory file edited outside Damaian could
   otherwise name `../../…` and turn recall into a read outside the repository,
   so a non-relative path is withdrawn rather than read.
6. **The category boost is a coarse intent test, not a second ranking model.**
   A small keyword list decides "running" versus "editing"; the boost adds one
   point and only orders entries that already have a term match, never makes an
   entry eligible, so a wrong guess cannot pull in an unrelated memory.

## 3. Open questions for the owner

1. **`memory_max_entries` default.** Proposal §5.3 asks for "a small maximum".
   The store's ceiling is 280 characters per statement; five entries is roughly
   1,400 characters, under 1% of a 128k window. The plan assumes five and a
   restrict-only key, so a user can lower it but not raise it above what the
   budget can hold.
2. **`context_budget.memory` default share.** The plan assumes 300 basis points
   (3%) and restrict-only like every other share. It is enough for five short
   entries and cannot displace code. Spec 51 has landed: `external_reference`
   is now the eleventh budgeted share (500, funded from `search_results`
   1,500 → 1,000), and the defaults still total 10,000. Task 1 therefore takes
   the 300 from a different share: `findings` 500 → 200. `findings` has no
   producer in the current tree (the only references are the enum variant and
   the config table), so its share is pure carry today; if spec 22's repair
   subset is wired up, that split needs revisiting. This means memory is not
   literally the smallest share (findings is 200) — the plan's "smallest" was
   descriptive, and the acceptance criterion is the ordering, which puts memory
   last and dropped first.
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

`tasks.md` Task 1 adds the category, the kind, the three config keys and the
overlay classification. It is deliberately self-contained: no store, no turn
path, no UI. Its acceptance is the compiler plus the config tests, because every
exhaustive match over `ContextCategory` fails to compile until it is updated —
which is the point of C2.
