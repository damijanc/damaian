# Feature Spec: A Plan Left Open When the Turn Ends

Status: Not started. Specified on 2026-10-24 from a reporting-only session
that answered in full, ended `complete`, and left its plan panel showing the
first of four steps as **running** and "4 steps outstanding."
Order: 66 of 66
Plan: none. Evidence-driven, like [#47](47_agent_working_capability/proposal.md)
— an exception to the rule in [`README.md`](README.md).
Depends on: [#21](21_task_plan_progress_and_budget/proposal.md) (the plan, its
evidence and its report) — built; [#17](17_durable_task_state_and_crash_recovery/proposal.md)
(`PendingChatTurn` and resume) — built. Everything else named below is a
cross-reference, not a prerequisite.
Related implementation specs:
[`61_continue_the_stopped_turn/`](61_continue_the_stopped_turn/proposal.md) (a
paused turn is a `PendingChatTurn`; this spec adds to what it carries),
[`59_exploration_round_allowance/`](59_exploration_round_allowance/proposal.md)
(the round budget the reminder in §5.2 must fit inside),
[`63_exploratory_command_failures/`](63_exploratory_command_failures/proposal.md)
(an exploration exit is evidence that does not block a step, which §5.3 relies
on).

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

- `run_agentic_turn` (`chat.rs:1983`) ends a turn with `StopReason::Answered`
  whenever a model response carries no decodable tool call (`chat.rs:2703`,
  `2732`). It does not look at the plan. The task becomes `Complete`
  (`chat.rs:3788`).
- The only place a step reaches a terminal status is the `complete_step` arm
  (`chat.rs:3252`). It extends the open step's evidence, derives its status
  with `status_from_evidence`, opens the next pending step, and writes both
  through `update_plan_step`.
- Evidence accrues in `step_evidence`, a local of `run_agentic_turn`
  (`chat.rs:2209`), from `evidence_for` after each tool arm (`chat.rs:3619`,
  `3625`). Its comment says a crash loses "at most the evidence of the step
  that was still running". But every resume — after a command approval
  (`chat.rs:1275`), a plan decision (`chat.rs:1401`), a #61 *Continue* — enters
  `run_agentic_turn` again with an empty buffer. `PendingChatTurn` does not
  carry it. So does a turn that ends without `complete_step`: the buffer is
  dropped with the function.
- `SessionStore::append_step_evidence` (`session.rs:1492`) can attach evidence
  to whichever step is open, without closing it. Only `edit.rs:668` uses it.
- `PlanReport::summary` (`plan.rs:404`) reports "N steps outstanding" for a
  plan with open steps. It does not know whether the task is still running.
- `renderPlanPanel` (`app.js:7126`) labels any `in_progress` step "running"
  (`app.js:7175`). The plan JSON (`desktop-shell/src/lib.rs:3674`) carries no
  task status. The history view renders the panel under the turn's last
  assistant message (`app.js:4409`), where the task status is known to the
  caller but not passed in.
- The `propose_plan` and `complete_step` tool descriptions (`chat.rs:5652`,
  `5657`) and the "Plan recorded" result are the only places the model is told
  to advance the plan.

## 3. Requirements

1. **The model is reminded once.** When a turn would end as answered while its
   plan has a step that is not `completed` or `skipped`, the engine does not end
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
4. **Evidence survives the end of the turn.** When a turn ends with a step
   still open, for any reason, the evidence accrued for that step is attached
   to it with `append_step_evidence` before the task's final status is
   written. The step stays open: attaching is not closing.
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
`phase()`.

### 5.2 The reminder

At each `StopReason::Answered` break in `run_agentic_turn` (both: the
no-tool-call response at `chat.rs:2703` and the empty decode at `2732`), when

- the plan is open,
- `plan_reminder_sent` is false for this turn,
- there are no pending proposals (a command, patch or plan review), and
- one more round fits both the round budget (#59's action budget,
  `agent_max_tool_rounds`) and the token ceiling's pre-call
  check (#21 §5.4),

the loop does not break. It:

1. appends the response to `messages` as the assistant turn it was, with its
   reasoning (thinking-mode providers reject a turn without it);
2. appends a user message:

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

3. sets `plan_reminder_sent = true`, and records `plan_reminder_sent` in the
   audit log with the count of open steps;
4. charges the round like any other action round and continues.

The reminder is a **user** message, not a system one, so it sits at the end of
the request where the model reads it, and because #61's replay and
`bounded_messages` already treat user messages between rounds correctly.

The answer the model gave is kept in `messages` and in the session log. If the
model then calls `complete_step` and answers again with a short text such as
"Done.", the user must not lose the real answer behind it. So the answer that
triggered the reminder is the **turn's response** (what `ChatTurnResult`
returns and what the conversation shows as the answer). The post-reminder
answer, when it is not empty, is appended after it as a second assistant
message. No heuristic decides which one "counts": both were written by the
model, in order, and the user sees both.

If the model calls `complete_step` after the reminder, the arm behaves exactly
as today. Each call closes the step that is `in_progress` and opens the next.
A model that wants to close four steps calls it four times, in one round or
several; `complete_step` is not batchable with reads today and this spec does
not change that.

`plan_reminder_sent` lives beside `round_budget` in `PendingChatTurn` as a
`#[serde(default)] bool`, so a turn that pauses for an approval in the
reminder round does not remind again on resume. A pending turn written before
this spec reads as `false`, which is today's behaviour plus at most one
reminder.

### 5.3 Evidence across pauses and at the end

`PendingChatTurn` gains `#[serde(default)] step_evidence: Vec<Evidence>`.
Every place that builds a `PendingChatTurn` from the loop (each site that sets
`round_budget`, `chat.rs:2906`–`4022`) writes the current buffer, and
`run_agentic_turn` seeds `step_evidence` from the pending turn instead of
`Vec::new()` when it is resuming one. The comment at `chat.rs:2195` is
corrected: a crash still loses the evidence of the step that was running
**unless** the turn was paused, because a paused turn now carries it.

The resume after a command approval also attaches the approved command's own
`CommandExit` (minted by `evidence_for` from its execution, with #63's
purpose), so the command the user approved is evidence for the step it served.
Today it is fed back to the model and audited but never reaches the plan.

When the turn ends for any reason other than a pause — answered, tool budget,
token budget, the stop button — and the plan has an `in_progress` step and the
buffer is non-empty, the loop calls `append_step_evidence` for that step before
`update_task_status`. #63 already makes an exploration exit non-blocking, so
attaching a failed `grep` does not make a reporting step look blocked when it
is later closed.

### 5.4 What the panel says

The plan JSON gains `"active": bool`: true while the task's status is
non-terminal (`Created`, `PreparingContext`, `WaitingForModel`, `RunningTool`,
`WaitingForApproval`, `Paused`, `ApplyingPatch`, `Validating`), false once it
is `Complete`, `Failed`, `Cancelled`, `ToolBudgetExhausted` or
`TokenBudgetExhausted`. The mapping is an exhaustive `match` in `session.rs`,
so a new status has to choose. The live stream
sends `true` until the final plan event of the turn, which sends `false`; the
history view computes it from the task it already has. `PlanReport::summary`
takes it as an argument:

| Plan state | `active` | Summary |
|---|---|---|
| open steps | true | "N steps outstanding." (today) |
| open steps | false | "Turn ended with N steps not marked done." |
| no open steps | either | today's text |

`renderPlanPanel` labels an `in_progress` step "running" only when `active` is
true, and "not finished" otherwise. Pending steps of an inactive plan keep no
label, as today. The summary's colour follows `data-complete` as today.

The words are deliberately not "failed" or "incomplete work": the turn may
well have done the work, as in §1. "Not marked done" says what Damaian knows.

### 5.5 Recording

- Audit: `plan_reminder_sent` (task id, open-step count), and on turn end
  `plan_left_open` (task id, open-step count, whether the reminder was sent,
  stop reason). Neither carries step titles beyond the session log's.
- Session log: the reminder is an ordinary `message_appended` (role `user`,
  marked by a `"kind": "plan_reminder"` field so the conversation view can
  render it as a system line rather than as something the user typed).
- Eval harness: each run record gains `planOpenAtEnd` and `planReminderSent`.
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
- The reminder is not sent twice across an approval pause, a plan-review pause,
  a #61 *Continue* or a crash resume.
- With no room left in the round budget or the token ceiling, no reminder is
  sent and the turn ends as today.
- After the reminder, `complete_step` closes steps exactly as before, each
  status derived from evidence.
- The answer that triggered the reminder is the turn's response; a non-empty
  post-reminder reply follows it as a second message, and an empty one adds
  nothing.
- Evidence accrued before an approval pause is on the step when it closes after
  the resume, and the approved command's own exit is among it.
- A turn that ends with a step open, for any reason, leaves that step open with
  its accrued evidence attached; no step changes status because the turn ended.
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

None yet.
