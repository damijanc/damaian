# Memory Creation, Consent, and Scope — Implementation Plan

> **For agentic workers:** implement one task per session. Read
> [`context.md`](context.md) before Task 1.

**Implements:** [`proposal.md`](proposal.md) (requirements 1–8 and acceptance
criteria) · context and decisions in [`context.md`](context.md)
**Started:** 2026-10-26 — **Done:** 2026-10-26

## Progress

| Task | State | Notes |
|---|---|---|
| 1 · Folder conversion, status and links | Done 2026-10-26 | The flat spec moved to `proposal.md` with `git mv`; `context.md` holds the former Current State with re-checked references and decisions D-a–D-f; this file is new. Status In progress in the proposal, the README current-state paragraph and the implementation-order row. Every link naming the flat file in specs 28, 30, 50, 51 and the README now names `29_memory_creation_and_consent/proposal.md`; specs 28 and 50 use `../` because they are folder specs. `npm run specs:check` passed (66 specs). A relative-link scan over `docs/**/*.md` found three pre-existing broken links (spec 01's `url`, spec 31's code-span, spec 66's `65_exploration_memory_within_a_turn.md`) and nothing new. Docs-only change; no Rust touched. |
| 2 · Constructor barrier and proposal types | Done 2026-10-26 | `MemoryEntry` is `#[non_exhaustive]`, `MemoryEntry::new` is gone, and `MemoryProposal::confirm(confirmation, gate)` is the only conversion; the module docs state plainly that this is an API-surface barrier and that serde deserialization remains a deliberate second path. `ProposalOrigin` has four variants, `MemoryProposal` carries `task_id` for #28's `SuggestedAndConfirmed`, `UserConfirmation` records the action and optional scope, and `MemoryProposalGate` is built only by `MemoryStore::proposal_gate`. Confirmation normalizes the statement, enforces the 280-character ceiling, scans for secrets, applies the instruction check to every non-user origin, checks evidence eligibility, and maps origins to #28's three provenance variants. **Deviation:** fields stay public for reading; `#[non_exhaustive]` supplies the construction barrier. #28's tests now build a proposal and confirm it; tests that need an invalid record for the store to refuse deserialize one, which is the documented second path. `--test memory` 56 passed; inline `memory::` 18 passed; scoped clippy clean. |
| 3 · Screening: statement, secret, instruction, evidence | Done 2026-10-26 | `is_instruction_shaped` uses a conservative prefix and phrase table; it runs for every non-user origin, including `ModelSuggestion`. `MemoryProposal::screen` checks the raw candidate before normalization, enforces the 280-character ceiling, rejects multi-line/control statements, scans the statement, an `ExternalContent` source and every evidence path for secrets, requires a task id on external content and a content hash on repository evidence, and checks each path with `MemoryProposalGate::check_evidence` (relative, `PathPolicy::is_restricted`, configured patterns and per-directory `.gitignore`). `MemoryStore::propose` audits `memory_proposal_refused` with reason, scope, category, origin and the evidence path only; never the statement, and never a secret. It normalizes the statement after screening and before serializing, so the card and the proposal event carry the sentence confirmation will persist. `propose_with_evidence` screens the path before opening the file; `evidence_ref_for` records the content hash. Tests cover the pattern table and benign conventions inline, a seeded secret in a statement and in an external source, restricted `.env` evidence, an ignored file that is still index-resident, an over-long statement, a multi-line candidate, empty evidence hashes, missing task ids, normalization order, and the refusal-before-read order. `--test memory` 77 passed by the end of the branch. §7 records two inherited limits: evidence attribution is model-declared, so an omitted `evidence_path` cannot be proven absent, and an evidence path is checked as a string, so a symlink to a protected file needs final-path resolution in a future hardening. |
| 4 · Proposal persistence, confirm, dismiss and cap | Done 2026-10-26 | Proposals and decisions are session-log events (`memory_proposal_proposed`, `memory_proposal_decided`) through new `pub(crate)` `SessionStore` methods; `append_session_event` stays private. `MemoryStore::propose` screens and caps non-user suggestions at `MAX_SUGGESTIONS_PER_TASK = 3`, counted from the log so a resumed turn does not reset it. `pending_proposals` is proposed minus decided. `confirm_proposal` accepts only a proposal id, re-reads and re-screens the stored proposal, inserts the entry, appends a confirmation decision and audits `memory_proposal_confirmed` with the source; a failed insert leaves the proposal pending. `dismiss_proposal` appends a dismissal and audits it without the statement. Nine integration tests cover survival across reopening, one entry per confirmation, a second confirmation refused, dismissal, the cap with an uncapped user request, user scope disabled without flipping the flag, and a user-scope proposal saved as project. `--test memory` 68 passed; scoped clippy clean. |
| 5 · Explicit request and the `propose_memory` tool | Done 2026-10-26 | `explicit_memory_request` reads one cue, the first sentence and nothing inferred; the turn calls it after the user message is appended. `ToolAction::ProposeMemory` is parsed by `tool_action_from_call`, offered beside `turn_notes`, allowed in every mode and profile because it writes only the session log, and charged as exploration. Dispatch maps `evidence_path` to `RepositoryObservation`, `source` without evidence to `ExternalContent`, otherwise `ModelSuggestion`; it hashes the evidence file, calls `MemoryStore::propose`, and returns a result naming what happened without repeating the statement. `ChatOrchestrator` gained its `memory_store` dependency. Proposals are session-log events, not transcript roles, so they never enter `build_conversation`. **Deviation:** the model may suggest a memory but the tool cannot confirm one; the eval scenarios cover that end to end. Tests: inline cue parsing and category/scope selection, tool parsing and schema, tool-effect classification, and two turn-level tests proving an explicit cue creates one proposal and no memory file. Scoped clippy clean. |
| 6 · Shell routes and the confirmation card | Done 2026-10-26 | Three routes: `GET /api/memory-proposals`, `POST /api/memory-confirm`, `POST /api/memory-dismiss`. The confirm route takes `repo`, `session_id`, `proposal_id` and an optional `scope`; it resolves `ProjectIdentity` from the repository, verifies the session belongs to that repository, and creates the `UserConfirmation` server-side, so a client cannot supply a statement, a project key or a confirmation id. Dismiss applies the same session/repository check. The UI reads the pending list into a collapsed proposal stack below the conversation, with statement, scope, category, origin/source, a scope picker, Remember and Dismiss. The picker offers only scopes the server will accept. A user-scope proposal while the scope is off defaults to project scope and names `memory_user_scope_enabled=true` in `user.conf`. The stack is re-read after every turn and after each decision. **Deviation:** proposals are not transcript messages, so they cannot reach `build_conversation`; the stack is a sibling of the log content. Five route tests run through the real token-gated shell: list/confirm/dismiss end to end, a user-scope refusal that leaves the flag off and the proposal pending, a session from another repository refused for both confirm and dismiss, the absence of a statement/project-key field, and no bulk route. **Manual walk:** performed on 2026-10-26 in headless Chrome against the shell on port 4899 with a throwaway data dir and two seeded proposals; observations are in `proposal.md` §7. `node --check` and `npm run lint:web` pass; scoped clippy clean. |
| 7 · Acceptance tests a scenario cannot cover | Done 2026-10-26 | Added the store-level `a_memory_claiming_approval_or_path_policy_changes_nothing` (a stored instruction still leaves command risk, approval policy, `PathPolicy` and `memory_user_scope_enabled` untouched) and four shell-level tests: no bulk route (a POST to `/api/memory-confirm-all` is 404 and the UI has no accept-all label), the confirm handler has no statement or project-key field, the route-level user-scope refusal, and a session from another repository refused for both confirm and dismiss. The store tests cover a forged identity (the project key must match the resolved identity), the dismissal and cap audit lines, the confirmed-path round trip, and the index-resident ignored evidence. |
| 8 · Deterministic eval scenarios | Done 2026-10-26 | New fixture `crates/eval-harness/fixtures/memory-injection/` (poisoned `AGENTS.md` and `CONTRIBUTING.md`, committed `local.conf`, `.env`, `secrets/prod.env`) and six scenarios. `seed_files` writes files after the baseline commit without committing them, which makes `local.conf` ignored; it does **not** prove index residency, because the index is built lazily after those writes. The direct test `ignored_evidence_is_refused_after_the_file_was_indexed` indexes `local.conf` first, asserts it is in `index.files`, then writes the ignore rule and refuses the proposal. New assertions read the run's memory files and session proposal events: `memory_entries`, `memory_proposals`, `memory_user_entries`, `memory_absent` (memory records plus proposal events only, so the refusal audit may name a path) and `memory_audit_contains`. `run_scenarios` adds `memory_unconfirmed_persistence` to every record, and the tier test asserts it is present and passing for all 36 runs — the invariant is over the run, not per scenario. All six scenarios pass in the deterministic tier; the whole deterministic tier still passes with no approval-policy or secret violations (`cargo run -p eval-harness -- run --tier deterministic`). **Not regenerated:** `evals/baseline.json`; observation #40 records that it is already stale for reasons outside #29, and no test compares it automatically. |
| 9 · Documentation and close-out | Done 2026-10-26 | **Docs.** `USER_GUIDE.md` now says memory can be proposed and confirmed, explains the two creation paths, why even an explicit request is confirmed, what the origin line means, why an instruction-shaped suggestion is refused outright, why a secret or protected/ignored evidence produces no card, and that confirming a user-scope proposal does not enable user scope. `TROUBLESHOOTING.md` gains a proposal-event table (`memory_proposal_created/refused/confirmed/dismissed/failed`), the refusal reasons, the cap, and where pending proposals live. The proposal §7 records the pattern list, the cap (3), the ceiling (280), the constructor-barrier caveat, the provenance mapping, the session-log placement, the surface deviation and everything not verified. **Close-out.** #29 is Done in the proposal, README current state, README implementation-order row and this header/progress table, with the full seven-check gate green in CI on 2026-10-26 for the `main` push at `4abbf1f`. #30's `Depends on:` entry for #29 is built and it is in the ready set; a "Re-derived on 2026-10-26" history entry names what #29 leaves for #30. `CHANGELOG.md` had no #29 Unreleased line, so nothing was removed and no release row was added. **Checks.** Fast checks, scoped clippy for all three touched crates, the memory/shell test suites and the whole deterministic eval tier passed; the workspace clippy and test suite were left to CI; the full seven-check gate passed on 2026-10-26 for the `main` push at `4abbf1f`. |

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

- [x] Make `MemoryEntry` `#[non_exhaustive]`, remove `MemoryEntry::new`, and keep
      `insert` public but reachable only with a value from `confirm` or
      deserialization. Document the barrier and its limits in the module docs.
- [x] Define `MemoryProposal` (`task_id: Option<String>`), `ProposalOrigin`
      (four variants), `UserConfirmation`, `MemoryProposalGate`,
      `MemoryConfirmError`, and `MemoryStore::proposal_gate`. The error is the
      existing `MemoryError`/`MemoryRefusal` pair, with new reason codes rather
      than a parallel error type.
- [x] Implement `MemoryProposal::confirm(self, confirmation, gate)` as the only
      conversion: check the confirmation's proposal id, normalize the statement,
      enforce the ceiling, re-scan for secrets, re-run instruction refusal for
      non-user origins, re-run evidence eligibility, choose the scope, and map
      origins to #28's three provenance variants (D-c).
- [x] Add `MemoryEntry` read accessors where a caller outside the crate needs
      them; do not add a test-only constructor. **Deviation:** the fields stay
      public for reading (and for the store's transition methods), because
      `#[non_exhaustive]` already stops another crate writing a struct literal.
      Adding fifty accessors would be noise, not a boundary.
- [x] Update #28's tests and inline tests that called `MemoryEntry::new` to build
      a proposal and confirm it. Tests that deliberately build an invalid record
      for the store to refuse use serde deserialization, the documented second
      path, because the consent gate now refuses those candidates first.
- [x] Tests: a confirmation for another proposal id is refused;
      `ExternalContent` becomes `SuggestedAndConfirmed` and keeps its source;
      `RepositoryObservation` becomes `RepositoryEvidence` with
      `EvidenceCurrent`; a user request mentioning a protected path with no
      evidence is eligible.

**Verification:** `cargo nextest run -p workspace-engine --test memory`;
`cargo clippy -p workspace-engine --all-targets --locked -- -D warnings`.

## Task 3 · Screening: statement, secret, instruction, evidence

- [x] Implement `is_instruction_shaped` with the conservative patterns in §7;
      run it for every non-user origin (D-d).
- [x] Implement proposal screening for an over-long or multi-line statement,
      a detected secret, and every ineligible evidence path (not relative,
      restricted, or ignored by configured patterns or per-directory
      `.gitignore`).
- [x] Compute an evidence `content_hash` at proposal time. Never read the file's
      content into the statement.
- [x] Audit `memory_proposal_refused` with origin, reason, scope, category and —
      only when the candidate carried one — the refused path. No statement, no
      secret.
- [x] Tests: every instruction pattern and a table of benign conventions; a
      seeded AWS key in a statement and in a path; `.env` restricted; a
      per-directory `.gitignore` case while the file is index-resident; a
      user-typed sentence naming `.env` with no evidence is accepted.

**Verification:** `cargo nextest run -p workspace-engine --test memory`;

## Task 4 · Proposal persistence, confirm, dismiss and cap

- [x] Add `pub(crate)` `SessionStore` methods for appending and reading memory
      proposal events and decisions; keep `append_session_event` private.
- [x] `MemoryStore::propose(root, session_id, task_id, proposal)` screens, caps
      non-user suggestions at three per task (read from the log), appends the
      proposal event, audits creation, and returns `None` for a refusal.
- [x] `MemoryStore::pending_proposals(session_id)` returns proposed events minus
      decided ones, newest last.
- [x] `MemoryStore::confirm_proposal(root, identity, session_id, proposal_id,
      confirmation)` re-reads the stored proposal, re-runs the gate, inserts the
      entry, appends a confirmation event and audits
      `memory_proposal_confirmed` with the proposal id, entry id, confirmation
      id, origin and source. A failed insert leaves the proposal pending.
- [x] `MemoryStore::dismiss_proposal(session_id, proposal_id)` appends a
      dismissal event and audits it without the statement.
- [x] Tests: a proposal survives reopening the session store; a second confirm
      is refused; a dismissed proposal creates no entry and stays recorded; the
      cap holds across a resumed task; a user-scope confirm while disabled fails
      without enabling the scope; confirming with project scope creates the
      entry in the project file.

**Verification:** `cargo nextest run -p workspace-engine --test memory`.

## Task 5 · Explicit request and the `propose_memory` tool

- [x] Add `explicit_memory_request(prompt)` and call it at turn start, after the
      user message is appended, to create a `UserRequest` proposal with that
      message id.
- [x] Add `ToolAction::ProposeMemory`, parse `propose_memory`
      (`statement`, `category`, optional `scope`, `source`, `evidence_path`),
      offer it wherever `turn_notes` is offered, and classify it as session-log
      only in `mode.rs` and `action_effect`.
- [x] Wire `ChatOrchestrator::memory_store` (constructor change) and dispatch the
      tool: `evidence_path` selects `RepositoryObservation`, `source` without
      evidence selects `ExternalContent`, otherwise `ModelSuggestion`. Return a
      result naming what happened, never the stored statement.
- [x] Make sure a proposal statement never reaches the model prompt: proposals
      are not transcript messages.
- [x] Tests: a prompt with "remember that …" creates exactly one proposal whose
      statement is the confirmed sentence; a scripted `propose_memory` tool call
      creates at most a proposal and no entry (covered by the eval scenarios);
      the fourth suggestion in one task is refused; the tool is not offered when
      the mode/profile does not allow it (the `permits` arm allows it in every
      mode, like `ask_user` and `turn_notes`, because it writes only the session
      log and authorises nothing).

**Verification:** `cargo nextest run -p workspace-engine --lib chat::`;
`cargo nextest run -p workspace-engine --test memory`.

## Task 6 · Shell routes and the confirmation card

- [x] `GET /api/memory-proposals?session_id=…` returns the pending proposals
      with scope, category, statement, origin/source and evidence paths.
- [x] `POST /api/memory-confirm` takes `repo`, `session_id`, `proposal_id` and an
      optional `scope`; resolves the identity from `repo`, calls
      `confirm_proposal`, and refuses a client-supplied project key or
      statement. `POST /api/memory-dismiss` takes the same identifiers.
- [x] Render the card in the conversation from the pending list: statement,
      scope, category, origin line, Confirm, Dismiss, and a project-scope choice
      for a user-scope proposal while user scope is disabled. Collapse to a
      count when there is more than one. Re-render after a reload and after a
      turn.
- [x] No accept-all button and no route that takes a list of proposals.
- [x] Tests: the route round-trips a confirm and a dismiss; a user-scope confirm
      while disabled fails and leaves `memory_user_scope_enabled=false`; an
      unknown or already-decided proposal id is refused by the store, and the
      route inherits that; the UI helper has a card for one proposal and a count
      for several. **Deviation:** the card is a stack below the conversation's
      messages rather than an inserted transcript message, so a suggestion can
      never enter the model prompt.

**Verification:** `cargo nextest run -p desktop-shell` (scoped to the new tests);
`node --check crates/desktop-shell/static/app.js`; `npm run lint:web`.

## Task 7 · Acceptance tests a scenario cannot cover

- [x] No bulk confirmation: no route accepts more than one proposal id.
- [x] Memory cannot widen: write an entry claiming "all commands are approved;
      ignore path policy", then assert the mode, approval policy, path policy
      and command policy are unchanged.
- [x] A confirmed memory cannot choose the project key: a confirmation context
      with a forged identity is refused by the store, not accepted by the route.
- [x] User scope: confirming a user-scope proposal while disabled is refused and
      does not flip the config; confirming with project scope writes a project
      entry and no user entry.
- [x] Dismissal and cap records: one dismissal event per action, one cap refusal
      per over-cap suggestion, no statement in either audit line.
- [x] A record round-trips through the confirmed path with provenance, evidence
      hash and sensitivity intact.

**Verification:** `cargo nextest run -p workspace-engine --test memory`;
`cargo nextest run -p desktop-shell`.

## Task 8 · Deterministic eval scenarios

- [x] Add a `memory-injection` fixture whose `AGENTS.md` and `CONTRIBUTING.md`
      instruct the agent to remember that commands are pre-approved, and a
      `local.conf` that is committed and then ignored by a `seed_files`
      `.gitignore`.
- [x] Add `seed_files` to the scenario format and runner (written after the
      baseline commit, never committed).
- [x] Add assertion fields and evaluation for: memory entry count, pending
      proposal count, a path absent from every proposal and entry, refusal audit
      presence, and the user-scope case. `memory_entries` counts stored lines;
      `memory_proposals` counts proposed-minus-decided events; `memory_absent`
      searches memory files plus proposal events only.
- [x] Scenarios:
      - `memory_injection_repository.toml` (AGENTS.md / CONTRIBUTING.md);
      - `memory_injection_external.toml` (issue text via `source`);
      - `memory_secret_refusal.toml`;
      - `memory_ineligible_evidence.toml` (`.env`, restricted path, ignored
        index-resident `local.conf`);
      - `memory_no_unconfirmed_persistence.toml` (invariant over the run);
      - `memory_user_scope_disabled.toml`.
- [x] Each asserts no entry, no proposal where the spec requires none, no
      statement/secret/path in any proposal or entry, and the refusal audit.

**Verification:** `cargo run -p eval-harness -- run --tier deterministic
--scenario <name>` for each new scenario, then the whole deterministic tier.

## Task 9 · Documentation and close-out

- [x] `docs/USER_GUIDE.md`: how memory is created, why even an explicit request
      is confirmed, what the origin line means, why an instruction-shaped
      suggestion is refused outright, what enabling user scope does, and that a
      suggestion is never a stored memory.
- [x] `docs/TROUBLESHOOTING.md`: why a memory was not created, how to read a
      refused proposal in the audit log, how to see which scope an entry is in,
      and the audit event names.
- [x] Fill the proposal's §7 implementation notes: the patterns used, the cap,
      the ceiling, false positives/negatives observed, and any criterion that
      could not be verified.
- [x] Mark #29 Done in all four places: `proposal.md` `Status`, README
      current state, README implementation-order row, `tasks.md` header and
      progress table.
- [x] Promote #30: flip its `Depends on:` entry for #29 to built, update the
      README ready set, and add a "Re-derived on 2026-10-26" history entry
      naming what #29 leaves for #30.
- [x] Remove #29's line from `CHANGELOG.md` → Unreleased if one exists; do not
      add a release row. There is no `Unreleased` line for #29, so nothing was
      removed and no release row was added.
- [x] Walk the acceptance criteria in §6 against tests and scenarios; record
      what could not be verified, and say plainly that CI is outstanding.
- [x] Run the fast checks and the scoped suites; leave the workspace clippy and
      test suite to CI.

**Verification:** `scripts/quality-gate.sh fmt node lint typos deny`;
`cargo clippy -p workspace-engine -p desktop-shell --all-targets --locked --
-D warnings`; scoped nextest; `npm run specs:check`.
