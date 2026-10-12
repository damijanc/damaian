# Provider Fallback Consent Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Implements:** [`proposal.md`](proposal.md)
**Started:** 2026-10-07 — **Done:** 2026-10-12

## Progress

| Task | State | Notes |
|---|---|---|
| 1 · The offer and the decline (engine) | Done 2026-10-07 | `Config::fallback_provider_for` (C2, C3) and the pause in the refusal arm, through a new `pause_for_provider_fallback`, after all of spec 48's bookkeeping. `resume_after_fallback_decision` handles decline. Approve returns `InvalidInput` and puts the offer back until Task 2. Also landed: the command-resume guard, `PausedTurns::provider_fallback_target`, `ReattachedApproval::ProviderFallback`, and two audit events, `provider_fallback_offered` and `provider_fallback_decision`. **Deviations:** (a) `TurnProposals` is unchanged: the pause returns from the refusal arm directly, so only `ChatTurnResult` gained a field. (b) `ProviderRefusal::from_code` was added in `error.rs`, because `ProviderRefusal` is not serialisable and the decline must return the refusal. (c) The config tests live in `tests/provider_limits.rs`, since `config.rs` has no inline test module. (d) `desktop-shell/src/recovery.rs` gained a minimal `ProviderFallback` arm (kind `provider_fallback`, payload `{proposalId, fallbackProvider}`) so the workspace keeps compiling. No card renders it yet; that is Task 3. (e) Two tests beyond the plan: `a_quota_exhaustion_with_only_a_shared_key_fails_closed` and `a_fallback_offer_whose_paused_turn_is_gone_fails_on_restart`. (f) The tests were written after the code, not before. As a substitute for the red step, each guard was broken and the tests watched fail: the offer turned off (7 failures), the quota key check removed (3), the command-resume guard removed (1, which ran an empty command for 7.6s). **For Task 2:** the pending turn stores the *refused* round and its messages as sent, so approval re-enters at `pending.round`. The offer text is in `ChatTurnResult.response` only and is never appended to the session. Nothing guards a second offer yet: an approved turn that is refused again would be offered again, which Task 2's `provider_fallback_used` closes. **Checks:** `provider_limits` 22/22. The scoped run (provider_limits, crash_recovery and recovery_prompt binaries, plus fallback/reattach/refusal/recovery tests in the engine and shell) 120/120. fmt, clippy on `-p workspace-engine -p desktop-shell --all-targets -D warnings`, and typos all clean. |
| 2 · Approval: re-issue, record, offer once (engine) | Done 2026-10-09 | The approve arm of `resume_after_fallback_decision` enforces C1: an engine whose active provider is not the offered fallback gets `InvalidInput`, and the offer is put back. It then hands off to a new `reissue_through_fallback`, which audits `provider_fallback_decision` (`approved`, with the fallback's model), writes a `provider_fallback` task event via `SessionStore::record_provider_fallback`, appends a `system` transcript message (`"Switched to <fallback> (<model>) for this turn after <refused> refused it (<code>)."`), clears pending approvals, sets `PreparingContext`, and re-enters `run_agentic_turn` at `pending.round` with `provider_fallback_used = true`. The refusal arm skips the offer when that flag is set. `SessionStore::read_provider_fallbacks` reads the switches back from all events, not only active ones: a rewind does not change which model answered a billed call. **Deviations:** (a) `ChatTurnOptions.provider_fallback_used` is `pub`. A crate-private field cannot be filled by `..Default::default()` from another crate, which broke the shell's literal. Setting it can only suppress an offer, the fail-closed direction. The two existing literals (`desktop-shell/src/lib.rs`, `tests/foundation.rs`) gained `..ChatTurnOptions::default()`. That is a one-line shell edit ahead of Task 3, needed to compile. (b) The transcript marker uses role `system`. Earlier turns reach the model as plain `role: content` lines in `build_model_prompt`, so it reads as a fact, and it says "switched to" because it is written before the fallback answers. (c) `the_next_turn_uses_the_configured_provider_again` became `a_later_turn_is_offered_again_rather_than_switched`. It asserts the next turn's request goes to `primary` *and* that a refusal there gets a fresh offer, so neither the switch nor the once-per-turn flag leaks into the next turn. (d) As in Task 1, tests came after the code. The red step was replaced by breaking each guard: approving with `provider_fallback_used = false` fails `a_fallback_that_also_refuses_…`, `pending.round + 1` fails `an_approved_offer_reissues_…` (via the audited `toolRound`), and dropping the C1 check fails `an_approval_from_an_engine_still_on_the_refused_provider_…`. **For Task 3:** the shell must build the approving engine with `config_for_repo_with_provider(repo, Some(target))`. The marker is an ordinary `system` session message, which `app.js` must render on reload. **Checks:** `provider_limits` 28/28. The wider scoped run (provider_limits, crash_recovery, recovery_prompt, foundation and checkpoint_wiring binaries, plus fallback/reattach/refusal/recovery tests in the engine and shell) 290/290. Eval harness deterministic tier: 16/16 scenarios pass. fmt, clippy on `-p workspace-engine -p desktop-shell --all-targets -D warnings`, and typos all clean. |
| 3 · Shell, frontend, docs, close the spec | Done 2026-10-12 | `POST /api/resume-fallback-stream` → `run_resume_fallback_request`, whose core `resume_fallback_decision` takes the engine and adapter builders as arguments so a test can watch what is built. It reads `PausedTurns::provider_fallback_target` first, so an unknown id errors before any key. Approve: `config_for_repo_with_provider(repo, Some(target))` (C1) and that config's key. Decline: the configured engine, no key, and a `NoModelCall` adapter that errors if called. `always` is ignored. `chat_result_json` gained `fallbackProposal` (both providers and models, `refusalCode`, `message`, no `allowAlways`). `app.js`: `createFallbackOffer` from `appendProposals` and from the recovery card (kind `provider_fallback`), with "Keep it failed" primary and focused, "Answer with <fallback>" secondary, and no always. An approval appends the marker live, in the engine's words. A reload renders the persisted `system` message through the existing transcript path, with no new code. A decline treats the stream's error event as the outcome. Docs: USER_GUIDE "Provider Limits and Retries" and a TROUBLESHOOTING section on why no offer appeared, plus the audit rows. The spec is closed: status records, README re-derivation (nothing promoted, ready set #23, #25, #32, #57), Parallel work bullet dropped, spec 48 §7 and Status pointed at the shipped behaviour, CHANGELOG Unreleased line removed. **Deviations:** (a) `switch_model_provider` was factored out of `config_for_repo_with_provider`, so the tests switch provider with the shell's own code. (b) The recovery card names only the fallback, because `PausedTurns` exposes nothing else and this task does not edit `chat.rs`. (c) The tests came after the code. The red step was replaced by making the decline resolve a key, which fails `a_declined_offer_fails_the_task_and_resolves_no_key`. (d) The browser check needed a known API token, which the standalone binary only hands out over Tauri IPC. A throwaway `examples/` binary served `run_server` with a fixed token, and it was deleted afterwards. **Browser check** (own port 4911, own `DAMAIAN_DATA_DIR`, mock provider with `primary` → 402 and `fallback` → SSE; only my PIDs were killed): the card appeared with the focus on Keep it failed. Declining failed the task with no fallback request. An open offer came back as a recovery card after a restart, and approving it continued on `fallback`. A live approval showed the marker, and a reload rendered it again. Every new turn hit `primary` first. **Found, not fixed:** (1) A declined turn leaves only the user message on reload, because the offer text was never persisted. That is the same as any spec 48 refusal. (2) `apply_model_provider_defaults` keeps the refused provider's `model_name` when the fallback lists no models and has no built-in config, so an approval would send the wrong model name. `fallback_model` is then empty in the offer. **Gate:** fmt, clippy `--workspace --all-targets -D warnings` (one `type_complexity` fixed with an `AdapterFor` alias), nextest 1191/1191 (24 skipped), `node --check`, `lint:web` (one pre-existing info in `scripts/check-spec-status.mjs`), typos, and cargo deny all pass. `specs:check` passes (57 specs). |

**Goal:** Turn spec 48's fail-closed refusal into a choice. On a terminal
refusal, when a different configured provider has its own credentials, pause
the turn on a one-shot, refused-by-default offer naming both providers. On
approval, re-issue the refused model call through the fallback and record the
switch. On decline, fail exactly as spec 48 does.

**Architecture:** The refusal arm of `run_agentic_turn` pauses the turn like a
plan review does: it saves a `PendingChatTurn` carrying a new
`provider_fallback` field, records a `"provider_fallback"` pending approval, and
returns `Ok` with a `fallback_proposal`. A new
`resume_after_fallback_decision` either fails the task (decline) or re-enters
`run_agentic_turn` at the *same* round with the same messages (approve). The
caller supplies an engine whose in-memory config has the fallback provider
active. The shell already builds one per request (`config_for_repo_with_provider`),
and nothing is written back, which is what "not remembered" means.

**Tech Stack:** Rust 2024, no new dependencies. Vanilla JS in `app.js`.

## Global Constraints

Every task's requirements implicitly include this section.

- **Parallel work: #56 edits `chat.rs`.** It may pair with #25, but with no
  other spec that edits `chat.rs` (see `docs/specs/README.md` → Parallel work).
  Tasks 1 and 2 edit `chat.rs`. Task 3 does not.
- **Parallel worktrees: private target dir.** Every cargo command runs from the
  worktree root with `CARGO_TARGET_DIR="$PWD/target"`, for example
  `CARGO_TARGET_DIR="$PWD/target" cargo nextest run -p workspace-engine --test provider_limits`.
  Never share a target directory with another worktree.
- **Read the corrections below before starting.** They came from reading the
  code on 2026-10-07. Where a correction contradicts the proposal, the
  correction wins, and the task that acts on it updates `proposal.md` in the
  same change.
- **Fail closed stays the default.** No candidate, an offer already made this
  turn, or a decline all end in spec 48's exact failure: marker `"refused"`,
  measured zero (or the mid-stream estimate), `provider_refusal` audit, and
  `failureKind`. The offer is inserted *after* that bookkeeping. It never
  replaces it.
- **No "always", nothing remembered.** No config write, no allow-list, no
  session flag that outlives the turn. The next turn uses the configured
  provider again.
- **The engine never sees a key.** Candidate selection reads `api_key_env`
  names only. Resolving the key is the shell's job.

## Corrections to the proposal found while planning

- **C1 — Swapping the adapter alone sends the wrong request.**
  `run_agentic_turn` takes the request's `provider`, `model` and `max_tokens`
  from `self.config` (`chat.rs:1768–1777`), and it reads
  `supports_native_tools()` (`chat.rs:1508`), the context budget, and the audit
  `provider`/`model` fields from there too. Proposal §5.1's "the caller passes
  the fallback adapter" is therefore not enough. The caller must build the
  *engine* from a config with the fallback provider active, and
  `resume_after_fallback_decision` refuses an approval when
  `self.config.model_provider != pending.fallback_provider` (it puts the
  pending turn back first). The shell's existing
  `config_for_repo_with_provider` (`desktop-shell/src/lib.rs`) does exactly
  this, in memory.
- **C2 — Candidates are other providers, never a second model on the same
  one.** §5.2 permits a second model on the same provider for rate limiting.
  Requirement 1 and §5.2's own first sentence require a different provider id,
  and the config has no notion of per-model credentials. This spec offers
  other providers only. For `QuotaExhausted` the candidate must also have a
  different `api_key_env` from the active provider (requirement 7: "its own
  credentials"). Two ids that share one key are one account. Selection is
  deterministic: the first match in `model_providers` order.
- **C3 — Native-tool history may not transfer.** The pending messages were
  built for the refused provider. If the turn has already fed back native tool
  calls and the candidate's `supports_native_tools` differs, the fallback
  request carries tool messages it was not built for. The candidate filter
  therefore also requires the same `supports_native_tools` as the active
  provider. This is conservative. Relax it only with a test showing a
  mismatched history is accepted.
- **C4 — The CLI is out of scope.** `damaian-cli` never calls
  `ask_with_session` or any `resume_after_*` method. Its model calls do not go
  through the chat orchestrator, so there is no CLI turn to pause. Proposal
  §2's mention of the CLI is about where adapters are built, not about where
  the offer surfaces.
- **C5 — An existing test inverts.** `no_fallback_happens_without_approval`
  (`tests/provider_limits.rs:294`) configures a second provider and asserts the
  task *fails*. After Task 1 the same setup pauses. The test keeps its name
  and its guarantee (no switch without approval): it asserts
  `waiting_for_approval`, a `fallback_proposal` naming both providers, and
  exactly one call on the refusing adapter. A new test,
  `a_refusal_with_no_candidate_still_fails_closed`, takes over the
  fail-closed assertion with a single provider configured.

## `chat.rs` functions this spec changes

| Function / item | Task | Change |
|---|---|---|
| `struct ChatTurnOptions` (`chat.rs:366`) | 2 | Gains `#[serde(default)] pub provider_fallback_used: bool`. It is already persisted in every `PendingChatTurn`, so the "once per turn" guard survives a later command pause and a restart within the same turn. Callers that build `ChatTurnOptions` by hand get `false`. |
| `pub struct ChatTurnResult` (`chat.rs:320`) | 1 | Gains `fallback_proposal: Option<AgentFallbackProposal>` (`TurnProposals` stays unchanged: the pause returns from the refusal arm directly, see the Task 1 row). That is a new public struct: `id`, `refused_provider`, `refused_model`, `fallback_provider`, `fallback_model`, `refusal_code`, `message`. Every `ChatTurnResult { .. }` literal gains `fallback_proposal: None`. |
| `struct PendingChatTurn` (`chat.rs:3424`) | 1 | Gains `#[serde(default)] provider_fallback: Option<PendingProviderFallback>`. Every `PendingChatTurn { .. }` literal gains `provider_fallback: None`: the plan-review site (~2187) and the command/MCP/web sites (~2320, ~2650, ~2757). |
| `run_agentic_turn` (`chat.rs:1482`), the `Err(ClientError::Provider(..))` arm (`chat.rs:1879–1928`) | 1 (pause), 2 (guard) | After the existing marker, usage and audit bookkeeping: if `!turn_options.provider_fallback_used` and `self.config.fallback_provider_for(&refusal)` is `Some`, it saves the pending turn (same `round`, `messages` as sent, no `matched_tool_call`), calls `note_pending_approvals` with kind `"provider_fallback"` and `session_store.await_approval`, and returns `Ok(ChatTurnResult)` directly, not through `terminal`. Otherwise it fails as today. Returning early also skips `seal_turn_checkpoint`, which is correct: the turn is not over. |
| `resume_after_command_decision_with_options` (`chat.rs:805`) | 1 | A guard beside the existing `plan_review` one: a pending turn with `provider_fallback` is put back and refused with `InvalidInput`, so it can never be run as a shell command. |
| `resume_after_plan_decision` (`chat.rs:1114`) | 1 | No code change needed. Its `plan_review.is_none()` guard already refuses a fallback pending turn. Task 1 pins this with a test. |
| `resume_after_fallback_decision` (new, `pub`) | 1 (decline), 2 (approve) | Signature mirrors `resume_after_plan_decision`: `(proposal_id, approved, approved_by, model_adapter, sink) -> Result<ChatTurnResult>`. **Decline** (Task 1): `update_task_status_with_kind(Failed, message, refusal_code)`, audit `provider_fallback_decision` (`declined`), clear pending approvals, return `Err(ClientError::Provider(..))`. That is spec 48's exact outcome. **Approve** (Task 2): enforce C1, record the switch, set status `PreparingContext`, then `run_agentic_turn(.., pending.round, ..)` with `provider_fallback_used = true`. |
| `PausedTurns` (`chat.rs:3496`) | 1 | Gains `provider_fallback_target(proposal_id) -> Option<String>`, read as loose JSON like `plan_review_deferred_action`. The shell uses it to choose which provider's engine and key to build. Recovery uses it to reattach. |
| `has_pending_chat_command` (`chat.rs:1415`) | — | Unchanged. The shell's fallback route checks `PausedTurns::provider_fallback_target` instead. |

Outside `chat.rs`: `config.rs` (`Config::fallback_provider_for`, Task 1),
`session.rs` (a `provider_fallback` task event, Task 2),
`recovery.rs` (a `"provider_fallback"` arm in `reattach_pending_approvals`,
Task 1), `desktop-shell/src/lib.rs`, `desktop-shell/src/recovery.rs`, and
`desktop-shell/static/app.js` (Task 3).

## Task 1: The offer and the decline (engine)

**Requirements:** 1, 2, 3, 5, 7. **Proposal:** §5.1, §5.2, §5.3, §5.5 (no
candidate; crash between offer and decision). **Corrections:** C2, C3, C5.
**Files:** `config.rs`, `chat.rs`, `recovery.rs`, `tests/provider_limits.rs`.

- [x] **Step 1: Write the failing tests**
  - `config.rs`, unit tests on `Config::fallback_provider_for(&ProviderRefusal)`:
    `no_candidate_when_only_the_active_provider_is_configured`,
    `a_provider_with_an_empty_key_env_is_not_a_candidate`,
    `quota_exhaustion_skips_a_provider_sharing_the_active_key_env`,
    `rate_limiting_accepts_a_provider_sharing_the_key_env` (C2: same env, different id, not quota),
    `a_candidate_must_match_native_tool_support` (C3), and
    `the_first_matching_provider_in_config_order_is_chosen`.
  - `tests/provider_limits.rs`:
    `a_refusal_with_no_candidate_still_fails_closed` (C5);
    `no_fallback_happens_without_approval` rewritten per C5;
    `a_terminal_refusal_with_a_candidate_pauses_naming_both_providers`
    (status `waiting_for_approval`, pending approval kind `provider_fallback`,
    marker still `"refused"`, usage still measured zero, `provider_refusal`
    still audited);
    `a_transient_refusal_that_later_succeeds_never_offers` (429 then 200: no
    pause, requirement 2);
    `a_quota_exhausted_offer_names_only_a_provider_with_its_own_credentials`;
    `a_declined_offer_fails_with_the_refusals_failure_kind`;
    `a_fallback_pending_turn_cannot_be_resumed_as_a_command_or_a_plan`
    (both wrong resumes error, and the pending file is still there afterwards);
    `a_pending_fallback_reattaches_on_restart` (through
    `reattach_pending_approvals`; a missing pending file fails the task as
    `Unavailable`).
- [x] **Step 2: Run them and confirm they fail** for the expected reason.
- [x] **Step 3: `Config::fallback_provider_for`** (C2, C3). It is pure, reads
      only `model_providers`, the active provider's entry (or its built-in
      config), and names. No key resolution.
- [x] **Step 4: Types.** `PendingProviderFallback`, `AgentFallbackProposal`,
      the `PendingChatTurn` / `TurnProposals` / `ChatTurnResult` fields, and the
      literal updates listed in the table above.
- [x] **Step 5: Pause in the refusal arm**, after the existing bookkeeping.
      Leave the `Err` path textually intact as the `else`.
- [x] **Step 6: `resume_after_fallback_decision`, decline arm.** The approve
      arm returns `InvalidInput("not yet implemented")` until Task 2. Put the
      pending turn back before returning it, so Task 2's test can drive the
      same file.
- [x] **Step 7: Guards and accessor.** The command-resume guard, and
      `PausedTurns::provider_fallback_target`.
- [x] **Step 8: Recovery.** Add a `"provider_fallback"` arm to
      `reattach_pending_approvals` returning a new
      `ReattachedApproval::ProviderFallback { task_id, proposal_id, fallback_provider }`.
      Keep the match exhaustive and do not use a wildcard. Fix the
      `PendingApprovalRef.kind` doc comment (`session.rs:221`), which already
      omits `"plan"`.
- [x] **Step 9: Falsify.** Drop the quota key-env check and confirm the quota
      test fails. Make the arm offer even when `provider_fallback_used`
      (temporarily hard-code `false`) and confirm nothing in Task 1 notices.
      That gap is Task 2's test. Revert both.
- [x] **Step 10: Update `proposal.md`** for C2, C3, C4 and C5 (§2, §5.2, §7).
- [x] **Step 11: Scoped checks.** Run
      `CARGO_TARGET_DIR="$PWD/target" cargo nextest run -p workspace-engine --test provider_limits`,
      `CARGO_TARGET_DIR="$PWD/target" cargo nextest run -p workspace-engine -E 'test(fallback) + test(reattach)'`,
      then `cargo fmt --all`,
      `CARGO_TARGET_DIR="$PWD/target" cargo clippy -p workspace-engine --all-targets --locked -- -D warnings`
      and `typos`. Update this file's Progress row and the `Started` header.
      Then show the change and ask.

## Task 2: Approval: re-issue, record, offer once (engine)

**Requirements:** 4, 6, and §5.5 "the fallback also refuses". **Proposal:**
§5.1, §5.4, §5.5. **Corrections:** C1. **Files:** `chat.rs`, `session.rs`,
`tests/provider_limits.rs`.

- [x] **Step 1: Write the failing tests** (`tests/provider_limits.rs`; each
      builds a second `WorkspaceEngine` from a config with the fallback
      provider active, the way the shell will):
  - `an_approved_offer_reissues_the_refused_call_through_the_fallback`. The
    fallback adapter receives one request, its `provider`/`model` are the
    fallback's, its messages equal the refused request's, and the turn
    completes.
  - `an_approval_from_an_engine_still_on_the_refused_provider_is_rejected` (C1).
    `InvalidInput`, the adapter is not called, and the pending turn is kept.
  - `an_approved_switch_is_recorded_on_the_task_and_in_the_transcript`. There
    is a `provider_fallback` event with both ids, a transcript marker message,
    and a `provider_fallback_decision` audit row (`approved`).
  - `a_fallback_that_also_refuses_fails_without_a_second_offer`. Configure a
    third provider so a candidate exists. The task fails with the second
    refusal's `failureKind`, and no new pending turn is created.
  - `the_plan_survives_an_approved_switch`. A plan set before the refusal is
    read back unchanged after the fallback turn (requirement 4).
  - `the_next_turn_uses_the_configured_provider_again` (not remembered).
- [x] **Step 2: Run them and confirm they fail.**
- [x] **Step 3: `ChatTurnOptions.provider_fallback_used`**, and the guard in
      the refusal arm.
- [x] **Step 4: `SessionStore::record_provider_fallback(task, from, to)`**, a
      thin public wrapper appending a `provider_fallback` task event, following
      `await_approval`'s payload shape. For the transcript marker, append a
      plain session message on approval, before the re-issued call:
      `"Answered with <fallback> after <refused> refused (<code>)."` Task 3
      renders it. A system-role message must not reach the model's history as
      an instruction, so check `bounded_messages`/`build_model_prompt` and use
      whatever role the existing session notes use.
- [x] **Step 5: Approve arm** of `resume_after_fallback_decision`: the C1
      check (put back on mismatch), then record, audit, clear pending
      approvals, set `PreparingContext`, and call `run_agentic_turn` at
      `pending.round` (not `+ 1`: the round never completed) with
      `provider_fallback_used = true`.
- [x] **Step 6: Falsify.** Pass `provider_fallback_used = false` on approve and
      confirm the double-refusal test fails. Swap `pending.round` for
      `pending.round + 1` and confirm the round or budget assertion notices; if
      nothing notices, add an assertion that does. Revert both.
- [x] **Step 7: Eval harness.** This changes model-dependent recovery, so run
      `CARGO_TARGET_DIR="$PWD/target" cargo run -p eval-harness -- run --tier deterministic`.
- [x] **Step 8: Scoped checks** as in Task 1 Step 11. Update the Progress
      row. Then show the change and ask.

## Task 3: Shell, frontend, docs, close the spec

**Requirements:** 1, 3, 6 (user-visible halves); all acceptance criteria.
**Files:** `desktop-shell/src/lib.rs`, `desktop-shell/src/recovery.rs`,
`desktop-shell/static/app.js`, `docs/USER_GUIDE.md`,
`docs/TROUBLESHOOTING.md`, `CHANGELOG.md`, `docs/specs/README.md`,
`proposal.md`. This task does not edit `chat.rs`.

- [x] **Step 1: Shell tests first** (`desktop-shell/src/lib.rs` tests):
      the turn JSON carries `fallbackProposal` with both providers and no
      `always` field; `/api/resume-fallback-stream` with `approved=false`
      fails the task and builds no transport; an unknown `proposal_id` errors
      before any key is resolved.
- [x] **Step 2: Route `POST /api/resume-fallback-stream`**, modelled on
      `run_resume_plan_request`. Read `PausedTurns::provider_fallback_target`.
      On approve, build the engine with
      `config_for_repo_with_provider(repo, Some(target))` (C1), resolve *that*
      provider's key with `resolve_model_api_key`, and build the adapter from
      the switched config. On decline, use `engine_for_repo` and resolve no
      key. Ignore any `always` form field.
- [x] **Step 3: `chat_result_json`** (`lib.rs:3286`) gains `fallbackProposal`.
      The crash-recovery card in `desktop-shell/src/recovery.rs` handles
      `ReattachedApproval::ProviderFallback`.
- [x] **Step 4: `app.js`.** A fallback card beside `createPlanReview`. It
      names both providers and the refusal, with a primary "Keep it failed"
      (focused by default) and a secondary "Answer with <fallback>". There is
      no "always". Add the resume call beside the `/api/resume-plan-stream`
      one, and render the transcript marker message. Run `npm run lint:web`
      and `node --check crates/desktop-shell/static/app.js`.
- [x] **Step 5: Verify in the browser.** Run your own shell instance on a
      port other than 4765 with its own `DAMAIAN_DATA_DIR`, and track its PID.
      Drive a refusal with `DAMAIAN_MOCK_MODEL_RESPONSE` or a mock transport.
      Confirm the card, decline, approve, the marker, and reattach after a
      restart. Kill only your PID.
- [x] **Step 6: Docs.** `USER_GUIDE.md` "Provider Limits and Retries": the
      offer, that it is one-shot and declined by default, and that it never
      remembers. `TROUBLESHOOTING.md`: why no offer appeared (no candidate, an
      empty key env, a shared key env on quota, a native-tools mismatch, or
      already offered this turn) and the `provider_fallback` event and audit
      rows.
- [x] **Step 7: Close the spec** (AGENTS.md "When a spec becomes Done"). Fill
      in `proposal.md` §7 Implementation Notes and set its Status to Done.
      Update the README row, remove #56 from "What to build next" and the
      ready set, and drop it from the Parallel work bullets. Point spec 48's
      §7 note at the shipped behaviour. Remove the "Offer to answer a refused
      turn…" line from `CHANGELOG.md` Unreleased. Run
      `grep -rn "56_provider_fallback_consent" docs/specs/*.md docs/specs/*/*.md`
      and `npm run specs:check`.
- [x] **Step 8: Full quality gate**, all seven commands from `AGENTS.md`, with
      `CARGO_TARGET_DIR="$PWD/target"` on each cargo command. Record the
      results in the Progress row and set `**Started:** … — **Done:** …`.
      Then show the change and ask.
