# Feature Spec: Show When Damaian Is Waiting for You or Still Working

Status: In progress (started 2026-11-10; Tasks 1–2 of 6 done; see [tasks.md](tasks.md))
Order: 107 of 108
Plan: none. Written from the 2026-11-09/10 dogfooding sessions in which the
owner ran Damaian on spec 33 (planning, then Task 1). The decisions and the
code survey are in [context.md](context.md).
Depends on: [#93](../93_recovery_cards_follow_the_conversation.md) (recovery
cards below the conversation), [#102](../102_repair_provenance_and_stagnation.md)
(pre-apply baselines), [#103](../103_bounded_worker_continuation.md) (segment
continuation) and [#104](../104_worker_questions/proposal.md) (worker
questions) — all built; **nothing here is unbuilt, so this spec is ready to
start**.

## 1. Motivation

Once the engine stopped stalling (specs 101–106 and the 2026-11-10
turn-window fix), the owner's remaining problem was that he could not tell
what Damaian was doing:

- **A decision he had to make was out of sight.** A model question, a patch
  card and a check card render inside the assistant's message, above the plan
  card. The plan card, a Git status card and other panels sit between the card
  and the composer. The run status line disappears, the composer's Stop button
  turns back into Send, and the plan still marks its step **RUNNING**. Twice
  he had to scroll up to find what was needed, and once he read a waiting turn
  as stopped.
- **Work in progress looked like nothing.** The pre-apply baseline (`cargo
  test`, several minutes on the development Mac) runs with no progress and no
  cancel; he read it as "looks like it stopped".
- **An ordinary session interrupts every 16 rounds.** "Implement Task 1" in a
  normal Code-mode session paused on **Answer now / Continue — 16 more** at
  rounds 16, 32 and 48. Spec 103 continues automatically only for a worker
  with an approved budget.

The owner's priority statement: if a user cannot use Damaian, its features do
not matter.

## 2. Requirements

1. **One place for what needs you.** A decision the turn is waiting on — model
   question, patch review, check approval, pre-apply baseline, command
   approval, round-limit offer, plan review, provider fallback — renders at the
   end of the conversation, directly above the composer, following spec 93's
   rule for recovery cards. The assistant's message keeps its text and a
   one-line pointer; it no longer holds the card.
2. **The composer says so.** While a decision is pending, the composer area
   shows one line naming it ("Damaian needs your answer", "Review the patch",
   "Approve 2 checks", …), with a control that scrolls to and focuses the card.
   While a question is pending, the main composer does not compete with the
   card's own answer field (context D2).
3. **The status line never goes blank while a turn is alive.** It shows the
   waiting state ("Waiting for your answer", "Waiting for your review",
   "Running baseline: cargo test (repository root) · 1m 12s") instead of being
   removed when the engine reports `Pending`. While the turn works, it names the
   current activity from the last action ("Reading config.rs 1200–1600",
   "Running cargo test --test permission_profiles · 40s", "Waiting for the
   model · 12s"), and shows the current step's elapsed time separately from the
   turn's total (added 2026-11-10 at the owner's request: "Thinking · … 705s"
   read as one 12-minute call when it was the whole turn).
4. **The plan step tells the truth.** A step whose task is waiting on a
   decision shows **waiting for you**, not **running**.
5. **Baselines show progress and can be cancelled.** A pre-apply baseline
   streams which check is running and for how long, and offers **Cancel**,
   which records the baseline as declined (the patch is then applied or
   rejected as the user chooses, exactly as after **Skip baselines**).
6. **An ordinary Code-mode session does not ask to continue every 16 rounds
   while it is making progress** (context D3). It continues under the same
   progress and ceiling rules as spec 103's segments and still stops — with
   the existing card — when it stops making progress or reaches its ceiling.
   While it continues, a patch's verification is a **per-change** run (the
   owning root only), and the full completion floor runs once, when the task
   ends (added 2026-11-10: a planless task treated every patch as its end and
   ran root and crate `cargo test`, about 4 minutes, after each one —
   observation 83).

## 3. Non-goals

- Changing what needs approval. Every decision still waits for the user.
- The other UI findings of the same sessions (Protected paths panel, archive,
  Run in a worker, Git status card, model record) — spec 108.
- A redesign of the plan panel beyond the waiting label.

## 4. Design

See [context.md](context.md) for the code survey and decisions D1–D5. In
short: one `pendingDecision` presentation state from the engine drives the
card's placement, the composer line, the status line and the plan label; the
baseline route becomes a streamed request with a real cancel token; and
ordinary Code-mode turns reuse spec 103's segment decision with a default
budget.

## 5. Acceptance criteria

- A live turn that pauses on each decision kind in requirement 1 shows its card
  as the last element above the composer, and the composer line names it
  (desktop-shell UI test per kind, plus a manual check recorded in tasks.md).
- The status line is present, with a waiting label, for every pause kind; it
  is removed only when the turn has ended.
- The plan step of a waiting task reads **waiting for you**, live and after a
  reload.
- A baseline run reports progress events and can be cancelled; the
  cancellation is recorded as a declined baseline and leaves the patch
  undecided.
- A scripted ordinary Code-mode session of 40 progressing rounds pauses zero
  times; one that stops making progress pauses with the existing card.
- `USER_GUIDE.md` and `TROUBLESHOOTING.md` describe the new waiting states.
