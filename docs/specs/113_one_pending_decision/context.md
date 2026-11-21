# Spec 113 context

Background for [`proposal.md`](proposal.md).

## Evidence

On 2026-11-20 the owner worked on a Snake game repository. A plan review's card
appeared and vanished (the busy-elsewhere reload race, fixed the same day; a
reload restoring the card was fixed after it). With no buttons, he typed his
answers. The session's `chat/pending/` then held four plan reviews, each the
paused turn of a different task, started by these prompts in order:

1. "You're picking up a Snake game (vanilla ES modules + Flask, …"
2. "proceed with the plan"
3. "decision 1, go with the option A, decision 2 go with option B, …"
4. "proceed with the plan" (typed again, with a typo)

The third plan's text asked him to decide several points with options A/B/C in
prose. Its review text ended "You can reorder the steps, retitle them, remove
any you do not want, or approve the plan as it stands."

## Survey: what exists today

Line numbers are on `main` at the start of this spec and will drift.

**UI** (`crates/desktop-shell/static/app.js`)

- Send and Enter both call `sendChatPrompt()`. It goes to the worker composer in
  a worker view and to `sendSteeringNote` when a turn stream is running (spec
  110 D7). It refuses with a toast when the session is busy elsewhere. It never
  reads `pendingDecision`. Otherwise it posts `/api/ask-stream` with `repo`,
  `prompt`, `session_id`, `context_files` and the model fields.
- `syncComposerQuestionLock` disables `#chat-prompt` and `#ask-btn` only for a
  pending decision of kind `question` with a frame. The placeholder is "Answer
  the question" (spec 112).
- The plan card's Decline opens "How should the plan be changed?" and *Send*
  posts `/api/resume-plan-stream` with `approved=false` and `changes` (spec 108
  D6). The engine's `request_plan_changes` redacts the text and audits
  `plan_review_decision` `changes_requested` with `changesLength` only. It
  writes `plan_changes_requested`, which makes the plan replaceable, so the
  revised plan meets the gate again.

**Engine**

- `ask_with_session_inner` (`chat/ask.rs`) creates a task for every message and
  never checks for a paused task.
- `carry_plan_from_the_previous_turn` (`chat/plan_flow.rs`) carries the latest
  plan with open steps through a `plan_resumed` edge. The source plan was never
  approved, so the new task's plan is unapproved and the gate raises a fresh
  plan review. This is how the four pauses arose.
- A paused turn's file in `chat/pending/` is removed only by
  `PendingCommandStore::take`, which only the resume routes reach. The task
  stays `WaitingForApproval`.
- `mark_task_superseded` (`session.rs`) writes `task_superseded` and
  deliberately leaves the status alone. Its one caller is crash recovery's
  resume. `reattach_pending_approvals` (`recovery.rs`) reattaches every
  `WaitingForApproval` task in a session.
- `SessionStore::compaction_limit` already expects "a new prompt can arrive
  while an older task waits".

**Model guidance**

- `propose_plan`'s description (`chat/tool_definitions.rs`) says nothing about
  questions or options. `ask_user`'s says "when the request is genuinely
  ambiguous" and does not say "instead of writing options".
- `WORKER_QUESTION_PARAGRAPH` (`chat/mod.rs`) tells a worker to ask with
  `ask_user`. The desktop prompt (`chat/prompt.rs`
  `system_prompt_with_capabilities`) adds nothing when
  `synchronous_user_present`.
- `plan_review_response` (`chat/plan_flow.rs`) still offers reordering and
  retitling.

## Decisions

- **D1 — typing over a pending plan (owner, 2026-11-21).** Typed text is the
  plan's decline with changes. It authorises nothing (spec 108 D6), and it is
  what the owner meant when he typed his choices. The UI sends it through the
  existing route. No new route is added.
- **D2 — every other pending kind locks the composer (owner, 2026-11-21).** This
  is spec 107 D2's rule for questions, extended. A command, patch or check has
  no free-text answer route, and sending the text as a new turn is what piled
  up the pauses.
- **D3 — older pauses are settled (owner, 2026-11-21).** The status is
  `Cancelled`, not left alone as `mark_task_superseded` leaves it. A task
  whose decision can no longer be answered must not read as waiting: the
  sidebar's Needs you, the reload card and the startup sweep all key on
  `WaitingForApproval`. The audit event (e.g. `pending_decision_superseded`,
  `{taskId, supersededBy, kind}`) records why. Nothing is run or approved. The
  pending file is taken through the same store method the resume routes use,
  so no second deletion path exists. Where to hook it is the task's choice:
  the single place a task's status becomes `WaitingForApproval` with its
  pending link is preferable to each pause kind.
- **D4 — startup cleans up what already piled up.** The sweep applies D3 to a
  session with more than one pending decision before reattaching. The newest
  task by creation order wins.
- **D5 — model guidance (owner, 2026-11-21).** One paragraph in the desktop
  system prompt (Code and Plan modes) plus a sentence in each tool
  description, kept short to protect the prompt cache. The deterministic eval
  tier must still pass. Its live tier is not required.
- **D6 — what is not affected.** Steering notes (spec 110 D7): a running turn
  has no pending decision. Worker sessions: their pauses and composer are the
  worker's (spec 57).
