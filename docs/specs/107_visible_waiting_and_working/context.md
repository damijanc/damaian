# Spec 107 context

**Specified:** 2026-11-10. The survey was read on `main` at the commit that
recorded the turn-window CI run. Line numbers are approximate; re-read before
editing.

## Evidence

Local observations 60, 73 and 79 (`docs/PLAN/OBSERVATIONS.md`, not committed),
from the owner's screenshots and these session logs:

- spec 33 planning, `session_18e677a5f0853bf0_11ad4_c`: a question pending from
  20:39:14 that the owner could not see; patch cards at 20:58:53 and 21:06:36
  found only by scrolling up;
- spec 33 Task 1 rerun, `session_18e6ada304104d08_5998_16` (v0.66.0): round
  pauses at 13:15:56 and 13:18:22; the root baseline ran about 3 minutes and the
  `crates/workspace-engine` one several more, with nothing on screen.

## Survey: what exists today

- **Live decision cards are inside the assistant bubble.** `appendProposals`
  (`app.js` ≈13169–13212) appends every card with `message.body.append(...)`:
  patch (`createPatchPreview`), checks (`createCheckBatchCard`), round limit
  (`createRoundBudgetOffer` ≈13547), question (`createQuestionCard` ≈12207).
  `proposePatchFromChat` also appends into the bubble (≈15710). On reload a
  question becomes its own `article.message.question` at the end of
  `#chat-log` (≈7025–7053).
- **Spec 93's rule covers only recovery.** `renderRecoveryPrompts` (≈7283–7308)
  appends recovered tasks, reattached approvals and verification offers at the
  end of `#chat-log`; live cards do not follow it.
- **The baseline card is inside the patch card** (`createBaselineBatchCard`
  ≈14121, appended in the patch card's file list ≈14586–14628) and disables
  Apply/Reject while shown. Labels come from the engine (`baseline.rs` ≈376,
  ≈378).
- **The status line** is a `p.turn-indicator` built by `startTurnIndicator`
  (≈6351) after the bubble body. `finish(state)` (≈6451–6463) removes it for
  every state but `stopped`, and for `WaitingForApproval`/`Paused` the engine
  sends `indicatorState: Pending` (`task_control.rs` ≈1411–1416), so it
  vanishes exactly when the user is needed.
- **Send/Stop** is `setComposerBusy` (≈15727); the click handler (≈15997) stops
  while `chatSubmitting`, else sends.
- **Plan labels.** `renderPlanPanel` (≈10581) shows *running* for an
  `in_progress` step when `plan.active !== false`. `active` comes from
  `plan_json` (`lib.rs` ≈6312), true for `WaitingForApproval` and `Paused`
  (`TaskStatus::is_active`, `session.rs` ≈522–531). There is no waiting label.
- **Baseline execution** is `POST /api/resume-baseline` → `handle_resume_baseline`
  (`lib.rs` ≈3069), synchronous inside the request. `run_baseline`
  (`baseline.rs` ≈771) creates a `CancelToken::new()` nothing can trigger and a
  no-op output callback (≈781–782). No progress, no cancel.
- **The 16-round pause.** `agent_max_tool_rounds` (default 16, `config.rs`
  ≈3438). `chat.rs` ≈9572 sets `force_final`, and ≈9906 pauses
  (`pause_for_round_budget`, ≈13067) when `segment_tracker.is_some()` is false or
  `agent_max_task_tokens` is unset. A segment tracker exists only for a session
  with a worker record (`task_segment_run_state` ≈8813/8833).
  `decide_segment_continuation` (≈9707; `task_segments.rs` ≈324–365) continues
  while totals are under the approved caps, usage is available and the progress
  digest advanced.

## Decisions

- **D1 — one engine-owned presentation state (recommended).** Extend the turn
  presentation with `pendingDecision: { kind, label, cardId }` where `kind` is
  one of question, patch, checks, baseline, command, round_budget, plan,
  fallback. The UI never infers the kind from card markup; it renders from this
  state live and on reload (`/api/session`). The status line, composer line and
  plan label all read it.
- **D2 — the composer while a question is pending (recommended).** Disable the
  main composer and show "Answer the question above" with a button that focuses
  the card's answer field. Rationale: two text fields with two Send buttons was
  the confusion; a question has its own answer route and a typed message is not
  an answer (AGENTS.md: questions are not approvals; the route matters).
  Alternative: route composer text as a free-text answer — rejected, because it
  silently changes what typing in the composer means.
- **D3 — ordinary Code-mode continuation (owner to confirm before Task 5).**
  Recommended: give an ordinary Code-mode turn a segment tracker with a default
  budget (the worker defaults, `worker_task_budget_*`, restrict-only as today)
  so spec 103's `decide_segment_continuation` applies: continue while the
  progress digest advances and totals are under the ceilings; pause with the
  existing card when progress stops or a ceiling is reached. Ask, Plan and
  Review modes keep the 16-round pause. Alternatives: raise
  `agent_max_tool_rounds` only (fewer pauses, no progress check); keep the pause
  but make the card a one-click default (still an interruption).
- **D4 — baseline as a streamed request (recommended).** Run baselines through
  the same streamed path as verification checks, with phase events (check
  started, elapsed, finished) and a cancel token wired to a **Cancel** control.
  A cancelled baseline is recorded as declined (spec 102's existing
  declined-baseline provenance), so no new classification is needed.
- **D5 — scope of "end of the conversation" (recommended).** The pending card
  is the last element of `#chat-log` and is scrolled into view when it appears
  (`block: "nearest"`, as spec 93 does). Resolved cards stay where they are in
  the history.
