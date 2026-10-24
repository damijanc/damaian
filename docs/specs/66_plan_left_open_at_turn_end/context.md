# Context: A Plan Left Open When the Turn Ends

Background for [`tasks.md`](tasks.md). This file records the places where
[`proposal.md`](proposal.md) no longer matches the code, the decisions the
proposal leaves open, and the interactions and file collisions a fresh task
session must know about. Read it before Task 1. Do not "fix" any of these back
to the proposal's wording without re-reading the code they cite.

Checked on 2026-10-24 against `main` at `5ae1f28`. Two other pieces of work are
in flight in separate sessions and their edits are **not** in this checkout:
#26 context assembly (being implemented) and #65 exploration memory (being
planned). Both edit `chat.rs`; §5 records exactly where. Line numbers below were
correct on 2026-10-24 and will move when those land.

## 1. File references, checked on 2026-10-24

| Proposal cited | The code now |
|---|---|
| `run_agentic_turn` (`chat.rs:1983`) | `chat.rs:2505` |
| `Answered` breaks at `chat.rs:2703` and `2732` | The completion points are the no-tool-call answer at `:3301` (after `before_task_complete` at `:3290`) and the forced-final answer at `:3259` (after the hook at `:3247`). There is **no** "empty decode" `Answered` break: an undecodable call is fed back as a user note and the loop continues (`:3308`–`:3337`). Terminal proposals and questions also carry `Answered`, but they set a pending proposal and are pauses, not completions |
| Task becomes `Complete` (`chat.rs:3788`) | `:4658` (`StopReason::Answered => TaskStatus::Complete`); the status is written at `:4660`–`:4661` |
| `complete_step` arm (`chat.rs:3252`) | `:3975`–`:4053` |
| `step_evidence` local (`chat.rs:2209`) | `:2731` |
| Comment about lost evidence (`chat.rs:2195`) | `:2717`–`:2726` |
| `evidence_for` after tool arms (`chat.rs:3619`, `3625`) | `:4455`–`:4465` |
| Command-approval resume (`chat.rs:1275`) | `resume_after_command_decision_with_options` at `:1092`; re-entry to the loop at `:1430`–`:1441` |
| Plan-decision resume (`chat.rs:1401`) | `resume_after_plan_decision` at `:1462`; re-entry at `:1555`–`:1566`. The question resume is `:1579` / re-entry `:1707`; the provider-fallback resume is `:1728` / `:1847`; the round-budget resume is `:1873` / `:1949` and `:1983` |
| `PendingChatTurn` builders (`chat.rs:2906`–`4022`) | The struct is `:5696`. The seven literals that save a paused turn are `:3496` (plan review), `:3765` (command approval), `:4141` (web diagnostic), `:4253` (MCP call), `:4376` (question), `:4760` (provider fallback), `:4881` (round budget) |
| Tool descriptions (`chat.rs:5652`, `5657`) | `propose_plan_tool_definition` at `:6794` (description `:6797`) and `complete_step_tool_definition` at `:6802` (description `:6805`–`:6806`) |
| `SessionStore::append_step_evidence` (`session.rs:1492`) | Unchanged at `:1492`. Its only production caller is `edit.rs:804`, not `edit.rs:668` |
| `PlanReport::summary` (`plan.rs:404`) | Unchanged at `:404`; it takes no argument: `pub fn summary(&self) -> String` |
| `renderPlanPanel` (`app.js:7126`) | `:7482`; the "running" label is `:7531`–`:7536` |
| "running" label (`app.js:7175`) | `:7531`–`:7536` |
| History render (`app.js:4409`) | `:4741`, inside `renderMessages`. `renderMessages` receives `tasks` with each task's `status`; the plan JSON is put into `planByTask` and the status is not passed to `renderPlanPanel` |
| Plan JSON (`desktop-shell/src/lib.rs:3674`) | `plan_json` at `:3923`. Callers: the plan-review card `plan_proposal_json` (`:3983`), the SSE relay (`:4067`), the turn result (`:4154`) and `task_states_json` (`:4736`). The JSON is hand-built and carries no task status |

## 2. The claims, checked

All of these were checked; the ones that did not hold are collected in the table
at the end of this section.

- **`step_evidence` starts empty on every resume.** True. It is declared at
  `chat.rs:2731` as `Vec::new()` and is never seeded from the pending turn.
  Every `run_agentic_turn` call site — the initial ask (`:1055`) and all seven
  resumes (`:1430`, `:1555`, `:1707`, `:1847`, `:1949`, `:1983`, plus the
  helper re-entries) — passes only the message array and counters.
- **`PendingChatTurn` does not carry it.** True. The struct (`:5696`–`:5760`)
  carries `round`, `exploration_rounds`, `messages`, the paused call, reasoning,
  `round_calls_fed_back`, and the pause markers (`plan_review`,
  `provider_fallback`, `round_budget`, `question`). There is no evidence field
  and no reminder field.
- **`append_step_evidence` exists and only one production path calls it.**
  True. `session.rs:1492` appends one item to whichever step is
  `InProgress`; it returns `Ok(())` when there is no plan or no open step. It
  never recomputes the step's status. The only caller is `edit.rs:804`
  (`apply_stored_patch`), which runs outside the turn that planned the step.
- **The `complete_step` arm is the only place a step reaches a terminal
  status today.** True. It finds the `InProgress` step, extends its evidence (it
  does not replace it), derives the status with `status_from_evidence`, and
  writes the step and its successor through `update_plan_step` (`chat.rs:3990`–
  `:4027`). `update_plan_step` is otherwise used only by plan creation/revision.
  Task 5 adds the declined-plan `Skipped` transition (§3 D5), which records the
  user's decision rather than a model claim.
- **The real `TaskStatus` enum has fifteen variants**, not the thirteen §5.4
  lists: `Created`, `PreparingContext`, `WaitingForModel`, `RunningTool`,
  `WaitingForApproval`, `Paused`, `ApplyingPatch`, `Validating`, `Complete`,
  `Failed`, `Cancelled`, `ToolBudgetExhausted`, `TokenBudgetExhausted`,
  `Interrupted`, `UnknownExternalOutcome` (`session.rs:58`–`:83`). The last two
  are not `is_terminal()` (`session.rs:143`–`:152`) because recovery can act on
  them, but no turn is running. §5.4's list must add them as inactive; see §3
  D7.
- **`PlanReport::summary` takes no argument.** True (`plan.rs:404`). The only
  production caller is `plan_json` (`desktop-shell/src/lib.rs:3949`). The change
  is a signature change plus updates to every test in
  `crates/workspace-engine/tests/plan.rs` that calls it (`:1114`, `:1135`,
  `:1154`, `:1172`, `:1187`, and the assertions that call it a second time in
  their failure message).
- **The plan JSON is built by hand in `plan_json`.** True. It serialises
  `taskId`, each step's `id`/`title`/`detail`/`status`/`outcome`/`evidence`, and
  the report's counts and summary. There is no task status and no `active`
  field. The live path (`relay_turn_events`, `:4067`) has only the plan; the
  history path (`task_states_json`, `:4736`) has `statuses` beside `plans`, so
  it can compute `active` without a new read.
- **The eval-harness record is camelCase on the wire.** True: `RunRecord` and
  the nested records derive `Serialize` with `#[serde(rename_all =
  "camelCase")]` (`crates/eval-harness/src/record.rs:158`), so a Rust field
  `plan_reminders` is JSON `planReminders`. `plan` is an `Option<RecordedPlan>`
  (`record.rs:230`), absent for a planless run. The runner reads the plan from
  the session log and the final task status separately
  (`crates/eval-harness/src/runner.rs:664`–`:692`, `:694`–`:739`). A top-level
  bool would therefore read `false` for a planless run, which is not a
  measurement; see §3 D11.
- **The round budget's pre-call check is `chat.rs:2874`–`:2900`.** True:
  `max_rounds = (tool_round_limit(web_debug_mode) + round_budget_extension)
  .min(ABSOLUTE_TURN_ROUND_CAP)`; `force_final` is `action_rounds >= max_rounds`.
  `tool_round_limit` (`:651`) reads `agent_max_tool_rounds` through
  `default_tool_round_limit` (`:639`), clamped to at least 1. A forced-final
  call has no tools, so the reminder must leave room for a **tool-carrying**
  call after its own round; see §3 D12.
- **The token ceiling's pre-call check is `chat.rs:2841`–`:2872`.** True: it
  reads the task's recorded `input_tokens + output_tokens`, treats an absent
  record as zero, and stops before the call when `spent >= ceiling`. The
  reminder gate must use the spend **after** the answer round, not the figure
  read before it.
- **The `propose_plan`/`complete_step` descriptions and the "Plan recorded"
  result are the only places the model is told to advance the plan.** Essentially
  true. `plan_review_response` (`chat.rs:6692`) also says "Continue with the
  current step", and `token_budget_exhausted_response` names the remaining
  steps, but neither tells the model to call `complete_step`.

### Claims that did not hold

| The proposal said | The code says now | Resolved in |
|---|---|---|
| §5.4: thirteen `TaskStatus` states; five inactive | Fifteen; `Interrupted` and `UnknownExternalOutcome` are non-terminal but not running, so seven are inactive | §3 D7 |
| §5.4: `PlanReport::summary` "takes it as an argument" | It takes no argument (`plan.rs:404`); the signature and every caller/test change | Task 7 |
| §5.4: the live stream "sends `true` until the final plan event … which sends `false`" | There is no final plan event today; `TurnProgress::Plan` is sent only when the plan changes | §3 D9 |
| §2: the second `Answered` break is the "empty decode at `2732`" | An undecodable call is fed back and the loop continues (`chat.rs:3308`–`:3337`); the two completion points are the no-tool-call and forced-final answers | §2, Task 5 |
| §5.5: each run record gains `planOpenAtEnd` and `planReminderSent` | `RunRecord.plan` is an `Option<RecordedPlan>`; a planless run has no plan. A top-level bool would read `false` for "not measured" | §3 D11 |
| §2: "the history view … the task status is known to the caller but not passed in" | True, and the status is in the same `tasks` payload, so no new read is needed | §3 D9 |

**A plan-review decline leaves the plan untouched today.** The declined branch
of `resume_after_plan_decision` (`chat.rs:1483`–`:1488`) records neither an
approval nor an outcome for the steps, so step 1 stays `InProgress` and the
reminder would fire. This is why §3 D5 exists.

## 3. Decisions the proposal left open

### D1. The reminder runs **after** `before_task_complete`, not before it

Spec 32's hook already runs at both completion points (`chat.rs:3247` and
`:3290`). The order is fixed: hook first, then the plan reminder, only when the
hook returned `None` (allowed).

Why: the hook is the last gate before completion and must see the answer and
plan state as they are. A hook denial is already a continuation of the same
turn, so the turn was not about to end; reminding at that point would be
premature and would put two engine notes in front of the model at once. With
hook-first, a hook that keeps denying means no reminder is ever sent and the
turn ends at the round or token budget, as #32 intends.

Consequences to accept and test:

- A `before_task_complete` hook can fire **twice** on a turn with an open plan:
  once before the reminder and once at the completion after the reminder round.
  It would also fire at any other completion attempt; the dispatch is
  idempotent from Damaian's side and the payload carries the current round count.
- The hook's re-entry round and the reminder round are both charged with
  `action_rounds += 1` (`chat.rs:3256` is the hook's precedent). They draw on
  the same budget. The hook therefore has priority: a hook that consumes the
  last rounds leaves no room, and no reminder is sent.
- If the hook denies the completion **after** the reminder, the turn gets one
  more round with the hook's note; `plan_reminder_sent` stays true, so no second
  reminder. The final answer is whatever ends that later completion.

### D2. *Answer now* is never reminded

`RoundBudgetDecision::AnswerNow` sets `turn_options.round_budget_answer_now`
(`chat.rs:1979`–`:1982`). The existing completion code already skips
`before_task_complete` on that flag (`chat.rs:3247`), for the reason recorded
there: that path deliberately has no rounds left, and re-entering would be
unbounded because the pause that would stop it is skipped. The plan reminder
follows the same rule and is skipped on the same flag. The forced-final call
still runs, and a turn that answers there may still leave steps open; it ends
`Answered`, and §5.3 attaches the evidence.

### D3. A question pause is not a completion and never triggers the reminder

A question is a tool call (`ask_user`), so the round never reaches the
no-tool-call completion branch; the `ask_user` arm saves a `PendingChatTurn`
(`chat.rs:4376`) and breaks with a question proposal, and the loop's end sets
`pending_question` (`:4645`–`:4653`). No reminder is evaluated there, and
§5.2's "no pending proposals" condition is therefore structural at a completion
point rather than a runtime check.

If a question is asked **after** a reminder, the pause must carry
`plan_reminder_sent = true` and `step_evidence`, and `resume_after_answer` must
seed both. A question answer is not an approval (spec 50 §5.2 and AGENTS.md):
it must not clear a plan review, satisfy the reminder, or count as the model
completing a step. The reminder itself is engine-authored, not an answer to a
question, so it has no approval standing either.

### D4. The two-answers rule, precisely

- The answer that triggered the reminder is appended to the session log at the
  moment the reminder is sent, and to `messages` as the assistant turn with its
  reasoning.
- The post-reminder `response` is the loop's final value. It is appended as a
  second assistant message when it is not empty.
- `ChatTurnResult.response` is the **pre-reminder** answer only when the turn
  truly completes: `StopReason::Answered` and no pending proposal or question.
  `model_run` stays the last call's run (only `run_id`/`incomplete` are read from
  it by the shell), and `response` is the text the live view shows.
- When the post-reminder round instead pauses (command, patch, plan review,
  question), or ends `ToolBudget`/`TokenBudget`, the loop's own response stands,
  as today. The pre-reminder answer is already before it in the log; replacing
  the response with it would hide the card or the budget notice.
- An empty pre-reminder answer is not appended and is never made the response;
  an empty post-reminder answer adds no second message.

### D5. A declined plan is marked `skipped`, not reminded about

Without this, `resume_after_plan_decision(false)` leaves step 1 `InProgress`,
the model answers as instructed ("Do not make any change"), and the reminder
tells it to call `complete_step` — which would close the untouched step as
`Completed` with no evidence. That is the failure requirement 6 exists to
prevent, reached one level removed.

So the declined branch marks every non-terminal step `Skipped`, writes each
through `update_plan_step` with `completed_at_ms`, and sends the plan on the
sink. The user's decision is what closed the steps, not the model's claim; the
status is `Skipped`, not `Completed`; and the step keeps any evidence it already
had. The plan is then not open, so no reminder and no `plan_left_open` follow.

### D6. A pause carries; a true end settles

- **Pauses** — command/patch/plan-review/question/provider-fallback/round-budget
  — save `step_evidence` and `plan_reminder_sent` into the `PendingChatTurn` and
  do not touch the step. The resume seeds both.
- **True ends** — answered, tool budget, token budget, the stop button, a
  provider failure, and a declined provider fallback — attach the buffer to the
  open step with `append_step_evidence`, extend the in-memory plan so the final
  panel event carries it, audit `plan_left_open`, and only then write the task's
  final status. Attaching is best-effort: an unwritable log must not turn a
  finished turn into an error, the same rule `edit.rs` follows.
- The stop button leaves through `finish_cancelled_turn` (`chat.rs:4987`), from
  three places inside the loop (`:2803`, `:3023`, `:3172`). It needs the plan,
  the buffer, the flag and the sink, so its signature grows rather than adding a
  fourth caller-side attach.
- A declined provider fallback (`resume_after_fallback_decision`, `:1758`–
  `:1780`) fails the task without re-entering the loop; it must settle from
  `pending.step_evidence` and the plan read from the log.
- **Limit:** a task abandoned or marked failed from the recovery card does not
  run the loop, so its pending evidence is not attached. The task status still
  makes the panel say "not finished". Do not add a recovery-path attach here.

### D7. `active` is an exhaustive `TaskStatus` match, and it is not `!is_terminal()`

Add `TaskStatus::is_active(&self) -> bool` in `session.rs` with a match over all
fifteen variants. True: `Created`, `PreparingContext`, `WaitingForModel`,
`RunningTool`, `WaitingForApproval`, `Paused`, `ApplyingPatch`, `Validating`.
False: `Complete`, `Failed`, `Cancelled`, `ToolBudgetExhausted`,
`TokenBudgetExhausted`, **`Interrupted`, `UnknownExternalOutcome`**.

The last two are the correction. They are non-terminal for recovery but no turn
is running, and a crash-leftover plan must not say "running". A test over
`TaskStatus::all()` pins the partition, and a second one records why the two
differ from `is_terminal`.

### D8. The summary's wording keeps the other facts

When `active` is false and there are open steps, the summary leads with
"Turn ended with N steps not marked done" and then keeps any "N blocked" and
"N finished" parts that today's summary would print. A plan with a blocked step
and two open ones must not lose the blocked count to the new sentence. With no
open steps, the summary is exactly today's text, whatever `active` is.

### D9. `active` reaches the panel through three paths

- **Live:** add `TurnProgress::PlanFinished(TaskPlan)` (public, like
  `TurnProgress::Plan`). `plan_finished` sends it at a true end; intermediate
  `sink.plan` calls stay `Plan` and mean active. The shell's
  `turn_progress_event` maps `Plan` to `TurnEvent::Plan { plan, active: true }`
  and `PlanFinished` to `{ active: false }`; `relay_turn_events` calls
  `plan_json(&plan, active)`. The worker's progress writer (`worker.rs:1012`)
  maps both, so a worker's board gets the same flag.
- **History:** `task_states_json` parses the task's status with
  `TaskStatus::parse` and passes `is_active()` into `plan_json`.
- **Review card:** `plan_proposal_json` passes `true`; the turn is waiting, not
  ended.

`renderPlanPanel` uses `plan.active !== false` so an older payload without the
field behaves as today. The final live event is emitted for answered, budget,
cancelled and failed ends; a pause emits none.

### D10. The session-log reminder is tagged, and the next turn's seed skips it

`append_message` writes a fixed JSON object and `ChatMessage` has no kind. Add
`kind: Option<String>` to `ChatMessage` and an `append_message_with_kind`
(serialised as an extra `"kind"` field; old lines parse as `None`). The shell's
`message_json` forwards it, and `renderMessages` renders a
`kind === "plan_reminder"` entry through `appendChatMessage("system", …)` so the
existing `.message.system` style applies.

`build_model_prompt` (`chat.rs:6101`) seeds the next turn with the last eight
session messages. It must skip tagged plan reminders, or a later request starts
with an engine instruction that reads as something the user said. Within the
same turn the reminder is already in `messages` and is unaffected.

### D11. The eval fields live on `RecordedPlan`, and reminders are a count

`RecordedPlan` (present only when the turn had a plan) gains:

- `reminders_sent: u64` — from `trace.count("plan_reminder_sent")`. A count, so
  the "exactly one" assertion can fail on two; a bool could not.
- `left_open_at_end: bool` — from `trace.count("plan_left_open") > 0`, the
  engine's own fact rather than a second terminal-status mapping.
- `open_step_evidence: u64` — the number of evidence items on non-terminal
  steps in the final plan, which is how the second scenario sees the read that
  was attached to the still-open step.

No new metric: the deterministic scenarios and these record fields are the
measurement. A metric would need a baseline change and would say nothing the
assertions do not.

### D12. The reminder's budget gate is two rounds of room

At a completion with `action_rounds = A` and `max_rounds = M`, sending the
reminder sets `action_rounds = A + 1` and continues. The next iteration computes
`force_final = (A + 1) >= M`; if true, the tools are withheld and the model
cannot call `complete_step`. So the gate is `A + 1 < M`, not `A < M` and not
`A + 1 <= M`. The token gate is the same pre-call comparison against the spend
**after** the answer round; a helper that reads the task's spend should be
shared with the top-of-loop check so the two cannot disagree.

### D13. The reminder round is an action round

It dispatches no tool, exactly like the forced-final call and the hook
re-entry, so it is charged with `action_rounds += 1` and never to
`exploration_rounds`. #59's allowance is untouched.

### D14. The approved command's evidence uses the execution id as its breadcrumb

`evidence_for` takes a marker id, and the command arm passes the action
marker. The resumed approved command is not bracketed by a new action marker
(`validation_orchestrator.run_proposal` only runs and records the execution), so
the correct breadcrumb is `record.execution.id`: it is the `commandId` on the
`stored_command_executed` audit event that carries the exit. Do not invent a
marker or reuse the old `awaiting_approval` marker, which belongs to an action
that did not run.

### D15. The reminder is a nudge, not pinned state

`bounded_messages` (`chat.rs:6161`) keeps the head (system and user seed) and
the newest whole rounds. The reminder is an ordinary user message at the tail,
so a long post-reminder run can elide it. That is accepted: the model acts on
the reminder in the rounds that follow, and if it answers again the turn ends
with the steps open, which §5.4 reports. Pinning it would duplicate #65's
`turn_notes` machinery and collide with that spec; if the live run shows the
reminder being lost too soon, extend #65's pinning rather than adding a second
pinned mechanism here.

## 4. Interactions with work that landed after #66 was specified

### 4.1 Spec 32 hooks: `before_task_complete`

Code: `ChatEngine::before_task_complete` (`chat.rs:2455`–`:2502`) returns
`Some(note)` on `Deny`/`RequestApproval` and is called at the force-final
completion (`:3247`) and the no-tool-call completion (`:3290`). Its re-entry
pushes the answer and the note and does `action_rounds += 1`.

Resolution: hook first, reminder second, reminder only when the hook allows.
The full consequences are D1. The reminder round counts against the same
`action_rounds` budget the hook re-entry uses, so a hook that keeps denying can
consume the room; that is the hook's priority, not a bug.

### 4.2 Spec 61 *Continue* and *Answer now*

- *Answer now*: skip the reminder on `turn_options.round_budget_answer_now`
  (D2). The existing hook skip at `chat.rs:3247` is the precedent.
- *Continue*: the round-budget pause saves a `PendingChatTurn`
  (`chat.rs:4881`); it must write `step_evidence` and `plan_reminder_sent`, and
  `resume_round_budget` must seed both. A turn continued after a reminder must
  not remind again, and its evidence must survive the pause. `pending.round` is
  passed unchanged on *Continue* (`:1955`) and `+ 1` on the other resumes, which
  stays correct: the pause happens before a call, so the round was not charged.

### 4.3 Spec 50 question pause

A question is a tool call and a pause, never a completion (D3). The question
pause's `PendingChatTurn` (`:4376`) must carry both fields, and
`resume_after_answer` must seed them. A question answer is not an approval and
must not clear a plan review or the reminder. If the model asks a question
after a reminder, the flag prevents a second reminder when the answer resumes
the turn.

### 4.4 Specs 59 and 63

- #59: the reminder round is charged to `action_rounds` (D13); no change to
  `agent_max_exploration_rounds`, `seen_read_calls` or the budget messages.
- #63: verified. `status_from_evidence` (`plan.rs:167`–`:198`) treats
  `CommandExit { purpose: Exploration, .. }` as non-blocking, including `None`.
  Attaching a failed `grep` therefore cannot block a step that is closed later.

### 4.5 A declined plan (found while checking, not in the proposal)

See D5. It is the one case where a step changes status because the **user**
decided, not because the model claimed completion, and it must be tested with
the reminder in place.

## 5. Collisions with in-flight `chat.rs` work

The rule from [`docs/specs/README.md`](../README.md#parallel-work) is that at
most one spec that edits `chat.rs` is in flight. #66 edits `chat.rs` in Tasks
1–8, so it is a `chat.rs` track.

- **#26 context assembly (in progress in another session).** It rewrites
  `context_manager.rs`'s `build_context`/`build_context_with_hooks` and the
  assembly around it; in `chat.rs` its call site is
  `ask_with_session_with_options`'s `build_context_with_hooks` call at `:1030`,
  and it may touch `build_model_prompt` (`:6101`) for the manifest. #66 does not
  edit `ask_with_session_with_options`; it does touch `build_model_prompt` in
  Task 8 (the `plan_reminder` filter) and the loop from `:2505` on. The two
  must not run concurrently; #66 starts after #26's `chat.rs` edits are on the
  branch it will use, or rebases after them.
- **#65 exploration memory (being planned in parallel).** It edits
  `bounded_messages` (`:6161`), `build_model_prompt` (`:6101`), the turn loop
  (a `turn_notes` tool and its charging), and `PendingChatTurn`
  (`#[serde(default)] turn_notes` plus a call list). #66 edits
  `PendingChatTurn` (`step_evidence`, `plan_reminder_sent`), the loop's
  completion breaks and evidence accrual, `complete_step`, every resume, and
  `build_model_prompt`'s filter. The shared literals are the seven
  `PendingChatTurn { … }` sites (`:3496`, `:3765`, `:4141`, `:4253`, `:4376`,
  `:4760`, `:4881`): whichever lands second must add its fields to all seven.
  #65's `bounded_messages` must keep the reminder as an ordinary user message.
- **#32's `before_task_complete`** is already in `chat.rs` at the two sites
  #66's reminder sits beside. #66 changes the order only by calling the
  reminder after the hook returns `None`; it does not move the hook.
- **#61's round-budget pause** and **#50's question pause** are the same
  `PendingChatTurn` machinery; #66 adds fields to their saved turns.
- **`session.rs`:** #66 adds `ChatMessage.kind`, `append_message_with_kind` and
  `TaskStatus::is_active`. #65 does not edit `session.rs`; #26 may edit
  `context_manager.rs`, not this.
- **`desktop-shell/src/lib.rs` / `worker.rs` / `app.js`:** #66 edits the plan
  JSON, the live event and the panel. #26's inspector is #27; #65's notes are
  rendered in the composer/conversation. Coordinate the panel render with #65
  if both land in the same release.

**Pairing rule for #66, to be repeated in the README note:** #66 may pair only
with a spec that never edits `chat.rs`. Neither #26 (in progress) nor #65
(planned in parallel) qualifies, and no other ready spec is left, so #66 is a
solo `chat.rs` track. It does not pair with #23, #55 or #49's reuse slice for
the same reason.

## 6. Where the tests go

The repository uses integration tests under `crates/*/tests/` plus inline
`#[cfg(test)]` modules for pure functions. #66 uses both.

| Concern | File |
|---|---|
| `open_steps`, `TaskStatus::is_active`, summary wording | `crates/workspace-engine/tests/plan.rs`; `TaskStatus` may also go in `session.rs`'s `mod tests` (`:2711`) |
| The reminder text and the token gate helper | inline `chat.rs` `#[cfg(test)]` modules (see `evidence_tests` at `:7843` and the prompt tests at `:10292`) |
| Reminder/turn behaviour: fires once, two answers, closed/blocked/absent, budget, Answer now | `crates/workspace-engine/tests/plan_turn.rs` (reuse `temp_repo`, `engine_for`, `ask`, `ask_watching_plans`, `scripted`, `decide`, `plan_of`, `log_of`, `replay`) |
| Pause persistence and old pending files | `tests/round_budget.rs` (`pending_file`, `:92`), `tests/clarification.rs` (`pending_turns`, `:781`), and the strip-a-field pattern in `tests/exploration_rounds.rs:565`–`:600` |
| Hook order | `crates/workspace-engine/tests/hooks.rs` (its `/bin/sh` hook fixtures) |
| Cancel settles the plan | `tests/foundation.rs` (`ask_with_cancel`, `:1388`) or a local helper in `plan_turn.rs` |
| Provider failure settles the plan | `tests/provider_limits.rs` |
| Plan JSON, live event, task-state `active` | inline `desktop-shell/src/lib.rs` tests (`:4968`; `plan_json` tests at `:5111`–`:5175`, `:5484`) |
| Eval record and scenarios | `crates/eval-harness/tests/harness.rs`; scenario count guard at `:1170` |

Fixtures: engines set `enable_index_watcher: false` and a `data_dir` inside the
temporary repository. Model-dependent paths use `MockModelAdapter` (its
`requests` field records what each round was sent). There is no JavaScript test
harness; the UI change is checked by hand as spec 32 Task 9 did (rebuild,
restart on a spare port with a scratch `DAMAIAN_DATA_DIR`, drive `app.js` from
the console).

The eval tier currently has 23 scenarios; #66 adds two and the count guard moves
to 25. The existing `planned_task` scenario should also assert `plan_reminders =
0`, so the "no reminder for a closed plan" half is measured by the harness and
not only by an engine test.

## 7. Limits recorded rather than hidden

- The reminder can be elided by `bounded_messages` after a long post-reminder
  run (D15). It is not pinned.
- A recovery-card *Abandon* or *Mark failed* does not run the loop and does not
  attach the pending evidence (D6). The task status still makes the panel say
  "not finished".
- `PlanReport::is_complete` treats an all-skipped plan as complete because
  `skipped` does not block it; a declined plan therefore summarises as "0 steps
  complete, N skipped." This is existing #21 behaviour and #66 does not change
  it.
- `edit.rs` attaches a patch's evidence by writing one `plan_step_updated` per
  item through `append_step_evidence`; the turn-end attach uses the same path,
  so a turn that ends with 12 command results writes 12 step updates. The plan is
  small and the log is append-only; batching would be a separate change.
- The live panel is only as current as the last event. The final event is
  emitted on answered, budget, cancelled and failed ends; a crash leaves the
  client to reload the task states, as it does today.

## 8. Things the proposal says that were checked and hold

- **#21 requirement 6 is untouched.** `status_from_evidence` takes only
  evidence, `append_step_evidence` never recomputes status, and the reminder
  only asks the model to call `complete_step`; it never closes a step. A step
  that is closed with no evidence is `Completed, unverified`, said out loud by
  the panel, not silently counted as confirmed.
- **`complete_step` extends evidence rather than replacing it**
  (`chat.rs:3997`–`:4005`), so evidence attached after an earlier pause is not
  lost when the step finally closes.
- **`StepStatus::is_terminal` is `Completed | Blocked | Skipped`**
  (`plan.rs:34`–`:36`), so `open_steps` is exactly `Pending | InProgress`. A
  blocked step is not open, which is what §5.1 wants.
- **`TaskPlan` replay reads active events** (`session.rs:1554`–`:1575`), so a
  rewind takes a plan (and the attached evidence) with it; the reminder cannot
  resurrect a rewound step because the plan read at turn start no longer has it.
- **The reminder's round is not a new model call on its own.** The call it
  modifies is the next loop iteration's, so the `model_request_prepared` audit
  and token accounting see one ordinary call, not a synthetic one.
- **Zero-plan and zero-open-step turns are byte-identical to today.** No plan,
  no serialised plan event, no reminder, no `plan_left_open` (`open_steps` is
  empty). Both pin tests belong in `plan_turn.rs`.
