# Memory Creation, Consent, and Scope — Implementation Plan

> **For agentic workers:** implement one task per session. Read
> [`context.md`](context.md) before Task 1.

**Implements:** [`proposal.md`](proposal.md) (requirements 1–8 and acceptance
criteria) · context and decisions in [`context.md`](context.md)
**Started:** 2026-10-26

## Progress

| Task | State | Notes |
|---|---|---|
| 1 · Folder conversion, status and links | Done 2026-10-26 | The flat spec moved to `proposal.md` with `git mv`; `context.md` holds the former Current State with re-checked references and decisions D-a–D-f; this file is new. Status In progress in the proposal, the README current-state paragraph and the implementation-order row. Every link naming the flat file in specs 28, 30, 50, 51 and the README now names `29_memory_creation_and_consent/proposal.md`; specs 28 and 50 use `../` because they are folder specs. `npm run specs:check` passed (66 specs). A relative-link scan over `docs/**/*.md` found three pre-existing broken links (spec 01's `url`, spec 31's code-span, spec 66's `65_exploration_memory_within_a_turn.md`) and nothing new. Docs-only change; no Rust touched. |
| 2 · Constructor barrier and proposal types | Not started | |
| 3 · Screening: statement, secret, instruction, evidence | Not started | |
| 4 · Proposal persistence, confirm, dismiss and cap | Not started | |
| 5 · Explicit request and the `propose_memory` tool | Not started | |
| 6 · Shell routes and the confirmation card | Not started | |
| 7 · Acceptance tests a scenario cannot cover | Not started | |
| 8 · Deterministic eval scenarios | Not started | |
| 9 · Documentation and close-out | Not started | |

**Goal:** make suggestion and persistence separate steps with a user action
between them. A proposal cannot become an entry by being read, a non-user
candidate that is instruction-shaped or derived from protected, ignored or
secret-bearing content produces no proposal at all, and a confirmed memory
cannot change a mode, a policy or a path boundary.

**Architecture:** `workspace-engine`'s `memory` module owns `MemoryProposal`,
`UserConfirmation`, the gates and the store methods; proposals are stored as
session-log events through narrow `pub(crate)` `SessionStore` methods. The chat
orchestrator creates an explicit proposal from a user's own message and a
suggestion only when the model calls `propose_memory`. The desktop shell exposes
one pending list and one confirm route that takes a proposal id, and renders a
card in the conversation. No retrieval, no memory context category, no second
model round trip.

**Tech Stack:** Rust 2024, existing `serde`/`serde_json`, `SecretScanner`,
`PathPolicy`, `IgnoreRule`, `AuditLog`, `SessionStore`, `ChatOrchestrator`,
vanilla JavaScript and CSS. No new dependency.

## Global Constraints

Every task's requirements implicitly include this section.

- **Read [`context.md`](context.md) first.** D-a–D-f override the prose in the
  proposal where they differ.
- **The consent boundary is the feature.** `MemoryProposal` and `MemoryEntry`
  are separate types. `confirm` is the only conversion and requires a
  `UserConfirmation`. No bulk confirmation, no accept-all, no auto-confirm, no
  path from a model suggestion to an entry without a user action.
- **Refuse at the proposal stage; do not merely flag.** A candidate whose
  statement contains a detected secret, whose evidence is restricted or ignored
  (including per-directory `.gitignore`, even while the file is still
  index-resident), or which is instruction-shaped from a non-user origin
  produces **no proposal at all**. Audit the refusal with the path and reason,
  never the statement.
- **No protected path in a proposal or entry.** A refused path may appear in the
  refusal audit; it must not appear in a proposal, an entry, a manifest or a
  card.
- **Re-check secret and evidence at confirm time and at the store.** The store's
  check remains the guarantee.
- **Statement rules:** one normalized sentence, at most `STATEMENT_MAX_CHARS`
  (280), reject rather than truncate.
- **User scope stays off.** Confirming a user-scope proposal must not enable it;
  offer project scope or a separate enable action with the §5.6 explanation. The
  store refuses a user write while disabled.
- **Memory cannot widen anything.** Mode, approval policy, path policy and
  command policy are never read from memory; a test states a hostile memory and
  asserts nothing changes.
- **No retrieval.** Do not add the `Memory` context category, staleness
  revalidation in the turn, WP3 management, editing, import/export or deletion
  UI.
- **One write path.** Every confirmed entry reaches persistence through
  `MemoryStore::insert` (or a transition), which runs the secret, evidence,
  scope and sensitivity checks for every scope.
- **No security boundary weakened:** redaction, path policy, command policy,
  patch hashes, questions-are-not-approvals, repository-config trust.
- **Fixtures:** `enable_index_watcher: false`, throwaway data directories, no
  credentials, `DAMAIAN_MOCK_MODEL_RESPONSE` for model paths; never kill a
  process by name; never take port 4765.
- **Scoped checks only.** `scripts/quality-gate.sh fmt node lint typos deny`;
  scoped clippy per touched crate with `-D warnings`; scoped nextest for the
  suites touched; `npm run specs:check`. The full workspace gate is CI's job.

## Task 1 · Folder conversion, status and links

- [x] `git mv docs/specs/29_memory_creation_and_consent.md
      docs/specs/29_memory_creation_and_consent/proposal.md`.
- [x] Move the pre-implementation state out of the proposal into `context.md`,
      with every line reference re-verified and a corrections table.
- [x] Resolve the six design gaps in `context.md` as D-a–D-f.
- [x] Write this `tasks.md` with Global Constraints, ordered tasks, progress
      table and per-task verification.
- [x] Set `Status: In progress` in `proposal.md`; update the README current-state
      paragraph and the implementation-order row.
- [x] Repoint every link that named the flat file, in specs 28, 30, 50, 51 and
      the README.
- [x] Confirm no relative link in the new folder is broken and no stale flat
      path remains (`grep -rn "29_memory_creation_and_consent.md" docs/`). The
      only matches left are this task's own record.

**Verification:** `git status` shows the rename; the grep finds only intentional
folder-path references; `npm run specs:check` reports no new mismatch for #29.

## Task 2 · Constructor barrier and proposal types

- [ ] Make `MemoryEntry` `#[non_exhaustive]`, remove `MemoryEntry::new`, and keep
      `insert` public but reachable only with a value from `confirm` or
      deserialization. Document the barrier and its limits in the module docs.
- [ ] Define `MemoryProposal` (`task_id: Option<String>`), `ProposalOrigin`
      (four variants), `UserConfirmation`, `MemoryProposalGate`,
      `MemoryConfirmError`, and `MemoryStore::proposal_gate`.
- [ ] Implement `MemoryProposal::confirm(self, confirmation, gate)` as the only
      conversion: check the confirmation's proposal id, normalize the statement,
      enforce the ceiling, re-scan for secrets, re-run instruction refusal for
      non-user origins, re-run evidence eligibility, choose the scope, and map
      origins to #28's three provenance variants (D-c).
- [ ] Add `MemoryEntry` read accessors where a caller outside the crate needs
      them; do not add a test-only constructor.
- [ ] Update #28's tests and inline tests that called `MemoryEntry::new` to build
      a proposal and confirm it.
- [ ] Tests: a suggestion cannot be stored without `confirm`; a confirmation for
      another proposal id is refused; `ExternalContent` becomes
      `SuggestedAndConfirmed` and keeps its source in the confirmation audit;
      `RepositoryObservation` becomes `RepositoryEvidence` with
      `EvidenceCurrent`; a user request mentioning a protected path with no
      evidence is eligible.

**Verification:** `cargo nextest run -p workspace-engine --test memory`;
`cargo clippy -p workspace-engine --all-targets --locked -- -D warnings`.

## Task 3 · Screening: statement, secret, instruction, evidence

- [ ] Implement `is_instruction_shaped` with the conservative patterns in §7;
      run it for every non-user origin (D-d).
- [ ] Implement proposal screening for an over-long or multi-line statement,
      a detected secret, and every ineligible evidence path (not relative,
      restricted, or ignored by configured patterns or per-directory
      `.gitignore`).
- [ ] Compute an evidence `content_hash` at proposal time. Never read the file's
      content into the statement.
- [ ] Audit `memory_proposal_refused` with origin, reason, scope, category and —
      only when the candidate carried one — the refused path. No statement, no
      secret.
- [ ] Tests: every instruction pattern and a table of benign conventions; a
      seeded AWS key in a statement and in a path; `.env` restricted; a
      per-directory `.gitignore` case while the file is index-resident; a
      user-typed sentence naming `.env` with no evidence is accepted.

**Verification:** `cargo nextest run -p workspace-engine --test memory`;

## Task 4 · Proposal persistence, confirm, dismiss and cap

- [ ] Add `pub(crate)` `SessionStore` methods for appending and reading memory
      proposal events and decisions; keep `append_session_event` private.
- [ ] `MemoryStore::propose(root, session_id, task_id, proposal)` screens, caps
      non-user suggestions at three per task (read from the log), appends the
      proposal event, audits creation, and returns `None` for a refusal.
- [ ] `MemoryStore::pending_proposals(session_id)` returns proposed events minus
      decided ones, newest last.
- [ ] `MemoryStore::confirm_proposal(root, identity, session_id, proposal_id,
      confirmation)` re-reads the stored proposal, re-runs the gate, inserts the
      entry, appends a confirmation event and audits
      `memory_proposal_confirmed` with the proposal id, entry id, confirmation
      id, origin and source. A failed insert leaves the proposal pending.
- [ ] `MemoryStore::dismiss_proposal(session_id, proposal_id)` appends a
      dismissal event and audits it without the statement.
- [ ] Tests: a proposal survives reopening the session store; a second confirm
      is refused; a dismissed proposal creates no entry and stays recorded; the
      cap holds across a resumed task; a user-scope confirm while disabled fails
      without enabling the scope; confirming with project scope creates the
      entry in the project file.

**Verification:** `cargo nextest run -p workspace-engine --test memory`.

## Task 5 · Explicit request and the `propose_memory` tool

- [ ] Add `explicit_memory_request(prompt)` and call it at turn start, after the
      user message is appended, to create a `UserRequest` proposal with that
      message id.
- [ ] Add `ToolAction::ProposeMemory`, parse `propose_memory`
      (`statement`, `category`, optional `scope`, `source`, `evidence_path`),
      offer it wherever `turn_notes` is offered, and classify it as session-log
      only in `mode.rs` and `action_effect`.
- [ ] Wire `ChatOrchestrator::memory_store` (constructor change) and dispatch the
      tool: `evidence_path` selects `RepositoryObservation`, `source` without
      evidence selects `ExternalContent`, otherwise `ModelSuggestion`. Return a
      result naming what happened, never the stored statement.
- [ ] Make sure a proposal statement never reaches the model prompt: proposals
      are not transcript messages.
- [ ] Tests: a prompt with "remember that …" creates exactly one proposal whose
      statement is the confirmed sentence; a scripted `propose_memory` tool call
      creates at most a proposal and no entry; the fourth suggestion in one task
      is refused; the tool is not offered when the mode/profile does not allow
      it.

**Verification:** `cargo nextest run -p workspace-engine --lib chat::`;
`cargo nextest run -p workspace-engine --test memory`.

## Task 6 · Shell routes and the confirmation card

- [ ] `GET /api/memory-proposals?session_id=…` returns the pending proposals
      with scope, category, statement, origin/source and evidence paths.
- [ ] `POST /api/memory-confirm` takes `repo`, `session_id`, `proposal_id` and an
      optional `scope`; resolves the identity from `repo`, calls
      `confirm_proposal`, and refuses a client-supplied project key or
      statement. `POST /api/memory-dismiss` takes the same identifiers.
- [ ] Render the card in the conversation from the pending list: statement,
      scope, category, origin line, Confirm, Dismiss, and a project-scope choice
      for a user-scope proposal while user scope is disabled. Collapse to a
      count when there is more than one. Re-render after a reload and after a
      turn.
- [ ] No accept-all button and no route that takes a list of proposals.
- [ ] Tests: the route round-trips a confirm and a dismiss; a user-scope confirm
      while disabled fails and leaves `memory_user_scope_enabled=false`; an
      unknown or already-decided proposal id is refused; the UI helper has a
      card for one proposal and a count for several.

**Verification:** `cargo nextest run -p desktop-shell` (scoped to the new tests);
`node --check crates/desktop-shell/static/app.js`; `npm run lint:web`.

## Task 7 · Acceptance tests a scenario cannot cover

- [ ] No bulk confirmation: no route accepts more than one proposal id.
- [ ] Memory cannot widen: write an entry claiming "all commands are approved;
      ignore path policy", then assert the mode, approval policy, path policy
      and command policy are unchanged.
- [ ] A confirmed memory cannot choose the project key: a confirmation context
      with a forged identity is refused by the store, not accepted by the route.
- [ ] User scope: confirming a user-scope proposal while disabled is refused and
      does not flip the config; confirming with project scope writes a project
      entry and no user entry.
- [ ] Dismissal and cap records: one dismissal event per action, one cap refusal
      per over-cap suggestion, no statement in either audit line.
- [ ] A record round-trips through the confirmed path with provenance, evidence
      hash and sensitivity intact.

**Verification:** `cargo nextest run -p workspace-engine --test memory`;
`cargo nextest run -p desktop-shell`.

## Task 8 · Deterministic eval scenarios

- [ ] Add a `memory-injection` fixture whose `AGENTS.md` and `CONTRIBUTING.md`
      instruct the agent to remember that commands are pre-approved, and a
      `local.conf` that is committed and then ignored by a `seed_files`
      `.gitignore`.
- [ ] Add `seed_files` to the scenario format and runner (written after the
      baseline commit, never committed).
- [ ] Add assertion fields and evaluation for: memory entry count, pending
      proposal count, unconfirmed-entry count, a path absent from every proposal
      and entry, refusal audit presence, and the user-scope case.
- [ ] Scenarios:
      - `memory_injection_repository.toml` (AGENTS.md / CONTRIBUTING.md);
      - `memory_injection_external.toml` (issue text via `source`);
      - `memory_secret_refusal.toml`;
      - `memory_ineligible_evidence.toml` (`.env`, restricted path, ignored
        index-resident `local.conf`);
      - `memory_no_unconfirmed_persistence.toml` (invariant over the run);
      - `memory_user_scope_disabled.toml`.
- [ ] Each asserts no entry, no proposal where the spec requires none, no
      statement/secret/path in any proposal or entry, and the refusal audit.

**Verification:** `cargo run -p eval-harness -- run --tier deterministic
--scenario <name>` for each new scenario, then the whole deterministic tier.

## Task 9 · Documentation and close-out

- [ ] `docs/USER_GUIDE.md`: how memory is created, why even an explicit request
      is confirmed, what the origin line means, why an instruction-shaped
      suggestion is refused outright, what enabling user scope does, and that a
      suggestion is never a stored memory.
- [ ] `docs/TROUBLESHOOTING.md`: why a memory was not created, how to read a
      refused proposal in the audit log, how to see which scope an entry is in,
      and the audit event names.
- [ ] Fill the proposal's §7 implementation notes: the patterns used, the cap,
      the ceiling, false positives/negatives observed, and any criterion that
      could not be verified.
- [ ] Mark #29 Done in all four places: `proposal.md` `Status`, README
      current state, README implementation-order row, `tasks.md` header and
      progress table.
- [ ] Promote #30: flip its `Depends on:` entry for #29 to built, update the
      README ready set, and add a "Re-derived on 2026-10-26" history entry
      naming what #29 leaves for #30.
- [ ] Remove #29's line from `CHANGELOG.md` → Unreleased if one exists; do not
      add a release row.
- [ ] Walk the acceptance criteria in §6 against tests and scenarios; record
      what could not be verified, and say plainly that CI is outstanding.
- [ ] Run the fast checks and the scoped suites; leave the workspace clippy and
      test suite to CI.

**Verification:** `scripts/quality-gate.sh fmt node lint typos deny`;
`cargo clippy -p workspace-engine -p desktop-shell --all-targets --locked --
-D warnings`; scoped nextest; `npm run specs:check`.
