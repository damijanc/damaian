# Feature Spec: Run a Plan in a Worker

Status: Done 2026-10-31 (specified and built the same day; local checks in §6,
the full gate is left to CI).
Order: 94 of 94
Plan: none. A gap reported in use, like #73–#93: an exception to the rule in
[`README.md`](README.md).
Depends on: [#21](21_task_plan_progress_and_budget/proposal.md) (plan review)
and [#57](57_worker_sessions/proposal.md) (workers), both built.

## 1. Motivation

On 2026-10-31 a spec-sized task ran in the user's main checkout instead of in
a worker. The model edited files there and the commit landed on `main`. #57
offers a worker only through the model: a paragraph in the Code prompt asks
it to write a `worker-prompt` block when work suits a separate session. The
model did so once and then stopped, so whether the user was offered
isolation depended on the model.

A plan put up for review (#21 §5.5) is the point where a task shows its size,
before anything has changed. That is where Damaian itself should offer the
worker.

## 2. Requirements

1. With Worker sessions on, a plan review card in a session that is not a
   worker shows **Run in a worker**, next to **Approve** and **Decline**.
2. Pressing it opens the New worker dialog (#57 §5.1), filled with:
   - the request that set the work out: the latest prompt the user typed
     that says something, so not "continue", and not one of Damaian's own
     turns;
   - the plan's open steps, as the card has them, edits included;
   - an instruction to propose them as the worker's plan and work through
     them. It says the plan was drafted in another session and that the user
     has not approved it yet.
3. The dialog is the user's click (#57 §5.1). Cancelling it leaves the card
   waiting, and nothing is recorded.
4. Once the worker has started:
   - the paused turn ends without calling the model;
   - its open steps are marked skipped, as for a declined plan;
   - the turn records "Handed to a worker…" and becomes `complete`;
   - the card shows a link to the worker;
   - the hand-over is audited as `plan_handed_to_worker`, with the worker's
     session id.
5. The pause is answered once. A second hand-over, approve or decline is
   refused.
6. Handing over is not an approval. A hand-over is a new start: the worker
   puts its plan up for review before its first change (#21 §5.5), and that
   review is the one that counts. Counting the click as an approval would
   let a plan nobody read skip its review; it is pressed to move the work,
   not to accept the steps. The hand-over note in this session says so, so
   the worker's review card is not a surprise.
7. The model's own `worker-prompt` handoff is unchanged.

## 3. Non-goals

- Offering a worker before any plan exists. A turn with no plan, or one that
  only reads, has no review card (#21 §5.5).
- Moving work that has already started. The card exists only before the
  first mutating step.

## 4. Design

- **`chat.rs`:**
  - `plan_worker_prompt(proposal_id, revised)` loads the pause without taking
    it and builds the prompt. It applies the card's revision with the same
    `apply_plan_revision` that approval uses.
  - `hand_plan_to_worker(proposal_id, worker_session_id, by)` takes the
    pause, refuses anything but a plan review, skips the open steps, audits,
    appends the note, clears the pending approval and completes the task.
- **`desktop-shell`:** two routes, `POST /api/plan-worker-prompt` and
  `POST /api/plan-to-worker`.
- **`app.js`:** `createPlanReview` adds the button when `workersEnabled` and
  the session is not a worker. It fetches the prompt, opens
  `showNewWorkerDialog` with the session as parent, and calls
  `/api/plan-to-worker` only with the worker the dialog started.

## 5. Acceptance

- `a_plan_under_review_is_handed_to_a_worker` (`tests/plan_turn.rs`):
  - the prompt starts with the request and lists both steps, and says the
    user has not approved the plan yet (requirement 6);
  - a revision that drops a step and renames the other is what the prompt
    carries;
  - after the hand-over the task is `complete`, every step is skipped,
    nothing was written, the note says the worker asks for approval, and a
    second hand-over is refused.
- The button and dialog flow were checked by reading. The web UI has no
  automated tests for it.

## 6. Verification

Local checks, 2026-10-31:
- `scripts/quality-gate.sh fmt node lint typos deny`;
- `cargo clippy -p workspace-engine -p desktop-shell --all-targets --locked -- -D warnings`;
- `cargo nextest run -p workspace-engine --locked` and
  `cargo nextest run -p desktop-shell --locked`.

The workspace clippy and test suite are left to CI.
