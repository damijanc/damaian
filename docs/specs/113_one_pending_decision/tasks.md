# Spec 113 tasks

**Started:** 2026-11-21

Implementation plan for [`proposal.md`](proposal.md). The evidence, the code
survey and the decisions are in [`context.md`](context.md).

## Global constraints

- One task, one session (AGENTS.md).
- No change to what needs approval, and no new route that resumes a paused
  turn. A typed message never approves (requirement 5).
- Tasks 1 and 3 both edit `workspace-engine/src/chat/`. Task 2 edits `app.js`.
  Run them one at a time.
- Scoped checks per task; the workspace gate stays with CI. UI changes are
  checked in the running app (`serves_the_ui_for_manual_inspection` on its own
  port and data dir, never 4765, stopped by PID).

| Task | State | Durable result |
|---|---|---|
| 1 · One pending decision per session (req 3, D3, D4) | Done 2026-11-21 | New `workspace-engine/src/pending_decisions.rs`: `settle_older_pending_decisions` (every other `WaitingForApproval` task of the session: paused turn taken through `PendingCommandStore::take` via `chat::take_superseded_paused_turn`, status `Cancelled`, audit `pending_decision_superseded` with `sessionId`, `taskId`, `supersededBy`, `kind` — the approval kind, `question`, or `unknown` for a link-less pause — and `pausedTurnTaken`) and `keep_newest_pending_decision` (newest by creation order). Both return early for a session with a worker record (spec 57). **Hook:** not inside `SessionStore::await_approval`/`await_answer`, which hold no `AuditLog`; instead `ChatOrchestrator::await_approval`/`await_answer` wrap them and every chat pause site calls those (agentic turn, context review, repair question, fallback and round budget, both check batches), plus the one engine pause outside chat (`edit.rs`, the CLI edit patch). A new pause site must use the wrapper. The sweep: `reattach_pending_approvals` calls `keep_newest_pending_decision` first, so the shell's `run_sweep` needed no change. **Deviations:** "older" is implemented as every other waiting task of the session (the pause just raised is by definition the newest decision). A patch pause saves no paused turn, so its proposal stays in the patch store as history (`pausedTurnTaken: false`); a paused-turn file that no longer parses as `PendingChatTurn` is left on disk by `take` (orphaned, never reattached, since its task is `Cancelled`). **Readers of `WaitingForApproval`**, checked: sidebar `json::session_state` keys on the newest task's status, and `cancelled` maps to no state; `/api/pending-card` (`read_pending_approval`) and `/api/session`'s `pendingDecision` (`TaskControlService::pending_decision`) return nothing unless waiting; crash classification skips terminal `Cancelled`; the sweep reattaches only waiting tasks. A settled task drops out of each. **Tests** (`tests/one_pending_decision.rs`, 3): the live incident shape (A's plan review settled when B pauses: A `Cancelled`, file gone, audit names B, `reattach` returns only B, approving A's review afterwards errors), a three-pause pile keeps the newest after the sweep (another session's pause untouched; a second sweep settles nothing), and a worker session left alone; each asserts nothing was approved, written or run. Mutation-checked: disabling the settle fails the first two, dropping the worker guard fails the third. **Checks:** `cargo clippy -p workspace-engine --all-targets --locked -- -D warnings` clean; `cargo nextest run -p workspace-engine --locked` 2729 run, all pass (the 25 loopback-binding tests in `external_reference`/`foundation` rerun outside the sandbox); `cargo nextest run -p desktop-shell --locked` 353 passed; `scripts/quality-gate.sh fmt node lint typos deny` ok; deterministic eval tier 65/65 pass. Desktop-shell code is unchanged. Workspace clippy and tests left to CI. |
| 2 · What typing does while a decision waits (req 1, 2, D1, D2) | Done 2026-11-21 | All in `app.js`. **Plan:** `createPlanReview` (not a worker's card) registers its decline in a `planReviewDeclines` WeakMap and gets class `plan-review`; the hook opens the card's own "How should the plan be changed?" field, puts the text there and calls the card's `decide(false, text)` — the one `streamResumePlanRequest` → `/api/resume-plan-stream` call with `approved=false` and `changes`, so no new route and the engine's `request_plan_changes` audit is the card's. It never approves. `planDeclineFor` finds the review in the pending card itself or inside the recovery card that wraps it after a restart. `sendChatPrompt`, after the worker, steering-note and busy-elsewhere branches and only for the composer's own text (no `promptOverride`), sends it through `declinePendingPlanWithChanges`, which clears the composer (the card keeps the text if the request fails; if the card is already sending, the text goes back to the composer). **Lock:** `syncComposerQuestionLock` became `syncComposerLock` over `composerDecisionState`: a plan gives placeholder "Describe changes to the plan" with the composer enabled; every other kind disables `#chat-prompt` and Send with a placeholder from `COMPOSER_LOCK_PLACEHOLDERS` (question "Answer the question" unchanged; command "Approve or reject the command first"; patch, checks, fetch, round budget, fallback, context review each named; an unknown kind locks with "Answer the card above first"). `sendChatPrompt` also returns early for a locked kind, so a Send that slips past the disabled field is never a new turn. Answering any card unlocks it: `resumeDecision` (and a patch's `onResolved`) settle the decision, which runs `syncComposerStatus`. Steering notes are untouched (a running turn makes `pendingDecisionApplies` null). **Deviations:** like the question lock, both behaviours need the decision's card on screen (`frame`); a pending decision with no card (an unreadable `/api/pending-card`) leaves the composer as before, so a typed message starts a turn — Task 1 now settles the older pause when that turn pauses. The name `syncComposerDecision…` was avoided because spec 112's test forbids the removed `syncComposerDecision`. **Tests** (`src/tests.rs`): `app_js_typing_over_a_pending_plan_declines_it_with_changes` and `app_js_other_pending_decisions_lock_the_composer` (every `PENDING_DECISION_FIELDS` kind but `plan` must have a placeholder); `app_js_composer_names_the_pending_decision_and_yields_to_a_question` updated for the rename. **Checks:** `scripts/quality-gate.sh fmt node lint typos deny` ok (Biome's one warning is the existing `style.css` specificity one); `cargo clippy -p desktop-shell --all-targets --locked -- -D warnings` clean; `cargo nextest run -p desktop-shell --locked` 355 passed. **In the app** (`serves_the_ui_for_manual_inspection` on 4911, its own data dir, stopped by PID): the fixture's seeded pending plan (a recovery card after restart) showed "Describe changes to the plan"; typing "keep option A" and Enter posted exactly one `/api/resume-plan-stream` with `proposal_id=planprop_inspection`, `approved=false`, `changes=keep option A` and no `/api/ask-stream`, and cleared the composer (the engine then refused the fixture's paused turn: "missing field `session`", a fixture shape problem outside this task; the card kept the text). An injected pending command disabled the composer and Send with "Approve or reject the command first", a forced `sendChatPrompt()` posted nothing, and clicking the card's Reject posted `/api/resume-command-stream` and re-enabled the composer with its own placeholder. The fixture's pending question still locks with "Answer the question". Workspace clippy and tests left to CI. |
| 3 · The model asks choices with `ask_user` (req 4, D5) | Done 2026-11-21 | **Prompt:** `chat/prompt.rs` `USER_CHOICE_PARAGRAPH` ("When the user must make a choice before you can go on, ask it with `ask_user` (2 to 4 short options) before proposing a plan. Never write options for the user to pick from into a plan or an answer."), appended by `system_prompt_with_capabilities` only when a user is synchronously present and the mode is Code or Plan (the two modes `mode_permits` lets propose a plan). Ask and Review, the headless prompt and the worker prompt (`WORKER_QUESTION_PARAGRAPH`) are byte-identical to before; the paragraph is the last one, so the prefix before it is unchanged and the prompt is still static per mode (spec 49 cache). **Tools:** `propose_plan` gains "Ask a choice the user must make with `ask_user` before proposing, never as options in a plan."; `ask_user` gains "Ask a choice the user must make here, before any plan, never as options in a plan or an answer." These are shared with workers, which also offer `ask_user`. **Plan review:** `plan_review_response` ends "You can approve the plan, or decline it and say what to change." **Tests:** `clarification_tests::with_a_user_present_only_code_and_plan_gain_the_choice_paragraph` (was `…every_modes_prompt_is_byte_identical_to_before`; Ask/Review still pinned to the pre-spec-50 bytes, Code/Plan to those bytes plus the paragraph) and the new `the_choice_guidance_sends_choices_to_ask_user` (paragraph, both descriptions, absent without a user and on the worker route); `system_prompt_tests::code_mode_system_prompt_is_byte_identical_to_todays` and `review_mode_system_prompt_matches_plan_modes_restricted_wording` updated for the paragraph; new `plan_deferral_tests::the_plan_review_text_offers_approve_or_decline_with_changes`. No test, eval scenario or shell test asserted the old plan-review text or descriptions. **Deviation:** eval scenario `bounded_worker_long_read_reread` sat within a few dozen tokens of its compaction limit: the longer tool schemas are fixed request overhead, and at `context_window = 60000` its eighth compaction returned `task_compaction_cannot_fit` (7 of the expected 10). It now uses 61000 (60500 also passed), with a comment saying why; the same ten compactions and the reread still happen. Any later growth in tool descriptions may hit the same edge. **Checks:** `cargo clippy -p workspace-engine -p eval-harness --all-targets --locked -- -D warnings` clean; `cargo nextest run -p workspace-engine --locked` 2731 run, all pass (the 26 loopback/watcher/process tests in `external_reference`, `foundation` and `stop_running_actions` rerun outside the sandbox: 234 passed); `cargo nextest run -p eval-harness --locked` 151 passed; deterministic eval tier 66/66 pass; `scripts/quality-gate.sh fmt node lint typos deny` ok. Desktop-shell untouched (no shell test asserts this text). Workspace clippy and tests left to CI. |
| 4 · Documentation and close-out | Not started | |

## Task 1 · One pending decision per session (req 3, D3, D4)

- [x] When a task's status becomes `WaitingForApproval` with its pending link,
      settle every older `WaitingForApproval` task of the same session: take
      its pending turn through the store, set it `Cancelled`, audit
      `pending_decision_superseded` (`taskId`, `supersededBy`, `kind`).
      Exclude worker sessions.
- [x] The startup sweep applies the same rule to a session with several pending
      decisions before reattaching (keep the newest).
- [x] Engine tests: the two acceptance criteria for requirement 3, plus a
      worker session left alone and nothing approved or run.
- [x] Check what reads `WaitingForApproval` (sidebar state, `/api/pending-card`,
      recovery) and record that a settled task drops out of each.

Files: `crates/workspace-engine/src/` (session store, recovery, the pause
path), `crates/desktop-shell/src/recovery.rs` if the sweep needs it, tests.
Checks: scoped clippy for each crate touched; scoped workspace-engine tests;
`cargo nextest run -p desktop-shell`; the deterministic eval tier.

## Task 2 · What typing does while a decision waits (req 1, 2, D1, D2)

- [x] `sendChatPrompt`: with a pending plan review (not in a worker view), post
      the text as the plan's decline with changes, exactly as the card's
      Decline → Send does, and clear the composer.
- [x] Generalise the composer lock: every pending kind except `plan` and
      `question` disables `#chat-prompt` and Send, with a placeholder naming
      it. The question lock is unchanged. A plan sets the placeholder
      "Describe changes to the plan".
- [x] Desktop-shell tests in the `app_js_…` style.
- [x] Manual check: a pending plan (injected or seeded) with typed text posts
      one `/api/resume-plan-stream` with `approved=false` and the text, and no
      `/api/ask-stream`; a pending command locks the composer.

Files: `crates/desktop-shell/static/app.js`, `style.css` if needed,
desktop-shell tests. Checks: `scripts/quality-gate.sh fmt node lint typos
deny`; `cargo clippy -p desktop-shell --all-targets --locked -- -D warnings`;
`cargo nextest run -p desktop-shell`.

## Task 3 · The model asks choices with `ask_user` (req 4, D5)

- [x] Desktop system prompt (Code and Plan modes): ask a choice the user must
      make with `ask_user` before proposing a plan; never write options for the
      user into a plan or an answer.
- [x] `propose_plan` and `ask_user` descriptions: one sentence each to the same
      effect.
- [x] `plan_review_response`: drop "reorder … retitle … remove"; say the user
      can approve the plan, or decline it and say what to change.
- [x] String tests for the three; the deterministic eval tier passes (update
      any scenario that asserts the old text).

Files: `crates/workspace-engine/src/chat/{prompt.rs,tool_definitions.rs,plan_flow.rs}`
and tests; eval scenarios if they assert the old text. Checks: scoped clippy;
scoped workspace-engine tests; `cargo run -p eval-harness -- run --tier
deterministic`.

## Task 4 · Documentation and close-out

- [ ] `USER_GUIDE.md`: what typing does while each kind of decision is
      pending; one pending decision per session. `TROUBLESHOOTING.md`: the
      `pending_decision_superseded` audit event and why an older card is gone.
- [ ] Spec 107: a dated note on D2 (the lock now covers every kind but a plan).
- [ ] Close-out per AGENTS.md: Status, this header, the README row and *What to
      build next*, the CHANGELOG *Unreleased* line, `Depends on:` lines naming
      #113.

Checks: `scripts/quality-gate.sh fmt node lint typos deny`; `npm run
specs:check -- --strict`.
