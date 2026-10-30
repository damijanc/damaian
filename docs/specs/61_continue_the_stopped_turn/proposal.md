# Feature Spec: Continue the Stopped Turn

Status: Done 2026-10-23 (started 2026-10-18). The pause, *Continue* and *Answer
now*, the hard cap, crash recovery and worker answering, the UI row, the *Ask
again* path and the `continue_keeps_state` scenario are built; plan and
progress in [`tasks.md`](tasks.md). Specified on 2026-10-17 from the session
behind [#59](../59_exploration_round_allowance/proposal.md) and
[#60](../60_worktree_aware_read_tools/proposal.md): three presses of *Continue
debugging* produced three new turns that had lost the user's request, drifted
into debugging a worktree, and still never answered.
Order: 61 of 61
Plan: none. Evidence-driven, like #47 — an exception to the rule in
[`README.md`](../README.md).
Depends on: [#17](../17_durable_task_state_and_crash_recovery/proposal.md) (pending approvals reattach after a
crash) — built; [#21](../21_task_plan_progress_and_budget/proposal.md) (the round
budget, the plan and its carry-over) — built;
[#47](../47_agent_working_capability/proposal.md) (requirement 6, the bounded
continuation this completes) — built. Everything else named below is a
cross-reference, not a prerequisite.
Related implementation specs:
[`55_conversation_compaction/proposal.md`](../55_conversation_compaction/proposal.md) (keeps the
objective across turns; explicitly not the in-turn array, §4),
[`57_worker_sessions`](../57_worker_sessions/proposal.md) (a worker answers
approvals in its own process),
[`59_exploration_round_allowance`](../59_exploration_round_allowance/proposal.md)
(changes which rounds count; this spec changes what happens when they run out).

## 1. Motivation

When a turn runs out of tool rounds, the only way on is *Continue debugging*,
and it does not continue anything. It sends a fixed prompt — "Continue debugging
from the last tool-budget exhaustion. Use the prior session evidence and continue
from the unresolved diagnostic state." — as a **new turn**. The new turn sees
the last eight session messages, each cut to 2,000 characters, and nothing else
from the turn that stopped: not its tool results, not its plan, and, once the
stopped turn has written more than eight messages, not the user's request.

On 2026-10-17 the user asked for a report and added "this is a reporting only
task, no code changes". The first turn wrote thirty messages before it stopped,
so the request was gone by the second. The second, third and fourth turns,
primed by "unresolved diagnostic state" and a window full of one worktree's
diffs, debugged spec 25's worktree and proposed `cargo check` and
`cargo nextest`. None of them wrote the report. Each started over from about
70k tokens of context it had to rebuild with fresh tool calls.

#47 named this exactly: the round cap is "a cliff whose only continuation is a
manual re-ask that starts a new task and loses in-turn state". Its requirement 6
fixed it only for users who set a token ceiling. With the default configuration,
which sets none, the cliff is still there.

## 2. Current State

- At the cap, `force_final` makes one more model call with tools dropped. If the
  model still asks for a tool, the turn stops with `StopReason::ToolBudget` and
  `TaskStatus::ToolBudgetExhausted`, which is terminal (`session.rs`,
  `is_terminal`). The forced call's output is replaced by
  `tool_budget_exhausted_response` (called at `chat.rs:2421`, defined at
  `chat.rs:5630`).
- With `agent_max_task_tokens` set, #47's continuation adds a segment of
  `agent_max_tool_rounds` inside the same turn instead, up to
  `ABSOLUTE_TOOL_ROUND_CAP`, and records `turn_continued` (`chat.rs:2400`).
- *Continue debugging* (`continueDebuggingFromExhausted`, `app.js:10119`) posts
  the fixed prompt with `continue_debugging: true`. The engine creates a new task
  (`chat.rs:807`), raises its round limit to `ABSOLUTE_TOOL_ROUND_CAP`
  (`tool_round_limit`, `chat.rs:568`), and forces `web_debug_mode` on
  (`chat.rs:1942`), whatever the original request was.
- `build_model_prompt` (`chat.rs:4445`) gives a new turn the last eight messages,
  each truncated to 2,000 characters.
- `carry_plan_from_the_previous_turn` (`chat.rs:1713`) carries the plan only from
  a `TokenBudgetExhausted` or superseded task. A tool-round stop drops it.
- A turn paused for approval is persisted as a `PendingChatTurn`
  (`chat.rs:4163`) at `<data_dir>/chat/pending/<proposal_id>.json`, with its
  `messages`, `round`, `turn_options` and plan review, and resumes into
  `run_agentic_turn` with the same task. `PendingApprovalRef.kind` is
  `command`, `patch`, `plan` or `provider_fallback` (`session.rs:244`), and
  crash recovery reattaches each kind (`reattach_pending_approvals`,
  `recovery.rs:311`).
- The existing absolute cap is `ABSOLUTE_TOOL_ROUND_CAP` (`chat.rs:4372`), value
  **16**, and the default segment is `agent_max_tool_rounds` (`config.rs:2570`),
  value **8**. §5.3's `ABSOLUTE_TURN_ROUND_CAP` is a new, distinct constant.

## 3. Requirements

1. A turn that reaches its round limit with the model still wanting a tool
   **pauses** with its state intact rather than ending. Its task waits on the
   user, as it does for a command approval.
2. The user can choose to **continue**, which grants the same turn another
   `agent_max_tool_rounds` and resumes it with every message, tool result and
   plan step it had, or to **answer now**, which makes the forced final call
   with tools dropped and ends the turn with whatever that call produces.
3. No fixed "continue debugging" prompt is sent, and continuing does not switch
   on web debug mode.
4. A turn's total rounds are hard-bounded across all continuations, so a
   repeatedly continued turn still ends.
5. Each continuation is recorded: who granted it, how many rounds, and the
   running total.
6. A paused turn survives a crash or restart and can still be continued or
   answered, or fails with a stated reason as other unavailable approvals do.
7. A turn stopped at the cap under the previous behaviour — a stored
   `ToolBudgetExhausted` task — still offers a way on, and that way carries the
   original request and plan into the new turn.
8. A user who sets a token ceiling keeps #47's automatic continuation; the pause
   applies where that continuation does not.

## 4. Non-goals

- Keeping the objective across separate turns in general. That is
  [#55](../55_conversation_compaction/proposal.md); this spec keeps one turn alive instead
  of replacing it.
- Changing what counts as a round ([#59](../59_exploration_round_allowance/proposal.md))
  or the default `agent_max_tool_rounds`.
- Automatic continuation without a token ceiling. Without one, the user's click
  is the only bound on spending, and that is the point of asking.
- Enforcing a user's stated constraint ("no code changes") mechanically. The
  session's mode does that ([#20](../20_working_modes/proposal.md)); this spec only
  stops the request from being lost.

## 5. Design

### 5.1 Pause instead of stop

At `round >= max_rounds`, before the forced final call, and only when #47's
continuation does not apply (no token ceiling, or the cap is reached):

1. The loop saves a `PendingChatTurn` with a new field
   `round_budget: Some(RoundBudgetPause { rounds_used, rounds_granted })`,
   beside the existing `provider_fallback` field, holding `messages`, `round`,
   the plan and `turn_options` as every other pause does.
2. It records a pending approval with `PendingApprovalRef { kind:
   "round_budget", proposal_id }` and sets the task to `WaitingForApproval`.
3. The turn's partial answer is kept as it is for any pause.

The pause comes **before** the forced call, not after it. Today the forced call
is made, and if the model still wants a tool its output is thrown away and
replaced by the stop message — the most expensive call of the turn, spent on
text nobody sees. Pausing first means that call is made only if the user chooses
*Answer now*.

Nothing is in flight at that point: the stop happens before the requested tool
is dispatched. That is what makes the pause safe to persist and to recover (#17
§5.4).

### 5.2 Continue and Answer now

A new resume entry point, `resume_round_budget(proposal_id, decision)`, takes the
pending turn and:

- **Continue**: adds `agent_max_tool_rounds` (web debug: its own limit) to the
  turn's `round_budget_extension`, appends one user message — "You have N more
  tool rounds. Continue the task." — and re-enters `run_agentic_turn` with the
  saved messages and round, the same task and the same plan. The model was
  stopped before its requested call was dispatched, so it simply asks again.
- **Answer now**: re-enters with `force_final` set, so the next call is the
  forced final call with tools dropped, and the turn ends as today: answered, or
  `ToolBudgetExhausted` if the model still asks for a tool.

`resume_round_budget` refuses a pending turn of any other kind, and the existing
`resume_after_command_decision` / `resume_after_plan_decision` /
`resume_after_fallback_decision` refuse a `round_budget` one, mirroring the rule
specs 23 and 50 record for their own kinds (C5). A pause can only be answered by
its own entry point.

`round_budget_extension` is a *local* of `run_agentic_turn` (`chat.rs:2139`), not
part of the persisted turn. The `round_budget` field's `rounds_granted` is the
running total, and the resumed turn seeds `round_budget_extension` from it so its
`max_rounds` continues where it stopped rather than resetting to the base limit.

The request the user typed reaches the model inside the assembled prompt, which
is the **second** message of the saved array (`[system prompt, user request, …]`,
`chat.rs:889`), not the first; the raw text is also held on `task.user_prompt`.
Nothing about it depends on the eight-message window, because the in-turn
`messages` array is what is replayed, not `build_model_prompt`'s sliding window.

### 5.3 The hard bound

`ABSOLUTE_TURN_ROUND_CAP` (`chat.rs:4546`), value 64, bounds the rounds of one
turn across all continuations — the initial eight-round segment plus seven
continuations of eight. At the bound, the pause offers only *Answer now*. It is
distinct from the existing `ABSOLUTE_TOOL_ROUND_CAP` (`chat.rs:4539`, value 16),
which bounds a single configured segment and #47's token-ceiling continuation.

> **Amended 2026-10-30.** `agent_max_tool_rounds` now defaults to 16, the
> segment ceiling, and `agent_web_debug_max_tool_rounds` to 16 with it (it was
> 12, and `web_debug_tool_round_limit` already raised it to the default). At
> 8, spec 59's measured implementation tasks (27–78 action rounds, median 41)
> paused about five times each; at 16 the median task pauses two or three
> times. The bound of 64 is unchanged and is now four segments. Neither
> ceiling moved, so a repository can set no more than it could before. The
> deterministic scenarios that count rounds against the budget
> (`continue_keeps_state`, `read_loop`, `answer_now_ends_the_turn`,
> `long_discovery`, `failed_validation_retry`) pin `agent_max_tool_rounds=8` in
> their `config_overlay`, because the pause is what they test, not the default.
> The full seven-check gate passed in CI on 2026-10-30 for the `main` push that
> contains this amendment (Quality run 41034494208).

Each request inside the turn stays bounded by `agent_max_turn_messages` through
`bounded_messages`, so continuing grows the number of calls, not their size.
The token ceiling, when set, still stops the turn before the call that would
cross it.

### 5.4 Recording

- An audit record `turn_budget_extended` with `actor: user`, `round`,
  `roundsGranted`, `roundsTotal`, mirroring #47's `turn_continued` (whose actor
  is `system`).
- The pending approval and its resolution go through the existing approval
  events, so the session log shows the pause and the decision like any other.
- The eval harness counts continuations per run.

### 5.5 Crash recovery and workers

- `reattach_pending_approvals` gains a `round_budget` arm that reads the pending
  turn through `PausedTurns`, as the `plan` and `provider_fallback` arms do. A
  missing or unreadable file fails the task with a stated reason and audits
  `pending_approval_unavailable`, as for the other kinds.
- A worker session (#57) answers the pause in its own process, through the same
  in-process approval path its command approvals use. The worker's socket
  `approve` command means *Continue* and `reject` means *Answer now*; both
  resume the turn through `resume_round_budget`.

### 5.6 What the user sees

Under the turn, in place of *Tool budget exhausted* and *Continue debugging*:

> **Used 8 of 8 tool rounds.** [Continue — 8 more] [Answer now]

The row names the rounds used so far in this turn and, after a continuation,
the total. It sits where the turn indicator is (#58), and the button labels say
nothing about debugging. While paused, the task shows as waiting on the user,
in the conversation and on the Workers board.

### 5.7 Turns stopped before this spec

A stored `ToolBudgetExhausted` task has no saved turn to resume. Its row keeps a
button, renamed *Ask again*, which starts a new turn whose prompt is built by the
engine, not the UI: "Continue this earlier request: <the stopped task's
user_prompt>". `carry_plan_from_the_previous_turn` also carries the plan from a
`ToolBudgetExhausted` task, as it does from `TokenBudgetExhausted`. The
`continue_debugging` turn option no longer forces web debug mode in any path;
the option is read in exactly two places today — `tool_round_limit`
(`chat.rs:541`) and the `web_debug_mode` seed (`chat.rs:1906`) — so both are
touched, and the field (`chat.rs:424`) is removed or repurposed.

The same path applies to a turn that ends `ToolBudgetExhausted` under this spec
— after *Answer now* when the model still asked for a tool, or at the hard bound.

### 5.8 Eval scenario

A deterministic scenario, `continue_keeps_state`: a scripted turn that needs
twelve rounds. With the default limit it pauses at eight. The harness answers
*Continue*, and the assertions are: the same task id finishes, the model's ninth
request contains the first turn's tool results and the user's original prompt,
`turn_budget_extended` is recorded once, and no second task exists. A second
case answers *Answer now* and asserts one forced final call and the
`ToolBudgetExhausted` or answered outcome.

Driving this needs two things beyond the scenario file: the round-budget pause
surfaces on `ChatTurnResult` as a new field (parallel to `fallback_proposal`),
and the runner's `drive` gains a scenario option — analogous to
`approval_decision` — that calls `resume_round_budget` and keeps driving until
the turn finishes. The twelve-round script must name all twelve responses:
`MockModelAdapter` repeats its last entry when the sequence is exhausted, so an
undersized script would end the turn at whatever round it ran out on. §5.4's
"count continuations per run" is recorded here too.

### 5.9 Documentation

On Done: `docs/USER_GUIDE.md` (the budget section: what the pause is and what
the two buttons do), `docs/TROUBLESHOOTING.md` (a task waiting on
`round_budget`, and what *Ask again* sends).

## 6. Acceptance Criteria

- With defaults, a turn that reaches eight rounds wanting more shows the pause
  with *Continue* and *Answer now*; its task is `WaitingForApproval`, not
  terminal.
- *Continue* resumes the same task; the next model request contains every
  message the turn had, including the user's original request and all tool
  results (subject to `bounded_messages`); `turn_budget_extended` is recorded.
- *Answer now* makes exactly one model call with no tools, and the turn ends.
- No model call is made between reaching the limit and the user's choice.
- No path sends the fixed "Continue debugging…" prompt, and continuing never
  enables web debug mode.
- At `ABSOLUTE_TURN_ROUND_CAP` only *Answer now* is offered.
- Killing the app while paused and restarting shows the same pause; choosing
  *Continue* then resumes with the saved messages.
- A worker session's pause is answered in the worker without the shell
  reading the pending file.
- With a token ceiling set, #47's automatic continuation is unchanged and its
  tests pass.
- A stored `ToolBudgetExhausted` task offers *Ask again*, whose new turn's prompt
  contains the original request and whose plan is carried over.
- `continue_keeps_state` passes.

## 7. Implementation Notes

The per-task record is in [`tasks.md`](tasks.md), including each deliberate
deviation and the falsification that checked the load-bearing tests.

- The pause runs before the forced final call, so a turn with no token ceiling
  never spends that call until the user chooses *Answer now*. §5.1's ordering
  is what makes "no model call between the limit and the choice" hold.
- `RoundBudgetPause`, `AgentRoundBudgetProposal`, `RoundBudgetDecision` and
  `resume_round_budget` are public because `ChatTurnResult`, `recovery.rs` and
  the worker read them. `ChatTurnOptions` carries the `round_budget_extension`
  seed and the `round_budget_answer_now` bypass; both are `#[serde(default)]`,
  so existing callers are unchanged.
- `ABSOLUTE_TURN_ROUND_CAP` is 64 and measured in total rounds, not in
  `rounds_granted` (which tops out at 56 after seven continuations of eight).
- The offer is `createRoundBudgetOffer` in `app.js`. A reattached pause reads
  only `rounds_used` and `rounds_granted` from `PausedTurns`, so its hard-cap
  check repeats the 64-round constant and it omits the figures it does not
  know.
- Task 7 wrote §5.9's documentation in `docs/USER_GUIDE.md` and
  `docs/TROUBLESHOOTING.md`; the workspace clippy and test suite are left to
  CI.
