# Feature Spec: Worker Return and Inline Choice

Status: Done 2026-10-31 (specified and built the same day; local checks in §6,
the full gate is left to CI).
Order: 100 of 100
Plan: none. A workflow defect reported in use, following #99.
Depends on: [#57](57_worker_sessions/proposal.md) (worker lifecycle),
[#76](76_worktree_tasks_go_to_a_worker.md) (worktree routing), and
[#99](99_worker_task_outcomes.md) (durable worker outcomes), all built.

## 1. Motivation

Workers were intentionally one-way in #57: its non-goals say that nothing
flows from a worker to its parent. That leaves the parent conversation at the
model-suggested `worker-prompt` while the child eventually reaches an idle,
terminal task state in another worktree. The user has to notice the child,
interpret its state, open the parent, and reconstruct what should happen next.
That is not a handoff.

The same prompt contract makes a worker suggestion look like a refusal to work
in the current session. A worker is useful isolation, not a mandatory routing
decision. The user must be able to choose it, or explicitly ask the current
session to do the work here.

## 2. Requirements

1. When a worker task reaches a terminal outcome without a pending approval,
   it returns one durable, redacted result to its parent session when it has
   one. The return identifies the worker session, branch and worktree, task
   status, and the worker's final response. It never copies raw reasoning,
   tool output, credentials, or a pending proposal into the parent.
2. A return is idempotent per worker task. Restarting a worker, reloading the
   parent, or re-reading the progress file must not duplicate it. A task that
   is waiting for a decision does not return until the decision leads to a
   terminal outcome.
3. The parent conversation renders the return as a worker-result card linked
   to the worker. Its explicit next action is **Continue with result**. That
   action starts a normal parent turn with the returned result as a bounded,
   delimited user message. It does not merge a branch, apply a patch, or
   approve an action; those remain ordinary explicit approvals.
4. Returning a result never starts a model call automatically. The return is
   durable communication from the worker process to the parent session; the
   user chooses whether and when its parent spends tokens or acts on it.
5. A model-suggested `worker-prompt` block offers both **Start worker** and
   **Work here instead**. The latter sends the task to the current session as
   an ordinary new turn, with an explicit instruction that the user chose not
   to use a worker and that the work belongs in the current checkout.
6. The worker-routing prompt may recommend isolation, but must not override
   an explicit user choice to work in the current session. A request that
   names a separate worktree or branch still explains the consequence: work
   here uses the current checkout; it does not create or select another
   worktree.
7. The choice is available only on assistant-authored, exactly-tagged
   `worker-prompt` blocks in a non-worker session, like the existing start
   action. It is absent when worker sessions are disabled and in worker
   sessions. No free-form pasted block gains an action.
8. The UI makes the choice clear without presenting either action as approval
   of a patch, command, plan, merge, or branch mutation.

## 3. Non-goals

- Automatically merging, rebasing, applying, or deleting a worker worktree.
- Automatically starting a parent model turn, which would spend tokens and
  could create a new proposal without a user action.
- Returning an incomplete live stream, raw reasoning, or unredacted command
  output to the parent.
- Letting workers spawn workers.

## 4. Design

- `SessionStore` gains a typed, append-only worker-return event and a query
  that returns only the unconsumed return records for a parent. Its payload is
  built from the worker record and final task outcome, then scanned before
  persistence. A consumed marker records the parent task that handled it.
  Session-log appends take a per-session `flock` sidecar lock, because the
  worker process and the shell can append to one parent log. The sidecar also
  holds the authoritative last reserved sequence; an append advances and writes
  that reservation before appending the log, so it never reparses the JSONL.
  Only a missing or invalid counter initializes from the log once. The return's
  existence check and append happen under the same lock.
- The worker turn driver writes the event after it has persisted its own task
  outcome and only for a terminal task with a parent. That includes the error
  path: when the engine persists `Failed` and returns `Err`, the driver reads
  the newest task, and if it is terminal returns it with the latest durable
  assistant text or the error. Failure to write the parent notification is
  visible as a worker progress/error outcome and does not change the worker
  task's own outcome.
- The shell exposes pending parent returns with the normal session payload and
  an endpoint that consumes exactly one return and streams an ordinary parent
  turn. It resolves config and the model adapter first, then one locked event
  both consumes the return and carries the complete parent task with its final
  id and canonical prompt. That single `worker_return_consumed` line is the
  durable record; task replay treats it as the task creation fact. The turn runs
  that already-created task. A second browser tab or a restarted process sees
  the return consumed and starts nothing.
- `app.js` draws the card beside the parent conversation and provides
  **Continue with result** plus a worker link. The worker-prompt footer grows a
  secondary **Work here instead** action. That action reuses the ordinary
  streaming-chat path with a fixed, user-visible instruction rather than a
  second execution path.
- `WORKTREE_TASK_PARAGRAPH` becomes a recommendation and preserves an explicit
  user choice to work in the current checkout. The code prompt remains clear
  that it must not silently make another worktree itself.

## 5. Acceptance Criteria

- A worker with a parent and a completed task records exactly one redacted
  return. Its parent reads the worker identity, branch, worktree, status and
  final response; a parentless worker records none.
- A waiting approval records no return; resolving it to completion records one.
  Replaying, restarting and a second consume do not duplicate or re-run it.
- Consuming a return starts one ordinary parent task with a delimited result;
  it does not merge, apply, or approve anything. A second consume is refused.
- Source-level UI coverage proves the result card and its labels, plus the
  exact `worker-prompt` gating for **Start worker** and **Work here instead**.
  The latter sends the explicit inline-choice instruction through
  `sendChatPrompt`; a worker session and an untagged block have neither action.
- Prompt tests prove the worktree paragraph permits an explicit inline choice
  while retaining the rule against silently creating a worktree.

## 6. Verification

Local checks passed on 2026-10-31:

- `scripts/quality-gate.sh fmt node lint typos` and `cargo deny check`;
- `cargo clippy -p workspace-engine -p desktop-shell --all-targets --locked -- -D warnings`;
- focused `workspace-engine` tests for the return event, the redaction and
  idempotency, and the amended worktree paragraph;
- focused `desktop-shell` tests for the worker driver's return, the waiting
  decision, the consume route and its refusal, and the source-level UI actions;
- `npm run specs:check -- --strict`.

The workspace-wide clippy and test suite remain CI-only under `AGENTS.md`.

### 6.1 What landed

- `SessionStore` gained `WorkerReturn`, `record_worker_return` (idempotent per
  worker task and scanned before persistence),
  `pending_worker_returns` (only unconsumed records, read with `parsed_events`
  so a parent rewind cannot resurrect one), and
  `consume_worker_return_and_create_task`, whose one `worker_return_consumed`
  event carries the complete created task. There is no standalone consume
  method that could write the event without that task. Session appends use a
  per-session `flock` sidecar that reserves sequence numbers and holds the
  authoritative counter, so a normal append does not scan the log; the readers
  of task state accept the consumed event as a task-creation fact.
- The worker `TurnDriver` writes the return after a terminal turn when its
  record names a parent, and also when a turn returns `Err` after the engine
  persisted a terminal task. A failure to write it is recorded beside the
  terminal worker status and does not change the task's own outcome.
- `/api/session` includes `workerReturns`, and
  `/api/continue-worker-result-stream` validates the parent session and
  repository, resolves the model, then consumes one return and creates its
  parent task in the same event; the bounded delimited user message is the
  task's prompt, and the already-created task runs the ordinary parent turn. A
  model or policy failure leaves the return consumed and that one task failed.
  A second consume and a restart are refused without another model call or
  task.
- `app.js` renders a **Worker result** card with **Continue with result** and a
  worker link, and adds **Work here instead** beside **Start worker**. Both
  keep the existing exact-tag, assistant-only, non-worker gating.
- `WORKTREE_TASK_PARAGRAPH` now recommends a worker and preserves an explicit
  choice to work in the current checkout. Spec 76 §4 records the amendment.
