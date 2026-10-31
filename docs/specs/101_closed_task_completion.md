# Feature Spec: A Worker Completes Only a Closed Task

Status: Not started
Order: 101 of 106
Plan: none. This is a failure-driven specification from the 2026-10-31
attempt to implement the reserved spec 86 with one Damaian worker.
Depends on: [#21](21_task_plan_progress_and_budget/proposal.md) (durable plans),
[#23](23_verification_loop/proposal.md) (verification outcomes),
[#66](66_plan_left_open_at_turn_end/proposal.md) (the existing one-reminder
behaviour), and [#99](99_worker_task_outcomes.md) (worker outcomes), all built.
Everything else named below is a cross-reference, not a prerequisite.

## 1. Motivation

A model answer is not proof that its task is finished. Today
`StopReason::Answered` becomes `TaskStatus::Complete` after #66's one reminder
even when the carried plan still has open implementation or validation steps.
#99 then presents that durable status as **Task complete**.

The spec 86 attempt demonstrated the consequence: Damaian produced a
specification commit and a partial `chat.rs` commit, but never landed all of the
planned code, tests, documentation and checks. The execution record had no
engine-owned condition that could distinguish that partial result from a
finished task.

## 2. Requirements

1. `TaskStatus::Complete` is written only when an engine-owned completion gate
   says the task is closed. A model cannot assert or tool-call its way around
   the gate.
2. A planned task is closed only when every non-verification plan step is
   terminal, no verification is due or running, no proposal or question is
   pending, and every required check known to the task has a current terminal
   disposition.
3. A skipped or declined check counts only as an explicit user waiver. It is
   never represented as passed, and the completion report names the waiver.
4. When the model answers after #66's reminder while work remains, the task
   becomes a new durable `incomplete` status rather than `complete`. The open
   plan and the reasons from the completion gate remain attached.
5. `incomplete` is terminal for the current run but resumable. An ordinary
   session and a worker both offer **Continue task**; continuing carries the
   same approved plan and completion reasons into the next run.
6. A worker progress record and #99's outcome renderer distinguish
   **Complete** from **Incomplete**. A result returned through #100 carries the
   truthful status; #100 does not translate `incomplete` into success.
7. Recovery recomputes the gate from durable state. A crash between the final
   model answer and the status write cannot turn an open task into complete.
8. Existing approval boundaries are unchanged. The completion gate may stop or
   continue work, but it cannot approve a patch, command, check or question.

## 3. Non-goals

- Making the model finish an arbitrarily difficult task. This spec makes the
  outcome truthful and resumable; #103 supplies longer bounded execution.
- Deciding whether a failing check was caused by the task. That is #102.
- Defining required checks from prose. #105 makes approved checks structured.
- Treating every open finding in the session as a blocker. Findings from an
  earlier task or a pre-existing failure may remain; the gate uses the current
  task's verification disposition.

## 4. Design

Add a `CompletionGate` report computed from the task, active plan, verification
state, pending links and required-check evidence. It contains a boolean
`eligible` plus stable reason codes such as `open_plan_steps`,
`verification_due`, `pending_decision`, `required_check_missing` and
`required_check_failed`.

The final `StopReason::Answered` settlement calls the gate. Eligible work keeps
today's `complete`; ineligible work writes `incomplete` and stores the reason
codes in the task-status event. The final response remains in the transcript —
it may be useful — but is not treated as the execution verdict.

`TaskStatus::Incomplete` is terminal but not successful. It is included in
session listing, progress reconstruction, worker return, task reports and
recovery classification. Continuing it follows the same durable-plan carry
path as a budget continuation, without granting more approvals.

## 5. Acceptance

- A model that answers twice with an open plan receives one reminder and ends
  `incomplete`, with the plan unchanged.
- Completing the remaining steps and current required checks changes the next
  run to `complete`.
- A declined required check produces a named waiver, never pass evidence.
- A due verification, pending patch, pending question or open required step
  prevents `complete` in ordinary and worker sessions.
- Reloading an incomplete worker shows **Incomplete — Continue task** and
  continuing it carries the same plan.
- No historical task already stored as complete is rewritten during migration;
  the stricter rule applies when new status events are settled.
