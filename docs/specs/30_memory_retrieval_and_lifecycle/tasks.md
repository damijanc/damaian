# Spec 30 — Tasks

Implementation tasks for
[`proposal.md`](proposal.md). Read the proposal's §3 requirements and §5 design,
and [`context.md`](context.md) for the pre-implementation state and the
corrections planning found, before starting any task.

**Started:** 2026-10-28

## Global Constraints

- **`chat.rs` is a solo track.** Tasks 3 and 4 edit the turn path, and Task 4
  edits `build_model_prompt`'s caller. No other spec may be in flight that edits
  `chat.rs` while either task is open. This is the same rule the README records
  for #27 and #49's reuse slice.
- **The config four-file pattern.** Task 1 adds keys to `config.rs`,
  `profile.rs`, `effective_policy.rs` and `tests/permission_profiles.rs`. Every
  new `ConfigOverlay` field forces a decision in `split_profile_keys`; do not
  add wildcard handling.
- **The manifest stays content-free.** No task may put a memory statement into
  `ContextManifest` or into an audit event. Statements are resolved by id at
  display time (context.md C3).
- **No new creation path.** Retrieval never calls `insert`. The only writes it
  may perform are `mark_state` (expiry, deletion), the verification transition
  (context.md C7) and `record_recall`; none of them creates or rewrites an
  entry's statement.
- **Secrets.** Every rendered statement passes the existing `SecretScanner` on
  the way into a `ContextItem`. The store refuses *detected* secrets; a memory
  file edited outside Damaian may still hold one, and requirement 6 of spec 29
  leaves that end-to-end assertion here.
- **Scoped checks only.** Run `cargo fmt --all`, scoped `cargo clippy -p …`,
  scoped `cargo nextest run -p …`, and the web checks when `app.js` changes.
  The full seven-check gate runs on CI, never locally (AGENTS.md).
- **Do not change spec 28's or 29's behaviour.** If a task finds a defect there,
  record it in `context.md` and in `docs/PLAN/OBSERVATIONS.md` when that
  directory is available, rather than widening the task.

## Progress

| Task | Status | What landed |
|---|---|---|
| 1 · Types and configuration | Done 2026-10-28 | **Landed:** `ContextCategory::Memory` is the thirteenth variant and twelfth budgeted category, **after** spec 51's `ExternalReference` (plan correction: the draft said "after `ConversationSummary`", which would have left fetched pages lower than memory and violated this spec's own "memory drops first" acceptance criterion). `BUDGETED`/`shares` are `[_; 12]`; `apply_budget`'s `indices` is `[_; 13]` and its capacity is 13. The default table totals 10,000 with memory at 300, funded by lowering `findings` 500 → 200 — `findings` has no producer in the tree, and `search_results` had already paid for `external_reference`. `ContextKind::Memory` maps `"memory"` to the category. New keys: `memory_max_entries` (u32, default 5, restrict-only lower-wins through `restrict_only_u32`), `memory_retention_days` (u64, default 0 = no expiry, `Forbidden` to repository and profile) and `context_budget.memory` (restrict-only share). All three are `Capability`, not `Preference` as the draft said: spec 31 defines a capability as a key repository scope does not apply freely, and its preference test requires free application. Both overlay writers and the resolved policy text print them; `rule_label` names them; `split_profile_keys` carries the cap and share and refuses retention; `equals_base_limit` knows the cap. **Tests:** `permission_profiles` 87/87 (three new weakening cases and two new tests: profile direction, user-only retention); `context_assembly` 52/52; `config::tests` 3/3; full `workspace-engine` 2,039 passed, 28 skipped. **Discoveries:** two consequences lay outside the draft's file list — `apply_budget`'s index array had no compile-time link to the enum size (a 13th variant would have panicked at runtime, not failed the build), and `context_assembly.rs` pinned the old count/last category in three places, including `an_external_reference_is_the_last_category_and_drops_first`, renamed `an_external_reference_drops_before_repository_content`. **Intermediate state:** the inspector shows the new empty `memory` row with its raw key label until Task 3 adds the label map and Task 5 the row, as planned. **Docs:** proposal §5.1 and Status, context.md C2/C12 and §1.2/§1.5/§3 Q2, spec 26 §5.1–§5.2 defaults, and the README row/current-state paragraph were corrected. Scoped checks: fast gate (`fmt`, `node`, `lint`, `typos`, `deny`) passed; scoped clippy clean; `npm run specs:check` passed. Workspace clippy and the full suite are left to CI. |
| 2 · Recall engine | Done 2026-10-28 | **Landed:** `MemoryStore::recall(root, identity, session_id, prompt, explicit_paths, cap)` in `memory.rs`, returning `RecalledMemory { entry, match_reason, verification, redacted }`. Order: scope (session / project / user only when enabled), `Active` only, expiry through `mark_state(.., Expired)` (own `MemoryExpiry` or `memory_retention_days`), evidence withdrawal, lexical scoring, cap, then hash revalidation and redaction for the selected entries only. `MemoryMatchReason { matched_terms, category_boost }` with `describe()`, serde-ready for Task 3's manifest. Scoring is `tokenize` plus a stopword filter and a trailing `.`/`/` trim; explicit paths contribute the whole path and its split parts; the +1 category boost from a small running/editing keyword test never makes an entry eligible. Verification: `set_verification` appends and audits `memory_verification_changed` without `mark_state`; any hash difference or unreadable/missing path is `EvidenceStale`, equality is `EvidenceCurrent`, so a revert works. Restricted, ignored or non-relative evidence is withdrawn with `memory_recall_withdrawn` (once per entry+reason per process); the entry stays `Active` and returns when the rule narrows. `MemoryStoreConfig` gains `retention_days`, filled from `config.memory_retention_days` in `workspace_engine.rs` — the task's file list omitted that file. Recall redacts a credential edited into the memory file and reports `redacted`, leaving the stored line unchanged (C13.2). `tokenize` is `pub(crate)`. **Tests:** `tests/memory.rs` 95/95 including 14 new: scope and session isolation, user scope off, state filter, cap and match reason, category boost, explicit paths, stopword-only overlap, stale/revert, deleted evidence, restricted+ignored withdrawal/restore, project-key mismatch at recall, all three project-key tiers, retention and entry expiry, and a file-injected credential redacted on read and in audit. Full `workspace-engine` 2,053 passed, 28 skipped. **Discoveries:** `tokenize` keeps `.` inside a term, so a statement-final `commas.` could never match a prompt saying `commas`; scoring trims edge punctuation. `Config`'s retention key had no store field. Withdrawal needed an audit report because recall's return type has no excluded list. **Checks:** `fmt`, `node`, `lint`, `typos`, `deny`; scoped clippy; `nextest -p workspace-engine`. |
| 3 · Context assembly | Done 2026-10-28 | **Landed:** `ContextRequest.memory: &'a [RecalledMemory]`; one `ContextItem` per entry, `kind: memory`, no path, content in the §5.2 frame with the entry's own `created_at_ms` date — user-confirmed, `evidence unchanged` for current evidence, or `may be out of date — that file has changed since` when stale (plural wording for several paths). Memory is collected after external references, so it is the last prompt block. `ContextManifest.memory: Vec<MemoryRecallRef>` (`entryId`, `matchReason` as `MemoryMatchReason`, `verification`, `#[serde(default)]`) records only included entries, never the statement; `PreparedContext::remove_optional` drops the matching ref by counting earlier `Memory` items (C14). Redaction runs in `add_text`, and `RecalledMemory.redacted` restores the status the second scan cannot see (C13.2). The §5.2 date reuses spec 51's calendar arithmetic through the new `external_reference::utc_date` rather than a second conversion. **Plan corrections:** the seam reaches eight more files — `chat.rs` (2 sites), `edit.rs` (1), five `workspace-engine` test files, two `desktop-shell` manifest fixtures (C14), plus `external_reference.rs` for the date helper and `lib.rs` for the re-export; all unlisted in the draft. Steps 4 and 5 were already satisfied by Task 1 (C14); and `context_budget.memory=0` did **not** switch memory off, because every category accepted the released carry, so `apply_budget` now treats a zero share as off and passes the carry through (C15, with one clarifying sentence in spec 26 §5.2). **Tests:** `context_manager` inline 15/15, six new — delivery and frame, zero share, memory dropped before file content at exact shares, stale/current markers and manifest verification, credential redaction and the recall-flag path, removal including ref alignment; full `workspace-engine` 2,059 passed, 28 skipped (including `prompt_cache`); `desktop-shell` 224 passed, 18 skipped; deterministic eval tier exit 0 with every scenario passing and approval-policy, restricted-path and secret violations all 0. **Discoveries:** a dropped or send-time-rejected memory needs its `memory` ref removed as well as an exclusion record, or the inspector shows a statement the request no longer carries; Task 4 takes that up. **Checks:** fast gate `fmt node lint typos deny` ok; scoped clippy `-D warnings` for `workspace-engine` and `desktop-shell`; scoped nextest for both crates. Workspace clippy/tests left to CI. |
| 4 · Turn wiring and send-time re-check | Not started | — |
| 5 · Inspector and shell | Not started | — |
| 6 · Evaluation harness and metrics | Not started | — |
| 7 · Documentation | Not started | — |
| 8 · Close-out | Not started | — |

## Task 1 — Types and configuration

**Goal:** the vocabulary exists, the compiler has forced every exhaustive match,
and the three new settings behave as restrict-only or user-only.

**Files:** `crates/workspace-engine/src/context_manager.rs`,
`crates/workspace-engine/src/config.rs`,
`crates/workspace-engine/src/profile.rs`,
`crates/workspace-engine/src/effective_policy.rs`,
`crates/workspace-engine/tests/permission_profiles.rs`,
`crates/workspace-engine/tests/context_assembly.rs`.

**Steps**

- [x] Add `ContextCategory::Memory` **after `ExternalReference`** in
      `context_manager.rs:58`, with the doc comment that it is lowest priority
      and dropped first. The draft said "after `ConversationSummary`", which
      spec 51's landed `ExternalReference` has since occupied; memory must be
      last in `Ord` to satisfy this spec's own acceptance criterion (context.md
      C2).
- [x] Add `ContextKind::Memory` and its `as_str` (`"memory"`) and `category`
      arms; update `BUDGETED`, `budget_index`, the `shares` array type and the
      default array to twelve entries, `apply_budget`'s `[Vec<usize>; 12]` and
      `Vec::with_capacity(12)` to thirteen, and the tests in
      `tests/context_assembly.rs` that assert the category count and that
      `ExternalReference` is the last category.
- [x] Add `context_budget_memory: Option<u16>` and
      `memory_max_entries: Option<u32>` to `ConfigOverlay`, the built-in config
      fields and parser, and `memory_retention_days: Option<u64>` as a user-only
      key. Fund the new share by lowering an existing one; never raise the
      total. Spec 51 has landed: `external_reference` (500) is funded from
      `search_results` (1,500 → 1,000). Take memory's 300 from `findings`
      (500 → 200), which has no producer in the current tree, rather than from
      file content.
- [x] Classify all three as `Capability`: the share and the cap are restrict-only
      (lower-wins), and `memory_retention_days` is forbidden to repository and
      profile, like `memory_user_scope_enabled` (`profile.rs:443`). The draft
      said "`Preference`", which contradicts the partition's rule that a
      capability is a key repository scope does not apply freely (context.md
      C12).
- [x] Add `rule_label` text for each key and extend
      `tests/permission_profiles.rs` with the three classifications: a
      repository raise is refused by name, a repository retention value is
      refused, and a profile may lower the cap and the share but not raise them.
- [x] Update `Config::validate_context_budget` and its test so twelve shares
      still total at most 10,000, and add `memory_max_entries` to
      `profile.rs`'s `equals_base_limit` so an equal cap is not reported as a
      loosening import.

**Checks:** `cargo fmt --all`; `cargo clippy -p workspace-engine --all-targets
--locked -- -D warnings`; `cargo nextest run -p workspace-engine --test
permission_profiles`; the `config` inline tests; `cargo nextest run -p
workspace-engine --test context_assembly` (the two category-count tests).

**Done when:** the workspace compiles with `Memory` in every match, and the
config tests prove the three keys' direction and scope.

## Task 2 — Recall engine

**Goal:** `MemoryStore` can answer "what applies to this task, and does it still
hold?" without any UI or prompt code.

**Files:** `crates/workspace-engine/src/memory.rs`,
`crates/workspace-engine/src/indexer.rs` (`tokenize` visibility),
`crates/workspace-engine/src/workspace_engine.rs` (retention reaches the store),
tests in `crates/workspace-engine/tests/memory.rs`.

**Steps**

- [x] Add `RecalledMemory { entry, match_reason, verification, redacted }` and a
      `MemoryStore::recall(root, identity, session_id, prompt, explicit_paths,
      cap)` that:
      - filters by scope (session for this session, project for this
        `project_key`, user only when the scope is enabled);
      - filters `Active` only, before ranking (requirement 9);
      - expires entries whose own `MemoryExpiry` or `memory_retention_days` has
        passed, through `mark_state(.., Expired)` (context.md C8);
      - scores term overlap with `tokenize` over the statement against the
        prompt plus `explicit_paths`, filtering stopwords and a sentence-final
        `.`/`/` so `commas.` matches `commas` (context.md C13);
      - applies the `Environment`/`Command`-vs-`Convention`/`Decision` boost;
      - caps the result and returns each entry's match reason.
      The extra `redacted` field exists because recall removes a detected
      secret from the returned statement and Task 3 must still report the item
      as redacted (context.md C13.2).
- [x] Add `retention_days` to `MemoryStoreConfig`, filled from
      `config.memory_retention_days` in `workspace_engine.rs`, so the store has
      the setting C8 needs without recall depending on a caller's config.
- [x] Add the verification transition (context.md C7): recompute the hash of
      every `RepositoryEvidence` path about to be recalled; a difference
      transitions to `EvidenceStale`, equality transitions back to
      `EvidenceCurrent`, a missing path is `EvidenceStale`, and each transition
      is audited as `memory_verification_changed`. Do not route this through
      `mark_state`.
- [x] Withdraw, do not mark, an entry whose evidence path now fails
      `PathPolicy::is_restricted` (`path_policy.rs:149`) or the store's
      `is_ignored_in_repository` helper (which applies `ignore.rs:44` plus the
      per-directory `.gitignore`): excluded from recall, retained in the store,
      reported with the reason as a `memory_recall_withdrawn` audit event
      (context.md C13.3, C13.5).
- [x] Treat a `project_key` mismatch as a hard error, not a filtered row
      (already the store's read behaviour — assert it at the recall boundary).
- [x] Make `tokenize` `pub(crate)` (context.md C1).
- [x] Tests: scope isolation at each `project_key` tier; user scope off; stale
      on change and back to current on revert; deleted path is stale, not
      deleted; restricted and ignored paths withdrawn and restored when the rule
      narrows; `Expired`/`Deleted`/`Superseded` never recalled; cap and match
      reason; retention expiry; a stored credential added directly to the memory
      file is redacted on the way out (the spec 29 §6 assertion).

**Checks:** `cargo fmt --all`; scoped clippy; `cargo nextest run -p
workspace-engine --test memory`.

**Done when:** every retrieval and lifecycle acceptance criterion that does not
need a prompt, a manifest or a UI is proven by a test on the store alone.

## Task 3 — Context assembly

**Goal:** recalled entries reach the model as ordinary `ContextItem`s in the
lowest-priority category, and the manifest records ids and match reasons.

**Files:** `crates/workspace-engine/src/context_manager.rs`,
`crates/workspace-engine/src/external_reference.rs` (date helper visibility),
`crates/workspace-engine/src/lib.rs` (re-export),
`crates/workspace-engine/src/chat.rs` and
`crates/workspace-engine/src/edit.rs` (empty slices; Task 4 fills the turn path),
the five test files that construct a `ContextRequest`, and
`crates/desktop-shell/src/lib.rs` (two manifest fixtures) — see context.md C14.

**Steps**

- [x] Add `memory: &'a [RecalledMemory]` to `ContextRequest` and build one
      `ContextItem` per entry, `kind: memory`, no path, content rendered as the
      proposal §5.2 frame: descriptive wording, provenance, date, and the
      may-be-out-of-date sentence when stale.
- [x] Redact each rendered statement through the scanner before it becomes an
      item, and record the redaction status like any other item.
- [x] Add `ContextManifest.memory: Vec<MemoryRecallRef>` with
      `#[serde(default)]` — entry id, match reason, verification — never the
      statement (context.md C3, C10). A ref exists only for an included item;
      `remove_optional` drops the matching ref (context.md C14).
- [x] Make `Memory` the first category truncated when the budget is contested
      and add the `context_budget.memory` allocation. **Already true from
      Task 1**; this task proves it with tests rather than adding code
      (context.md C14).
- [x] Update the `CategoryUsage` order and any exhaustive label map. The Rust
      side is already Task 1's; the JS map is Task 5's (context.md C2).
- [x] Tests: with the memory category removed or its share zero, no memory text
      appears in the rendered prompt; `Memory` is dropped before any other
      category under pressure; stale text contains the marker in the rendered
      string; a memory item is removable like any optional item.

**Checks:** `cargo fmt --all`; scoped clippy; the `context_manager` inline
tests plus `cargo nextest run -p workspace-engine --test prompt_cache` (the
prefix-stability guard).

**Done when:** the rendered prompt and manifest are test-proven; nothing is
wired to a live turn yet.

## Task 4 — Turn wiring and send-time re-check

**Goal:** a real turn recalls memory, audits it once at send, and never sends an
entry deleted between assembly and the call.

**Files:** `crates/workspace-engine/src/chat.rs`,
`crates/workspace-engine/src/edit.rs` (empty slice only),
`crates/workspace-engine/src/workspace_engine.rs` (pass the store to the
orchestrator).

**Steps**

- [ ] Read identity, session and prompt before assembly in `chat.rs:1250` and
      pass the recalled entries through `ContextRequest.memory`.
- [ ] Pass an empty slice from `edit.rs:526` and `chat.rs:2627` unless the
      continuation is the same turn (the continuation must keep what the turn
      recalled, not recall again).
- [ ] Call `record_recall` once, at send: the default path after
      `sink.context`, the paused path on `Send` in
      `resume_after_context_decision` (`chat.rs:1479`), never on decline.
- [ ] Re-check each memory entry's state immediately before the model call and
      drop any that is no longer `Active` and eligible, recording the exclusion
      as a dropped item with a reason (proposal §5.6). Drop its
      `manifest.memory` row too, or the inspector names a statement the request
      no longer carries (Task 3 C14).
- [ ] Tests (inline or `tests/`): a turn sends memory only as items; a
      declined review records no recall; an entry deleted between assembly and
      send is absent from the request; a continuation keeps the original
      entries.

**Checks:** `cargo fmt --all`; scoped clippy; the chat-path tests touched by the
change.

**Done when:** the turn path is complete and the only remaining work is
presentation, measurement and documentation.

## Task 5 — Inspector and shell

**Goal:** the user can see what was recalled, why, and remove it before sending.

**Files:** `crates/workspace-engine/src/chat.rs` (read route only),
`crates/desktop-shell/src/lib.rs`, `crates/desktop-shell/static/app.js`,
`crates/desktop-shell/static/app.css`.

**Steps**

- [ ] Add an engine read call that resolves a list of memory entry ids against
      the store for display: statement, scope, provenance, date, verification,
      match reason, and "deleted" for an entry that no longer exists. It must
      not expose entries the current `project_key` does not own.
- [ ] Add the shell route and handler beside the existing context routes
      (`lib.rs:338`).
- [ ] Render the `Memory` category row with its entry count and stale count, and
      each entry with its details and a remove control using the existing
      `context-remove` path and item index (`app.js:2311`, `:2373`).
- [ ] Keep removal available only while the pre-send review is paused, per
      spec 27's existing rules; post-send shows the entries without a remove
      control.
- [ ] Tests: the shell handler's JSON shape; the JS category label and stale
      count; a removed entry is absent from the resumed request and recorded as
      `UserRemoved`.

**Checks:** `cargo fmt --all`; scoped clippy; `node --check
crates/desktop-shell/static/app.js`; `npm run lint:web`; the shell handler
tests.

**Done when:** the acceptance criteria about visibility, stale count and removal
are met in the running app, or a manual walk is recorded as the remaining gap.

## Task 6 — Evaluation harness and metrics

**Goal:** the injection and staleness claims are measured, not argued, and the
two placeholder metrics carry real values.

**Files:** `crates/eval-harness/src/{scenario.rs,assertions.rs,metrics.rs,runner.rs}`,
`crates/eval-harness/scenarios/*.toml`,
`crates/eval-harness/fixtures/memory-recall/`, spec 18's §5.6.

**Steps**

- [ ] Add assertions: an entry was recalled; the rendered context contains or
      excludes a marker; the request excludes a deleted entry; the audit carries
      `memory_recalled` and `memory_verification_changed`.
- [ ] Scenarios: instruction-shaped memory still requires approval; memory does
      not widen Ask mode; memory does not outrank `AGENTS.md` or the current
      message; stale marker in the rendered prompt after the evidence file
      changes; deleted-before-send absence; project isolation; user scope off.
- [ ] Fill `memory_recall_usefulness` and `memory_correction_rate` with the
      definitions in context.md C11, remove `notApplicable: "phase-3b"` from the
      two rows, and say in spec 18 §5.6 that they are proxies.
- [ ] Run the deterministic tier: `cargo run -p eval-harness -- run --tier
      deterministic`.

**Checks:** scoped clippy for `eval-harness`; `cargo nextest run -p
eval-harness`; the direct deterministic run.

**Done when:** the scenarios pass and no existing scenario's values regressed.

## Task 7 — Documentation

**Goal:** the user-facing story matches the behaviour.

**Files:** `docs/USER_GUIDE.md`, `docs/TROUBLESHOOTING.md`, `SECURITY.md` if the
memory boundary changed, `proposal.md` §7.

**Steps**

- [ ] `USER_GUIDE.md`: how memory reaches the model, that it is the
      lowest-priority context and dropped first, how to see and remove a
      recalled entry, what "may be out of date" means, and why memory cannot
      grant a permission.
- [ ] `TROUBLESHOOTING.md`: why an entry was or was not recalled, how to read
      the match reason, how staleness is detected, what to do about a stale
      entry, and where retention is configured.
- [ ] `SECURITY.md`: the withdrawal rule (restricted or ignored evidence is
      excluded, not marked) and the redaction of statements on the way out.
- [ ] Fill `proposal.md` §7 with the defaults chosen, the measured recall over
      real sessions, and whether lexical retrieval proved sufficient.

**Checks:** `typos`; `npm run specs:check`.

**Done when:** every setting and every visible state has a place the user can
find it.

## Task 8 — Close-out

**Goal:** the spec's status is truthful everywhere and the feature is proven
end to end.

**Steps**

- [ ] Walk the proposal §6 acceptance criteria one at a time and record the
      evidence for each in `proposal.md` §7.
- [ ] Update the `Status:` line, this file's header and progress table, and
      `docs/specs/README.md`'s row and **What to build next** section.
- [ ] Remove the memory-recall line from `CHANGELOG.md`'s `Unreleased` table.
- [ ] Re-derive the `Depends on:` lines that name spec 30 and promote whatever
      they were blocking.
- [ ] Run `npm run specs:check`.
- [ ] Hand the branch back; the gate runs on CI. Record the CI run on the
      commit once it passes.
