# Context: Memory Creation, Consent, and Scope

Background for [`tasks.md`](tasks.md). Read this before Task 1, with
[`proposal.md`](proposal.md) §5 beside it. Everything below was checked against
the repository at `main` on 2026-10-26, before this branch changed code. Line
numbers drift: re-read the function before editing near it.

## 1. What exists, and what does not

[Spec 28](../28_memory_model_and_storage/proposal.md) is built (Done
2026-10-26). It provides the record, the project identity and the store this
spec extends:

- `MemoryEntry`, `MemoryProvenance` (exactly three variants), `MemoryScope`,
  `MemoryCategory`, `MemoryState`, `Verification`, `Sensitivity`, `EvidenceRef`
  and `MemoryExpiry` are in `crates/workspace-engine/src/memory.rs`.
- `MemoryStore` is the single writer for every scope. `insert`
  (`memory.rs:975`) runs shape, secret, evidence, sensitivity and persistence
  in that order. Session writes reach the log only through
  `SessionStore::append_memory_record` (`session.rs:1117`); `append_session_event`
  stays private.
- `memory_user_scope_enabled` defaults to `false` and is Forbidden at repository
  and profile scope (`config.rs:269`, overlay handling around `:1756`).
- The project identity resolver is `ProjectIdentity::resolve`
  (`memory.rs:481`); `WorkspaceEngine` already exposes `pub memory_store`
  (`workspace_engine.rs:56`).

What does not exist yet:

- No `MemoryProposal`, `UserConfirmation`, or proposal persistence.
- No creation path at all: `MemoryEntry::new` (`memory.rs:255`) is public and
  the entry's fields are public, so any caller can build one and call
  `MemoryStore::insert`.
- No model tool, no suggestion trigger, no confirmation card, no shell route,
  and no memory UI.
- `ChatOrchestrator` (`chat.rs:675`) has no `memory_store` field;
  `WorkspaceEngine::build` constructs the store before the orchestrator
  (`workspace_engine.rs:145` and `:171`), so wiring it is a constructor change,
  not a new dependency.
- No approved proposal storage exists. The closest precedents are
  `record_question_asked` (`session.rs:1577`), the session-scoped browser
  diagnostic approval (`session.rs:1077`), and the pending-turn store
  (`chat.rs:8962`).

The pre-implementation "Current State" from the flat spec, with every line
reference re-checked:

- **No memory creation path exists.** Correct. Spec 28 defines only the record
  and store.
- **`MemoryProvenance` has no model-inference variant.** Correct: three
  variants, `UserRequest`, `SuggestedAndConfirmed` and `RepositoryEvidence`
  (`memory.rs:154`).
- **Secret refusal is available.** `SecretScanner::contains_secrets`
  (`secret_scanner.rs:65`) and `scan` (`secret_scanner.rs:27`) return findings
  without transforming; only `redact` (`secret_scanner.rs:42`) transforms.
- **User scope is off by default and enforced on read and write.** Correct
  (`memory.rs:938` for reads, `memory.rs:1156` and `:1286` for writes).
- **Instruction precedence is established.** Spec 11 defines `AGENTS.md`
  ordering; memory must sit below all of it.
- **Approval UI patterns exist.** Command approval (`validation.rs`, `app.js`
  around `createCommandApprovalPreview`) is the card shape to imitate.
- **Session-scoped approval has a precedent.**
  `allow_browser_diagnostics_for_session` is at `session.rs:1077-1108`, not the
  `:261-291` the flat spec named.
- **The repository watcher exists** in `index_cache.rs`; #30 uses it. #29 only
  records evidence.

## 2. Corrections to the flat spec's line references

| Flat spec says | Current location | Note |
|---|---|---|
| `secret_scanner.rs:65` / `:27` | same | Unchanged. |
| `path_policy.rs:138` `is_restricted` | `path_policy.rs:149` | Applies `restricted_rules`. |
| `ignore.rs:44` `is_ignored_by_rules` | same | `parse_ignore_patterns` is `ignore.rs:15`. |
| `session.rs:261-291` session-scoped approval | `session.rs:1077-1108` | Struct and store moved. |
| `indexer.rs:218-226` ignore-check gap | `indexer.rs:244-251` | `index_single_file` checks only default/configured patterns; the doc comment at `:224-228` says so. |
| `indexer.rs:201-204` doc comment | `indexer.rs:224-228` | Same claim, moved. |
| `patch_engine.rs:291` hash comparison | `patch_engine.rs:334-335` | Same mechanism; #28 reuses it. |
| `context_manager.rs:260` redaction | `context_manager.rs:2252` and `:1298` | Not touched by #29. |

The substantive parts of the flat spec still hold: instruction-shaped
candidates from non-user origins must produce no proposal; protected and
ignored evidence must produce no proposal even while index-resident; the store
is the guarantee.

## 3. Design decisions taken before code

These resolve the tensions named in the task brief. They are decisions, not
discoveries; later tasks must follow them.

**D-a — The constructor barrier is `#[non_exhaustive]` plus no `new`, not
private fields.** `MemoryEntry` keeps its public fields for reading and for the
store's transition methods, but the struct becomes `#[non_exhaustive]`, so a
different crate (the shell, the CLI, the eval harness) cannot write a struct
literal. `MemoryEntry::new` is removed. The ordinary ways to obtain an entry are
`MemoryProposal::confirm` and `serde` deserialization. `Deserialize` is public,
so any crate can deliberately deserialize a record; `MemoryStore::insert` is
public and re-runs the store's shape, secret, evidence, scope and sensitivity
checks on whatever it receives. This is an API-surface barrier, not a sandbox:
an in-process caller that deliberately deserializes a record can still build
one, because the store itself must. A `#[doc(hidden)]` or `#[cfg(test)]`
constructor is deliberately not added — a test-only bypass must not compile into
the shipped app.

**D-b — Identity and session enter through `MemoryProposalGate`, supplied by
the store.** The proposal carries `session_id` in its origin and `task_id` where
#28 needs one. `confirm(self, confirmation, gate)` takes a gate built by
`MemoryStore` from the repository root, the resolved `ProjectIdentity`, the
scanner, the path policy and the ignore patterns. The client supplies only a
proposal id and a scope choice; the confirm route resolves the identity from
`repo` and never accepts a project key from the request.

**D-c — External content confirms as `SuggestedAndConfirmed`, not a fourth
provenance variant.** #28 says exactly three variants and §5.1 gives
`SuggestedAndConfirmed` a task id. A `MemoryProposal` therefore carries
`task_id: Option<String>` for every non-`UserRequest` origin. Confirming an
`ExternalContent` proposal produces
`MemoryProvenance::SuggestedAndConfirmed { session_id, task_id }`; the source
stays on the proposal and is written to the confirmation audit event. The
source is never silently dropped, and no model-inference variant is added.
`RepositoryObservation` confirms as `RepositoryEvidence` with its paths and
hashes and `EvidenceCurrent`; all other origins confirm as `UserConfirmed`.

**D-d — The instruction-shape check applies to every non-user origin, including
`ModelSuggestion`.** A poisoned `CONTRIBUTING.md` can cause the model to call
`propose_memory` without a `source`, which would otherwise become a
trusted-looking `ModelSuggestion` card. Attributing the source from the turn is
not reliable — the model chooses the label — so the conservative
instruction-pattern refusal runs on `ModelSuggestion`,
`RepositoryObservation` and `ExternalContent` alike. The branch is keyed on
"user request or not", not on which untrusted label was chosen, so relabelling
cannot bypass it. A false negative cannot slip through that path because every
non-user proposal reaches the same function before a card is appended; the
injection eval is the arbiter. False positives are accepted: the user can state
the same convention themselves, and that arrives as a trusted `UserRequest`.

**D-e — A pending proposal lives in the session log.** Proposed and decided
events are appended through new `pub(crate)` `SessionStore` methods, mirroring
`append_memory_record`. A proposal survives a reload and a restart, dies with
its session, and never becomes an entry by being read. The confirm route takes
`session_id` and `proposal_id`, re-reads the stored proposal and re-runs the
gate. A dismissed proposal appends a dismissal event and an audit
`memory_proposal_dismissed`; it leaves no entry, and the proposal remains in the
log as history. The card is rendered from the pending list, never from
client-supplied text.

**D-f — The suggestion trigger is a model tool during the turn, capped at
three per task.** `propose_memory` is dispatched like `ask_user` and
`turn_notes`; it cannot store, only propose. The cap is read from the session
log, so a resumed turn does not get a fresh allowance. An explicit "remember
that …" is parsed from the user's own message at turn start and is not capped.
No second model round trip and no post-task prose scan.

## 4. What #29 must not touch

- **Context assembly and `ContextCategory`.** Do not add the `Memory` category;
  #30 owns retrieval. The proposal's own statement must not be inserted into a
  model prompt.
- **The prompt conversation window.** Do not make proposals transcript
  messages. They are joined into the session payload separately, so they cannot
  reach `build_conversation` and become context.
- **Retrieval, staleness revalidation, WP3 management, editing, import/export
  and deletion UI.** #29 ends at proposal, confirmation, the stored entry and
  its evidence.
- **Mode, approval policy, path policy and command policy must not consult
  memory.** The `memory-cannot-widen` test states a hostile memory and asserts
  nothing changes.
- **Repository config trust.** Do not let a repository set
  `memory_user_scope_enabled` or a memory creation path.

## 5. Contracts from the neighbours

- **#28 §5.1:** exactly three provenance variants; `Sensitivity` is derived by
  rule and raise-only; a detected secret is refused, not redacted.
- **#28 §5.5:** `EvidenceRef` is a path and a hash, never content; an evidence
  path must be repository-relative and eligible.
- **#26 §5.5:** the manifest is content-free. #29 must not add a manifest field
  or a memory context category.
- **#27:** the context inspector shows statement, scope, provenance, date,
  verification and match reason. #29 must not add a second inspection surface
  for entries.
- **#10:** approval cards are specific, exact-scope, and have no broad pattern.
  A memory proposal is one statement, one action.
- **#23:** the completion report owns the end of a task. A suggestion must not
  compete with it; it is a separate, collapsed count.
- **#20/#31:** mode and permission are not consultable from memory.

## 6. Test fixtures and manual walk

- Follow `tests/memory.rs`'s conventions: a per-test temp directory, real local
  Git repositories where identity matters, `enable_index_watcher: false`, and a
  `data_dir` inside the temp tree. Never touch the user's application-support
  state.
- Eval scenarios use `crates/eval-harness/fixtures/`. The protected/ignored case
  needs a file that is committed and then ignored, so the scenario format gains
  `seed_files`: files written after the baseline commit, without committing
  them. That exercises the ignore rule, but it does **not** prove index
  residency: the index is built lazily after seed files are written. A direct
  store test indexes the file first, then writes the rule, and proves that case.
- The manual UI walk uses port 4899 and a throwaway `DAMAIAN_DATA_DIR`. The
  Tauri window and a real model call are acceptable gaps; record what was and
  was not walked.
