# Feature Spec: Fast Verification

Status: In progress (started 2026-11-11; Tasks 1–7 in [tasks.md](tasks.md))
Order: 109 of 109
Plan: none. Written from the owner's spec 33 Task 1 run on Damaian v0.69.0
(2026-11-11); the code survey and decisions are in [context.md](context.md).
Depends on: [#102](../102_repair_provenance_and_stagnation.md) (baselines and
provenance), [#105](../105_structured_plan_checks/proposal.md) (structured plan
checks), [#107](../107_visible_waiting_and_working/proposal.md) (per-change
verification for continuing turns, baseline progress) — all built or in
progress with the parts this spec changes already on main; **nothing here is
unbuilt, so this spec is ready to start**.

## 1. Motivation

Damaian's own repository is large enough to show the cost of verification as it
works today. For one 51-line patch to `config.rs` in spec 33 Task 1 the owner
waited through:

1. a pre-apply baseline: `cargo test` at the repository root (compiling every
   crate, including the desktop app) and again in `crates/workspace-engine`,
   several minutes, behind an approval card;
2. after applying, the crate's whole `cargo test` again — a few more minutes,
   behind another approval card — although the plan Damaian had just proposed
   carried a scoped required check (`cargo nextest run -p workspace-engine
   --test permission_profiles --test foundation`);
3. the same per-patch check after every later patch, and the full floor once
   more at the end.

The owner's verdict: "You wait several minutes for all the tests to run to get
a baseline and then a couple of minutes later Damaian wants to run it again.
A user will just close it and say it is broken."

Correctness does not need most of this. A baseline matters only when a check
fails afterwards; a scoped check the plan declared is the right per-change
signal; an approval for `cargo test` does not need asking again for the same
check in the same task; and a result for an unchanged tree is already known.

## 2. Requirements

1. **Baseline on failure, not before the patch.** A patch is applied without a
   pre-apply baseline. Only when a check run after it fails is a baseline taken,
   on the pre-patch tree, and only for that check. Provenance classification
   (spec 102) is unchanged in meaning: the same `CheckBaseline` values feed the
   same `classify`.
2. **Rerun only what failed.** Where the check's findings name individual tests
   (Rust test and nextest output today), the failure-time baseline reruns just
   those tests on the pre-patch tree. Other checks rerun whole.
3. **Scoped checks per change.** After a patch, the per-change verification
   uses the plan's scoped required check for the root the patch touched when the
   plan declares one; the detected root check remains the completion floor and
   runs once when the task ends (spec 101/105 R15–R16, spec 107 D3).
4. **One approval per check per task.** Approving a check that needs approval
   covers that exact check (command and directory) for the rest of the task
   chain, including continuations; it is never an "always allow", never covers
   another command or directory, and ends with the task.
5. **Reuse known results.** A check whose recorded run covered exactly the
   current tree (same file hashes and mutation generation) is not run again; the
   recorded result is used and shown as reused.
6. **Visible while it runs.** Every check or baseline that runs shows a clear
   running state on its card, in the turn's status line and in the composer line
   (OBSERVATIONS 89): "Running cargo test (crates/workspace-engine) · 1m 12s",
   with **Cancel**.

7. **The user can stop anything that is running** (decided by the owner,
   2026-11-11). Every running action has a visible **Stop**: a model call, a
   command, a check (per-change or completion), a baseline, a hook, an MCP call,
   a browser diagnostic. Stopping is always allowed, even when the action has
   side effects and stopping it mid-way may leave its outcome unknown: the
   action's process group is ended, its outcome is recorded as **unknown** with
   the existing side-effecting marker (spec 17), nothing is retried
   automatically, and the turn ends at that point with a report saying what was
   stopped. Evidence: on 2026-11-11 the owner could not stop a post-apply
   `cargo test` of the whole crate (run for a one-line import patch) because
   only baselines had **Cancel**.

## 3. Non-goals

- Changing what the completion gate requires or how provenance is classified.
- Running checks without the command policy's approval where the policy asks.
- Test-impact analysis beyond the plan's declared scoped checks.
- Non-Rust targeted reruns (they rerun the whole check; a later spec may add
  parsers).

## 4. Acceptance criteria

- A planned task with a scoped required check applies its first patch with no
  baseline card; the per-change check is the scoped one; a passing run raises
  no baseline at all (engine test and a deterministic eval scenario).
- A failing per-change check triggers a baseline of only that check on the
  pre-patch tree; for a Rust test failure only the failing tests rerun; the
  provenance matches what the old pre-apply baseline would have produced for the
  same fixtures (pre-existing, introduced, worsened, unchanged).
- A second run of the same check in the same task asks no approval; a different
  command or directory still asks; a new task asks again.
- An unchanged tree reuses the recorded result.
- On Damaian's own repository, the spec 33 Task 1 shape (one small engine patch)
  costs one scoped check run per patch and one floor run at the end — measured
  and recorded in tasks.md against the 2026-11-11 run.
- `USER_GUIDE.md` and `TROUBLESHOOTING.md` describe the new flow.
