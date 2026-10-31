# Feature Spec: Worker Task Outcomes

Status: Done 2026-10-31 (specified and built the same day; local checks in §6,
the full gate is left to CI).
Order: 99 of 99
Plan: none. A usability defect reported in use, like #73–#98.
Depends on: [#17](17_durable_task_state_and_crash_recovery/proposal.md) (task
status), [#57](57_worker_sessions/proposal.md) (worker progress and controls),
and [#81](81_thinking_phase_while_the_model_reasons.md) (live phases), all
built.

## 1. Motivation

The worker session showed a transient **Thinking** row while a model streamed
reasoning, then removed it when the turn returned to the worker host. The
remaining control strip reduced every settled state to **Idle**. `idle` is a
process lifecycle fact: it can mean a completed task, a patch awaiting review,
a cancelled or failed task, a budget limit, or a turn that failed before
recording an outcome. It does not tell the user whether work is complete,
whether input is needed, or what to do next.

The durable worker progress record already has the facts: `taskStatus`,
approval kind, errors, phase events, and timestamps. This feature makes the
worker view and board present those facts as a task outcome rather than asking
the user to interpret the process state.

## 2. Requirements

1. A worker's own session always presents a task-level status with an explicit
   next action. It must distinguish working, paused, needs a decision,
   complete, stopped/cancelled, failed, interrupted, unknown external outcome,
   and tool/token budget exhaustion. It must never present bare **Idle** as the
   user-facing conclusion.
2. A task holding a patch or provider-fallback proposal is **Needs your
   decision** even though its worker process is idle. Its review card remains
   the action; the composer cannot start a second task around that decision.
3. A completed task says that it is complete and changes the composer to **Send
   follow-up**. An unfinished idle task says why it stopped and uses **Continue
   task** with an instruction appropriate to the recorded outcome.
4. While running, the status says whether worker progress is current. After 30
   seconds without a progress event it says how long there has been no update
   and that the worker may still be waiting for the model. It does not claim to
   have diagnosed a stuck provider from the absence of output.
5. The Workers board groups an idle `waiting_for_approval` task under **Needs
   you**, labels it **Needs your decision**, and exposes **Review**. Recovery
   statuses that are neither active nor terminal must not be misreported as
   running.
6. Outcomes are reconstructed from the append-only progress record after a
   reload. No model text, raw reasoning, or secret is copied into the outcome.

## 3. Non-goals

- A watchdog that kills a model call merely because it has not emitted output.
  A live worker can legitimately wait on a provider; the UI reports the fact
  and leaves Pause/Stop with the user.
- A model-written completion summary separate from its ordinary final response.
  The session transcript remains the detail; this feature supplies the
  durable state and next action.
- Changing approval, cancellation, recovery, or worker lifetime semantics.

## 4. Design

- `app.js` keeps `taskStatus`, the most recent progress timestamp, and whether
  the worker has started a turn in its worker view. `workerViewOutcome` derives
  the visible label, detail, dot and available action from those durable facts.
  The strip keeps this result after the live Thinking indicator is removed.
- The 30-second stale threshold is presentation only. It compares the browser
  clock with `timestampMs` from the worker progress event and says “No worker
  progress …”; it never calls the worker dead or stuck.
- `workerComposerMode` refuses a decision-pending idle worker, names completed
  work as a follow-up, and names an unfinished settled task as continuation.
- `workers.rs::worker_row` classifies an idle task with
  `waiting_for_approval` as **Needs you**. It uses `TaskStatus::is_active()`
  rather than treating every non-terminal recovery status as running.

## 5. Acceptance Criteria

- A worker session source-level UI test covers explicit completed, decision,
  unknown-outcome, stale-progress, and follow-up labels.
- A board route test writes an idle `waiting_for_approval` progress status and
  proves that the row is **needs_you**, describes the decision, and can be
  reviewed.
- The existing worker route tests continue to prove a normally completed
  worker is `idle` at the host level; the new UI maps that fact to **Task
  complete**, not a false process state.

## 6. Verification

Local checks passed on 2026-10-31:

- `scripts/quality-gate.sh fmt node lint typos` and `cargo deny check`;
- `cargo clippy -p desktop-shell --all-targets --locked -- -D warnings`;
- `cargo nextest run -p desktop-shell --locked` (237 passed, 18 skipped);
- `npm run specs:check -- --strict`.

The workspace-wide clippy and test suite remain CI-only under `AGENTS.md`.
