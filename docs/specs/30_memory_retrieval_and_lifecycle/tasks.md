# Spec 30 — Tasks

Implementation tasks for
[`proposal.md`](proposal.md). Read the proposal's §3 requirements and §5 design,
and [`context.md`](context.md) for the pre-implementation state and the
corrections planning found, before starting any task.

**Started:** 2026-10-28 (planned; no task implemented yet)

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
- **No new write path.** Retrieval may call `insert` never; it calls
  `mark_state`, a new verification-transition method, and `record_recall` only.
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
| 1 · Types and configuration | Not started | — |
| 2 · Recall engine | Not started | — |
| 3 · Context assembly | Not started | — |
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
`crates/workspace-engine/tests/permission_profiles.rs`.

**Steps**

- [ ] Add `ContextCategory::Memory` after `ConversationSummary` in
      `context_manager.rs:37`, with the doc comment that it is lowest priority
      and dropped first.
- [ ] Add `ContextKind::Memory` and its `as_str` (`"memory"`) and `category`
      arms; update `BUDGETED`, `budget_index`, the `shares` array type and the
      default array to eleven entries, and the oversubscription test that
      assumes ten.
- [ ] Add `context_budget_memory: Option<u16>` and
      `memory_max_entries: Option<u32>` to `ConfigOverlay`, the built-in config
      field and parser, and `memory_retention_days: Option<u64>` as a user-only
      key. Fund the new share by lowering an existing one; never raise the
      total. Spec 51 adds `external_reference` to the same array on a parallel
      branch and pays for it out of `search_results`, so re-read the defaults
      after that lands rather than assuming `main`'s table.
- [ ] Classify all three: the two shares/caps are restrict-only `Preference`
      keys; `memory_retention_days` is forbidden to repository and profile, like
      `memory_user_scope_enabled` (`profile.rs:428`).
- [ ] Add `rule_label` text for each key and extend
      `tests/permission_profiles.rs` with the three classifications: a
      repository raise is refused by name, a repository retention value is
      refused, and a profile may lower the cap and the share but not raise them.
- [ ] Update `Config::validate_context_budget` and its test so eleven shares
      still total at most 10,000.

**Checks:** `cargo fmt --all`; `cargo clippy -p workspace-engine --all-targets
--locked -- -D warnings`; `cargo nextest run -p workspace-engine --test
permission_profiles`; the `config` inline tests.

**Done when:** the workspace compiles with `Memory` in every match, and the
config tests prove the three keys' direction and scope.

## Task 2 — Recall engine

**Goal:** `MemoryStore` can answer "what applies to this task, and does it still
hold?" without any UI or prompt code.

**Files:** `crates/workspace-engine/src/memory.rs`,
`crates/workspace-engine/src/indexer.rs` (`tokenize` visibility), tests in
`crates/workspace-engine/tests/memory.rs`.

**Steps**

- [ ] Add `RecalledMemory { entry, match_reason, verification }` and a
      `MemoryStore::recall(root, identity, session_id, prompt, explicit_paths,
      cap)` that:
      - filters by scope (session for this session, project for this
        `project_key`, user only when the scope is enabled);
      - filters `Active` only, before ranking (requirement 9);
      - expires entries whose own `MemoryExpiry` or `memory_retention_days` has
        passed, through `mark_state(.., Expired)` (context.md C8);
      - scores term overlap with `tokenize` over the statement against the
        prompt plus `explicit_paths`;
      - applies the `Environment`/`Command`-vs-`Convention`/`Decision` boost;
      - caps the result and returns each entry's match reason.
- [ ] Add the verification transition (context.md C7): recompute the hash of
      every `RepositoryEvidence` path about to be recalled; a difference
      transitions to `EvidenceStale`, equality transitions back to
      `EvidenceCurrent`, a missing path is `EvidenceStale`, and each transition
      is audited. Do not route this through `mark_state`.
- [ ] Withdraw, do not mark, an entry whose evidence path now fails
      `PathPolicy::is_restricted` (`path_policy.rs:149`) or
      `is_ignored_by_rules` (`ignore.rs:44`): excluded from recall, retained in
      the store, reported with the reason.
- [ ] Treat a `project_key` mismatch as a hard error, not a filtered row
      (already the store's read behaviour — assert it at the recall boundary).
- [ ] Make `tokenize` `pub(crate)` (context.md C1).
- [ ] Tests: scope isolation at each `project_key` tier; user scope off; stale
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

**Files:** `crates/workspace-engine/src/context_manager.rs`.

**Steps**

- [ ] Add `memory: &'a [RecalledMemory]` to `ContextRequest` and build one
      `ContextItem` per entry, `kind: memory`, no path, content rendered as the
      proposal §5.2 frame: descriptive wording, provenance, date, and the
      may-be-out-of-date sentence when stale.
- [ ] Redact each rendered statement through the scanner before it becomes an
      item, and record the redaction status like any other item.
- [ ] Add `ContextManifest.memory: Vec<MemoryRecallRef>` with
      `#[serde(default)]` — entry id, match reason, verification — never the
      statement (context.md C3, C10).
- [ ] Make `Memory` the first category truncated when the budget is contested
      and add the `context_budget.memory` allocation.
- [ ] Update the `CategoryUsage` order and any exhaustive label map.
- [ ] Tests: with the memory category removed or its share zero, no memory text
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
      as a dropped item with a reason (proposal §5.6).
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
