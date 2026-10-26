# Feature Spec: A Plan Left Open When the Turn Ends

Status: Done 2026-10-25 (implementation, live acceptance, and the full
seven-check gate green in CI). Started 2026-10-24 and implemented in ten tasks
([`tasks.md`](tasks.md)); Tasks 1–8 edited `chat.rs`. The one reminder per turn,
the evidence carried across pauses and attached at a true end, the declined
plan's `Skipped` steps, the `plan_left_open` audit record and the panel's
`active`/`not finished` wording are all in the tree; §7 below records the
acceptance walk and what was not exercised. Planned as a folder spec on
2026-10-24; specified the same day from a reporting-only session that answered
in full, ended `complete`, and left its plan panel showing the first of four
steps as **running** and "4 steps outstanding." Corrections where this spec no
longer matches the code, the decisions it left open, and its interactions with
specs 32, 50, 61 and 65 are in [`context.md`](context.md), not inlined here.
Order: 66 of 66
Plan: none. Evidence-driven, like [#47](../47_agent_working_capability/proposal.md)
— an exception to the rule in [`README.md`](../README.md).
Depends on: [#21](../21_task_plan_progress_and_budget/proposal.md) (the plan, its
evidence and its report) — built; [#17](../17_durable_task_state_and_crash_recovery/proposal.md)
(`PendingChatTurn` and resume) — built. Everything else named below is a
cross-reference, not a prerequisite.
Related implementation specs:
[`61_continue_the_stopped_turn/`](../61_continue_the_stopped_turn/proposal.md) (a
paused turn is a `PendingChatTurn`; this spec adds to what it carries),
[`59_exploration_round_allowance/`](../59_exploration_round_allowance/proposal.md)
(the round budget the reminder in §5.2 must fit inside),
[`63_exploratory_command_failures/`](../63_exploratory_command_failures/proposal.md)
(an exploration exit is evidence that does not block a step, which §5.3 relies
on), [`32_hooks/`](../32_hooks/proposal.md) (`before_task_complete` fires at the
same completion points; §5.2 fixes the order),
[`50_model_initiated_clarification/`](../50_model_initiated_clarification/proposal.md)
(a question pause is not a completion, and its answer is not an approval), and
[`65_exploration_memory_within_a_turn.md`](../65_exploration_memory_within_a_turn.md)
(it edits the same loop, `PendingChatTurn` and `bounded_messages`; see
`context.md` §5).

The most important corrections, all checked against the code on 2026-10-24:
- the completion points are the no-tool-call answer and the forced-final answer.
  There is no separate "empty decode" `Answered` break: an undecodable call is
  fed back and the loop continues. `before_task_complete` (spec 32) runs at both
  points, and the reminder runs **after** the hook allows;
- `TaskStatus` has fifteen variants, not thirteen. `Interrupted` and
  `UnknownExternalOutcome` are not running for the panel, so §5.4's list grows
  by two;
- a declined plan is marked `skipped` by the user's decision, so it is not open
  and the reminder never asks the model to complete work the user refused;
- the eval record carries a reminder **count** on `RecordedPlan`, not a bool on
  the run, so "exactly one" is measurable and a planless run cannot read as
  false.

## 1. Motivation

Session `session_18e166cdcd391880_9316_a` (2026-10-24, `deepseek-v4-flash`)
was asked to report which specs are left to implement. The model proposed a
four-step plan — collect statuses, inventory worktrees, verify against the
code, write the report — and the user approved it. The tool result said "Plan
recorded. The current step is: Collect every spec's declared status. Call
complete_step when its work is done."

The model then did all four steps' work across 19 model calls and 12 approved
commands, wrote the report, and stopped. It never called `complete_step`. The
log holds `plan_created` and `plan_approved` and **no** `plan_step_updated`.
The task went to `complete` at 09:35:56.

The panel the user saw afterwards:

```
Plan                                         4 steps outstanding.
  Collect every spec's declared status                    RUNNING
  Inventory worktrees and their in-progress specs
  Verify declared statuses against the code
  Write the report
```

The user read that as "Damaian is stuck" and went looking for the cause. It
was not stuck: nothing was running, and the answer was already in the
conversation. Three things went wrong, and only the first is the model's:

1. **The model forgot to advance the plan, and nothing reminded it.** The only
   instruction it ever got about `complete_step` was in a tool result from the
   first round, long since cut from the request window by `bounded_messages`.
2. **The panel said "running" about a turn that had ended.** The plan is
   rendered as recorded, and a step recorded `in_progress` is labelled
   "running" whatever the task's own status is.
3. **The evidence was lost.** The 12 commands the turn ran were exactly what
   #21 §5.3 wants attached to a step. None reached the plan, partly because no
   step closed, and partly because the evidence buffer starts empty on every
   resume after an approval (§2), so most of it was discarded twelve times
   before the turn ended.

#21 already accepts that a turn can end with a step open: its `Outstanding`
outcome exists for "a token stop, the stop button, a plan held for review"
(#21 `tasks.md`, Task 9). In each of those the open step is true — work really
was cut off. Here the work was finished and the plan says it was not, and the
panel says it is still happening. #21's requirement 6 forbids the obvious fix
(close the steps because the model answered): a step is never complete because
the model says so, and ending the turn is the model saying so.

## 2. Current State

- `run_agentic_turn` (`chat.rs:2505`) ends a turn with `StopReason::Answered`
  at two completion points: the no-tool-call answer (`chat.rs:3301`, after
  `before_task_complete` at `:3290`) and the forced-final answer (`:3259`, after
  the hook at `:3247`). An undecodable tool call is fed back as a user note and
  the loop continues (`:3308`–`:3337`), so there is no separate "empty decode"
  answer. Terminal proposals and questions also carry `Answered`, but they are
  pauses: the task becomes `WaitingForApproval` and no completion hook runs. A
  true completion writes `TaskStatus::Complete` (`chat.rs:4658`).
- The only place a step reaches a terminal status today is the `complete_step` arm
  (`chat.rs:3975`–`:4053`). It extends the open step's evidence, derives its
  status with `status_from_evidence`, opens the next pending step, and writes
  both through `update_plan_step`. `append_step_evidence` never changes a step's
  status. §5.1 adds one more terminal transition: the user's declined plan marks
  the remaining steps `skipped`.
- Evidence accrues in `step_evidence`, a local of `run_agentic_turn`
  (`chat.rs:2731`), from `evidence_for` after each tool arm (`chat.rs:4455`–
  `:4465`). Its comment (`chat.rs:2717`–`:2726`) says a crash loses "at most the
  evidence of the step that was still running". But every resume — after a
  command approval (`resume_after_command_decision_with_options`, `chat.rs:1092`,
  re-entry at `:1430`), a plan decision (`resume_after_plan_decision`, `:1462`,
  re-entry `:1555`), a question answer (`resume_after_answer`, `:1579`,
  re-entry `:1707`), a provider-fallback decision (`:1728`, re-entry `:1847`), a
  #61 *Continue* or *Answer now* (`resume_round_budget`, `:1873`, re-entries
  `:1949` and `:1983`) — enters `run_agentic_turn` again with an empty buffer.
  `PendingChatTurn` (`chat.rs:5696`) does not carry it. So does a turn that ends
  without `complete_step`: the buffer is dropped with the function.
- `SessionStore::append_step_evidence` (`session.rs:1492`) can attach evidence
  to whichever step is open, without closing it. Only `edit.rs:804` uses it.
- `PlanReport::summary` (`plan.rs:404`) reports "N steps outstanding" for a
  plan with open steps. It takes no argument and does not know whether the task
  is still running.
- `renderPlanPanel` (`app.js:7482`) labels any `in_progress` step "running"
  (`app.js:7531`–`:7536`). The plan JSON (`plan_json`,
  `desktop-shell/src/lib.rs:3923`) carries no task status; its production
  callers are the plan-review card (`:3983`), the live SSE relay (`:4067`) and
  the reloaded task states (`task_states_json`, `:4736`). The history view
  renders the panel under the turn's last assistant message (`app.js:4741`),
  where the task status is available to the caller but not passed in.
- The `propose_plan` and `complete_step` tool descriptions
  (`chat.rs:6794` and `:6802`) and the "Plan recorded" result are the only
  places the model is told to advance the plan.
- The round budget is checked before each call at `chat.rs:2874`–`:2900`
  (`agent_max_tool_rounds` through `tool_round_limit`, clamped by
  `ABSOLUTE_TURN_ROUND_CAP`); the token ceiling's pre-call check is
  `chat.rs:2841`–`:2872` (`agent_max_task_tokens` against the task's recorded
  spend). §5.2's reminder gate uses both.

## 3. Requirements

1. **The model is reminded once.** When a turn would end as answered while its
   plan has a step that is `pending` or `in_progress`, the engine does not end
   it on that response. It sends one more request with a reminder naming the
   open steps and asking the model to call `complete_step` for each finished
   one, or to answer again to end the turn with them open. The answer text from
   the response that triggered the reminder is kept, not discarded (§5.2).
2. The reminder is sent **at most once per turn**, across every resume of that
   turn. It is skipped when the round budget or the token ceiling has no room
   for another call; the turn then ends as it does today.
3. **Evidence survives pauses.** Evidence accrued for the open step is
   persisted in `PendingChatTurn` and restored on every resume (approval, plan
   decision, #61 pause, crash recovery from a pending turn), so a step closed
   after a pause is judged on everything it ran.
4. **Evidence survives the end of the turn.** When a turn truly ends with a
   step still open — answered, a budget stop, the stop button, a failure — the
   evidence accrued for that step is attached to it with
   `append_step_evidence` before the task's final status is written. A pause
   carries it instead (§5.3). The step stays open: attaching is not closing.
5. **Nothing is closed on the model's behalf.** No step changes status because
   the turn ended. #21 requirement 6 stands.
6. **The panel does not say "running" about a turn that has ended.** An
   `in_progress` step of a task that is not running is labelled "not
   finished", and the summary says the turn ended with steps open rather than
   only that they are outstanding.
7. The reminder, whether the model took it, and the open steps at the end of
   the turn are recorded in the audit log and visible in the session log.

## 4. Non-goals

- **Closing open steps automatically when the turn answers.** That is the
  "complete because the model said so" #21 requirement 6 rules out, one level
  removed. A step that ran a failing check and was never closed would be
  reported done.
- **Inferring step completion from the answer text** ("the report is written,
  so step 4 is done"). Same objection, and it needs a model call to judge.
- **Reminding on every round.** The model may legitimately work on a step for
  many rounds. A reminder per round costs tokens on every request and teaches
  the model to call `complete_step` to make the reminder go away.
- **Changing what `Outstanding` means for a token stop, the stop button or a
  plan review.** Those keep #21's behaviour. Only the label changes once the
  task is no longer running.
- **Forcing a plan.** Whether a turn plans at all stays the model's call (#21
  §7, "There is no mechanical triviality rule").

## 5. Design

### 5.1 When a turn has open steps

A plan is **open** when any step's status is `pending` or `in_progress`.
`blocked` is not open for this purpose: it is terminal for the step until the
model acts on it, and #21 already makes a blocked step stop the handoff and
report itself. (A turn can end with a blocked step; the reminder in §5.2 is
not for it, because the model was already told "Step … is blocked" in the
`complete_step` result.)

`TaskPlan::open_steps(&self) -> Vec<&PlanStep>` in `plan.rs`, tested beside
`phase()`. It replaces the two inline definitions that exist today
(`chat.rs`'s `carry_plan_from_the_previous_turn` and
`token_budget_exhausted_response`) and is the only definition the reminder uses,
so the rule has one source.

A **declined** plan is settled by the user's decision, not the model's: when the
user declines it at the review gate, the resume marks every non-terminal step
`skipped`. A skipped step is terminal, so the plan is not open, the reminder
never asks the model to complete work the user refused, and the panel reports
what happened. This is not "the model's behalf": the status records the user's
answer, and the step keeps any evidence it already had.

### 5.2 The reminder

At each completion point in `run_agentic_turn` — the no-tool-call answer
(`chat.rs:3301`) and the forced-final answer (`:3259`) — **after**
`before_task_complete` (spec 32) has allowed the completion, when

- the plan is open,
- `plan_reminder_sent` is false for this turn,
- the turn is not an *Answer now* round
  (`turn_options.round_budget_answer_now`), and
- the round budget leaves room for a tool-carrying call after the reminder:
  `action_rounds + 1 < max_rounds`, and the token ceiling's pre-call check
  would pass for that call (#21 §5.4),

the loop does not break. The budget condition is deliberately two rounds of
room, not one: the reminder round is charged before the next call, and if that
call landed at `max_rounds` the loop would force a final answer with the tools
withheld — the model could not call `complete_step` even though it was just
asked to. There are no pending proposals to check at these points: a proposal
or a question is a pause, not a completion, and never reaches this code.

The hook runs first, so it sees the answer and the plan as they stand. If it
denies (or asks for another round), the loop continues with the hook's note and
the reminder is not sent; it becomes eligible again at the next completion, with
the flag still false. If it allows and the reminder is sent, the hook runs once
more at the completion that follows the reminder round; that is the price of
asking it at every completion, and it is the same dispatch it would have done
without this spec.

The loop:

1. appends the response to `messages` as the assistant turn it was, with its
   reasoning (thinking-mode providers reject a turn without it), and appends
   that answer to the session log as an ordinary assistant message;
2. appends a user message to `messages` and to the session log:

   ```
   Your plan still has open steps:
     1. Collect every spec's declared status (in progress)
     2. Inventory worktrees and their in-progress specs
     3. Verify declared statuses against the code
     4. Write the report
   Call complete_step once for each step whose work is done — Damaian decides
   its status from what the step ran. If a step was not done, answer again
   without calling it, and the turn ends with that step open.
   ```

   The session-log entry is tagged `"kind": "plan_reminder"` so the conversation
   view renders it as a system line rather than as something the user typed,
   and so a later turn's seed does not present it as the user's own words
   (§5.5);

3. sets `plan_reminder_sent = true`, and records `plan_reminder_sent` in the
   audit log with the count of open steps;
4. charges the round like any other action round and continues.

The reminder is a **user** message, not a system one, so it sits at the end of
the request where the model reads it, and because #61's replay and
`bounded_messages` already treat user messages between rounds correctly. It is
a nudge, not state: a long post-reminder run can still elide it under
`bounded_messages`, and a model that then answers again ends the turn with the
steps open, which §5.4 reports.

The answer the model gave is kept in `messages` and in the session log. If the
model then calls `complete_step` and answers again with a short text such as
"Done.", the user must not lose the real answer behind it. So, when the turn
truly completes (no pending proposal or question), the answer that triggered the
reminder is the **turn's response** (`ChatTurnResult.response`, what the live
conversation shows as the answer). The post-reminder answer, when it is not
empty, is appended after it as a second assistant message. No heuristic decides
which one "counts": both were written by the model, in order, and the user sees
both. An empty pre-reminder answer appends nothing and is not made the
response; an empty post-reminder answer adds no second message.

When the post-reminder round does **not** truly complete — it pauses on a
command, patch, plan review or question, or ends at a budget — the loop's own
response stands, exactly as today, and the pre-reminder answer already sits
before it in the log. Rewriting the turn's response to the earlier answer would
hide the pause or the budget notice the user is being shown.

If the model calls `complete_step` after the reminder, the arm behaves exactly
as today. Each call closes the step that is `in_progress` and opens the next.
A model that wants to close four steps calls it four times, in one round or
several; `complete_step` is not batchable with reads today and this spec does
not change that.

`plan_reminder_sent` lives beside `round_budget` in `PendingChatTurn` as a
`#[serde(default)] bool`, so a turn that pauses for an approval in the
reminder round does not remind again on resume. A pending turn written before
this spec reads as `false`, which is today's behaviour plus at most one
reminder. The initial turn starts at `false`; no resume resets it.

### 5.3 Evidence across pauses and at the end

`PendingChatTurn` gains `#[serde(default)] step_evidence: Vec<Evidence>`.
Every place that builds a `PendingChatTurn` from the loop (the seven literals at
`chat.rs:3496`, `:3765`, `:4141`, `:4253`, `:4376`, `:4760` and `:4881`) writes
the current buffer, and `run_agentic_turn` seeds `step_evidence` from the
pending turn instead of `Vec::new()` when it is resuming one. The comment at
`chat.rs:2717`–`:2726` is corrected: a crash still loses the evidence of the
step that was running **unless** the turn was paused, because a paused turn now
carries it.

A pause carries, a true end settles. A command, patch, plan-review, question,
provider-fallback or round-budget pause writes the buffer into the pending turn
and the resumed loop seeds it; the step is not touched. A true end — answered,
tool budget, token budget, the stop button, a provider failure, or a declined
provider fallback — attaches the buffer to whichever step is still open with
`append_step_evidence`, before the task's final status is written. The step stays
open: attaching is not closing, and nothing recomputes its status. The
in-memory plan is extended too, so the final panel event carries the evidence.

The resume after a command approval also attaches the approved command's own
`CommandExit` (minted by `evidence_for` from its execution, with #63's
purpose), so the command the user approved is evidence for the step it served.
Today it is fed back to the model and audited but never reaches the plan. The
evidence carries the execution's own id as its `marker_id`, because the resumed
execution is not bracketed by a new action marker; that id is what
`stored_command_executed` carries, so the breadcrumb still points at the log
event with the exit.

#63 already makes an exploration exit non-blocking, so attaching a failed `grep`
does not make a reporting step look blocked when it is later closed.

### 5.4 What the panel says

The plan JSON gains `"active": bool`: true while the task's status is one in
which a turn is still going, false once it is not. The mapping is an exhaustive
`match` in `session.rs`, so a new status has to choose. The real enum has
fifteen variants:

| `active` | Statuses |
|---|---|
| true | `Created`, `PreparingContext`, `WaitingForModel`, `RunningTool`, `WaitingForApproval`, `Paused`, `ApplyingPatch`, `Validating` |
| false | `Complete`, `Failed`, `Cancelled`, `ToolBudgetExhausted`, `TokenBudgetExhausted`, `Interrupted`, `UnknownExternalOutcome` |

The last two are the correction: they are **not** `is_terminal` (recovery can
still act on them), but no turn is running, so the panel must not say
"running". The same mapping is what makes a crash-leftover plan read as "not
finished" rather than as live work.

The live stream sends `true` on every plan update during the turn and one final
plan event with `false` when the turn ends; a pause sends no final event,
because the turn is not over. The history view computes `active` from the task
status it already reads. `PlanReport::summary` takes it as an argument:

| Plan state | `active` | Summary |
|---|---|---|
| open steps | true | "N steps outstanding." (today) |
| open steps | false | "Turn ended with N steps not marked done." |
| no open steps | either | today's text |

When the plan also has blocked or finished steps, the inactive summary keeps
them: the "Turn ended …" sentence leads, and "N blocked" / "N finished" follow,
so the new wording never hides a failure. `renderPlanPanel` labels an
`in_progress` step "running" only when `active` is true, and "not finished"
otherwise. Pending steps of an inactive plan keep no label, as today. The
summary's colour follows `data-complete` as today.

The words are deliberately not "failed" or "incomplete work": the turn may
well have done the work, as in §1. "Not marked done" says what Damaian knows.

### 5.5 Recording

- Audit: `plan_reminder_sent` (task id, open-step count), and on a true turn
  end `plan_left_open` (task id, open-step count, whether the reminder was sent,
  stop reason). Neither carries step titles beyond the session log's.
- Session log: the reminder is an ordinary `message_appended` (role `user`,
  marked by a `"kind": "plan_reminder"` field so the conversation view can
  render it as a system line rather than as something the user typed). A later
  turn's `build_model_prompt` skips tagged entries, so a stale reminder is not
  presented to the model as the user's words.
- Eval harness: `RecordedPlan` gains `remindersSent` (a count, not a bool, so
  "exactly one" is measurable), `leftOpenAtEnd`, and `openStepEvidence` (items
  attached to steps still open). A planless run carries no `plan` at all, so
  none of them can read as a measured zero.
  The deterministic tier adds `plan_answer_without_complete_step`: a scripted
  model proposes a three-step plan, runs one read, answers without calling
  `complete_step`, receives the reminder, calls `complete_step` three times and
  answers "Done." It asserts one reminder, three closed steps with the read's
  evidence on step 1, the original answer as the turn's response, and "Done."
  as a second message after it. A second
  script answers twice without calling it and asserts one reminder, steps left
  open, evidence attached to the open step, and `active: false` with "not
  finished".

### 5.6 Documentation

On Done: `docs/USER_GUIDE.md` (what "not finished" and "Turn ended with N steps
not marked done" mean, and that the answer is still the answer), and
`docs/TROUBLESHOOTING.md` (the plan says a step is open after the turn ended:
not stuck; how to read `plan_left_open` and `plan_reminder_sent`).

## 6. Acceptance Criteria

- A turn whose model answers with an open plan gets exactly one reminder, and
  a turn whose plan is closed, absent or only blocked gets none.
- A plan the user declined is marked skipped and gets no reminder.
- `before_task_complete` sees the answer before the reminder; when it denies, no
  reminder is sent that round, and when it allows, the reminder is sent once.
- The reminder is not sent twice across an approval pause, a plan-review pause,
  a #61 *Continue*, an *Answer now* round, a question pause or a crash resume.
- With no room left in the round budget or the token ceiling, no reminder is
  sent and the turn ends as today.
- After the reminder, `complete_step` closes steps exactly as before, each
  status derived from evidence.
- The answer that triggered the reminder is the turn's response when the turn
  truly completes; a non-empty post-reminder reply follows it as a second
  message, and an empty one adds nothing. A pause or budget stop after the
  reminder keeps its own response and leaves the earlier answer before it in the
  log.
- Evidence accrued before an approval pause is on the step when it closes after
  the resume, and the approved command's own exit is among it.
- A turn that ends with a step open, for any reason — answered, a budget, the
  stop button, a provider failure or a declined fallback — leaves that step open
  with its accrued evidence attached; no step changes status because the turn
  ended.
- An ended turn's panel never says "running"; it says "not finished" on the
  open step and "Turn ended with N steps not marked done."
- While the turn runs, the panel is unchanged from today.
- The deterministic scenario passes in CI.
- **Live:** re-run §1's prompt against `deepseek-v4-flash` on this repository.
  Accepted if the turn either closes its steps after the reminder or ends with
  the panel reading "not finished", and the step evidence lists the commands it
  ran. Record which happened, how many tokens the reminder round cost, and
  whether the final response was the report.

## 7. Implementation Notes

### 7.1 Acceptance walk (2026-10-25)

| Criterion (§6) | Evidence | Kind |
|---|---|---|
| One reminder with open steps; none for closed, absent or only-blocked | `an_answer_with_open_steps_gets_one_reminder_and_closes_after_it`, `a_closed_plan_gets_no_reminder`, `a_planless_turn_gets_no_reminder`, `a_plan_with_only_a_blocked_step_gets_no_reminder`; eval `plan_answer_without_complete_step` (`planReminders = 1`), `planned_task` (`0`); `plan.rs`'s `a_blocked_step_is_not_open` | Automated |
| A declined plan is skipped and gets no reminder | `a_declined_plan_is_marked_skipped_and_never_reminded` | Automated |
| Hook first: no reminder when it denies; one when it allows | `a_plan_reminder_waits_for_before_task_complete`, `a_hook_that_always_denies_never_gets_a_reminder` | Automated |
| Not twice across an approval, plan review, restart, or a hook that keeps denying | `the_reminder_is_not_sent_twice_across_a_command_approval` (second engine = restart), `the_reminder_is_not_sent_twice_across_a_plan_review`, `a_hook_that_always_denies_never_gets_a_reminder`; the flag is saved and seeded by every pause literal, which the exhaustive `PendingChatTurn` literals enforce | Automated (the *Continue* and question paths share the same save/seed code, pinned by compilation and the pause tests rather than a dedicated end-to-end scenario) |
| No reminder with no round or token room | `the_reminder_gate_needs_two_rounds_of_room`, `the_reminder_gate_needs_room_under_the_token_ceiling`, `a_hook_that_always_denies_never_gets_a_reminder` | Automated (pure gate) |
| `complete_step` still closes steps from evidence after the reminder | `an_answer_with_open_steps_gets_one_reminder_and_closes_after_it` (two closed steps, read on step 1) | Automated |
| Two-answers rule | Same test (`response` is the report, "Done." is the last message) and `a_second_answer_ends_the_turn_with_the_steps_still_open` (response is the first answer, second is last) | Automated |
| Evidence before an approval is on the step after the resume, including the command's exit | `a_plan_review_pause_carries_the_evidence_and_the_reminder_flag`, `an_approved_command_reaches_the_open_step_as_evidence` | Automated |
| A turn that ends open keeps it open with its evidence, for any reason | `a_turn_that_answers_with_a_step_open_keeps_it_open_with_its_evidence`, `a_stopped_turn_keeps_its_step_open_with_its_evidence`, `a_provider_failure_keeps_its_step_open`; the engine never calls `status_from_evidence` on attach. A token-budget stop uses the same `truly_completed` branch and was not exercised end to end | Automated except the token-budget variant |
| An ended panel never says "running" | `an_ended_plan_summary_says_not_marked_done`, `an_ended_plan_with_a_blocked_step_still_names_it`, `an_ended_turn_s_plan_is_not_active`, `the_plan_event_carries_whether_the_turn_is_active`; the JS label reads `plan.active !== false`. **Checked in the real UI on 2026-10-25**: the panel read "Turn ended with 1 step not marked done, 1 finished.", labelled the open step "not finished" with no "running" label, and showed each step's evidence | Automated and manual, passed |
| While the turn runs the panel is unchanged | `the_states_whose_turn_is_still_running_are_exactly_the_non_terminal_work_states`; `plan.active: true` from `TurnProgress::Plan` and `task_states_json` for a `RunningTool` task | Automated |
| Deterministic scenarios pass in CI | The full seven-check gate passed in CI on 2026-10-25 (Quality run `40206976184`, merge commit `dd720d1`); the deterministic tier passed locally and in CI | Automated in CI |
| **Live:** §1's prompt against `deepseek-v4-flash` | **Passed 2026-10-25.** The §1 prompt verbatim, default budgets and Code mode, against a disposable clone of this branch (`1d263bf`) with an isolated data directory. The model planned four steps, ran the work, answered, received the one reminder (`openSteps: 4`), used *Continue* after the round-budget pause the reminder's charge exposed, and then closed all four steps. `TurnProgress::PlanFinished` arrived once; no `plan_left_open` was written. Step 1 carried 20 evidence items; steps 2–4 closed unverified. 27 model calls, 2,218,346 input (1,934,208 cached) and 17,662 output tokens. The reminder round itself cost **80,774 input (66,432 cached) and 249 output**. The final response was the report. | Manual, passed |

### 7.2 Implementation discoveries

- **A one-step plan is not decodable.** `tool_action_from_call` returns `None`
  for a `propose_plan` with fewer than two steps (`chat.rs`, `§5.1`), so every
  turn-level test needs two steps even when it only cares about step 1.
- **A mutating command cannot test the plan review and the command approval at
  once.** The review gate fires first on any mutating action while the plan is
  unapproved, so a "reminder then command pause" test must approve the plan
  first; the reminder then fires in the resumed turn.
- **The reminder at the forced-final completion is unreachable, by design.** A
  forced-final call has `action_rounds >= max_rounds`, so the two-round gate is
  always false. The call is still made at both completion points so the rule has
  one definition.
- **A blocked step could be handed off by a second `complete_step`.** The claim
  in §5.2 that #21 already stops the handoff held only for the first call: with
  no step `InProgress`, the second call left the status at its default
  `Completed` and started the next pending step. Fixed in #21's `chat.rs` path
  on 2026-10-26
  (`a_second_complete_step_after_a_block_is_refused_and_starts_nothing`);
  §5.2's rule is now true for every call.
- **`TaskStatus::is_active` is not `!is_terminal()`.** `Interrupted` and
  `UnknownExternalOutcome` stay recoverable but are inactive; a test pins the
  partition.
- **A turn ended by a declined provider fallback settles outside the loop.** The
  plan is read from the log and the evidence from the pending turn; this is the
  only true end that runs no loop code.
- **Attaching is many events, not one.** A turn that ends with twelve command
  results writes twelve `plan_step_updated` records; the log is append-only and
  the plan is small (also recorded in `context.md` §7).
- **The JS label has no automated check.** There is no JavaScript test harness;
  the contract (`active`) is tested, and the rendered panel was checked manually
  on 2026-10-25 through the real web UI against a seeded completed task with an
  open step. The browser showed "not finished" and the "Turn ended with 1 step
  not marked done, 1 finished." summary, with no "running" label.
- **The live prompt needs approvals, so the accepted run used a disposable
  clone.** In Code mode §1's prompt asks for a shell-control read
  (`ls … && grep … | head`), which pauses for approval; two Plan-mode attempts
  refused it and the model then answered without ever planning. The accepted run
  drove Code mode with approvals granted, against a clone of the committed
  branch, so no command could touch the working checkout. The clone lived under
  `target/`, which the model noticed and called a fixture copy; that is an
  artefact of the isolation method, not of the engine.
- **The reminder consumed the last action round and the turn paused at its round
  budget.** The live run needed *Continue* before the model could close its
  steps. That is the two-round gate doing what §5.2 asks — it left room for the
  reminder's own round, not a further one — and it makes #61's continuation part
  of the happy path on a long planned turn. The driver granted *Continue*; a
  user would have seen the round-budget card.
- **The reminder itself is one measured model call.** On the live run it cost
  80,774 input (66,432 cached) and 249 output tokens; the entire 27-call turn
  cost 2,218,346 input (1,934,208 cached) and 17,662 output.
