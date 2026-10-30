# Feature Spec: A Worker Verifies After Its Patch

Status: Done 2026-10-30 (specified and built the same day; local checks in §7,
the full gate is left to CI).
Order: 80 of 80 (#79, the context budget work, is reserved and in flight)
Plan: none. A defect reported in use, like #73–#78: an exception to the rule
in [`README.md`](README.md).
Depends on: [#23](23_verification_loop/proposal.md),
[#57](57_worker_sessions/proposal.md) and
[#77](77_continue_the_task_after_verification.md), all built.

## 1. Motivation

On 2026-10-30 a worker session was handed the brace-glob task. It proposed a
patch to `ignore.rs` and `navigation.rs`, and the user applied it at 20:11:47.
The session recorded `verification_due`, and then nothing happened: no
verification, no continuation, and the worker reported `idle`.

In the user's own sessions, the patch card's `onResolved` starts
verification, and since #77 the task then continues. The worker's patch card
(`renderWorkerApproval` in `app.js`) was drawn without that hook. The card
also stayed visible, because nothing marked the patch settled, so a second
**Apply** reached the hash check and was refused with "Target file changed
after patch generation". No harm was done, but the worker never moved on.

#57 §5.6 had left this open on purpose: "Continuing a turn after a patch is
applied … belongs in the engine for both hosts". #77 put it in the engine.
This spec connects the worker to it.

## 2. Requirements

1. The worker accepts `verify` while idle. It runs
   `ChatOrchestrator::verify_due` for its session in its worktree, in its own
   process, so the #77 continuation runs there too. The worker holds the
   session and the model key, and the shell serves one request at a time, so
   running this in the shell would block every other session.
2. A verification that pauses on a check card leaves the worker
   `waiting_for_approval` with approval kind `checks`. The card is answered
   with `checks`, which carries the card's id and one decision per check, in
   the same JSON the shell's check route takes. `approve` and `reject`
   refuse a check card.
3. The worker's patch card sends `verify` once every file is settled, just as
   a shell session's card starts verification.
4. The new job's `running` status clears the worker's approval, so a settled
   patch card is not drawn again.
5. A worker's check card offers no **Always allow**, matching a worker's
   command card (#57).
6. `verify` with nothing due is answered with the engine's own refusal, which
   is recorded as the job's error. The worker stays idle.

## 3. Non-goals

- Running a worker's verification in the shell process.
- Changing what verification or the continuation do. They are #23's, #77's
  and #78's.
- Recovering the run that found this. Its verification is still due, and the
  user can ask the worker to continue with a message.

## 4. Design

- **`worker.rs`:**
  - `ControlCommand` gains `Verify` and `Checks { proposal_id, decisions }`,
    parsed from `verify` and `checks` (`proposalId` and `decisions`
    required).
  - `TurnJob` gains `Verify` and `Checks`, and `take_turn` calls `verify_due`
    and `resume_after_check_decision`.
  - `DecisionKind::Checks` is set by `pending_decision` when the result
    carries a check batch.
  - `approval_payload` emits kind `checks` with `check_batch_json`.
  - `decide` refuses a pending check card. `decide_checks` validates the
    pending id, then parses the decisions with the shell's
    `parse_check_decisions`.
- **`workers.rs`:** `control_route` forwards `verify` with no arguments, and
  `checks` with `proposal_id` and `decisions`.
- **`app.js`:**
  - `renderWorkerApproval` draws a `checks` approval with
    `createCheckBatchCard(…, { decide })`, which hides Always allow and posts
    the decisions to `/api/worker-control`.
  - The worker's patch card gets `onResolved: () => verifyWorker(view)`.
  - `workerApprovalToShow` treats `checks` like a command, shown only while
    the live process waits.

## 5. Acceptance Criteria

- `verify_runs_the_due_verification_in_the_worker` covers the whole cycle:
  - `verify` with nothing due leaves the worker idle, with "No verification is
    due" recorded as the job's error;
  - after the shell applies the worker's patch, `verify` runs the
    verification in the worker, and its `Finished` event is in the session
    log;
  - each job, `verify` included, starts with a `running` status.
- `a_check_card_is_answered_with_checks_not_approve`: `checks` with no card
  pending is refused.
- The command table accepts `verify` and `checks`, and refuses `verify` with
  an argument and `checks` without `decisions`.

## 6. Implementation Notes

The UI has no JavaScript test harness. The card wiring is two call sites and
two small functions, `verifyWorker` and `sendWorkerChecks`, which follow
`decideWorkerApproval`.

## 7. Verification

The `desktop-shell` suite passed locally (230 tests). The fast checks and
scoped clippy are in the change's hand-back. The workspace clippy and test
suite are left to CI.
