# Feature Spec: Typed Turns Pick Up the Open Plan

Status: Done 2026-10-31 (specified and built the same day; local checks in §7,
the full gate is left to CI).
Order: 83 of 83
Plan: none. A defect reported in use, like #73–#81: an exception to the rule
in [`README.md`](README.md).
Depends on: [#21](21_task_plan_progress_and_budget/proposal.md) (task plan),
[#66](66_plan_left_open_at_turn_end/proposal.md) (plan left open at turn
end) and [#77](77_continue_the_task_after_verification.md) (continue after
verification), all built.

## 1. Motivation

In worker session `session_18e36455d1057170_17822_84` the first task proposed a
seven-step plan and ended with its steps open: it answered after #66's
reminder. The user typed "continue" several times. Each message started a new
task with no plan. A later patch was applied and verified, and the
verification turn carried the plan of the task that proposed the patch, which
had none. So #77 found no open steps of the task's own and ended with the
report. The seven steps were never taken up again.

The cause is in `carry_plan_from_the_previous_turn` (`chat.rs`). A task is one
turn, so a plan lives on the task that proposed it, and a new task carries it
only when the previous task handed its work over: a token-ceiling stop, a
tool-round stop or a crash-recovery supersession (#21 §5.4, #45, #61). A turn
that ended normally with steps open, which #66 made an ordinary outcome, hands
nothing over. Its comment called this narrow on purpose: carrying a plan into
an unrelated next question would put steps on the panel the user never asked
for.

## 2. Requirements

1. A turn the user types in Code mode picks up the session's latest plan when
   that plan still has an open step that is not a verification `verify_` step.
   "Latest" is the plan of the most recent task in the active log that has
   one, not only the previous task's.
2. It does not when:
   - every step is settled (completed, blocked or skipped, including a plan
     declined at review, #66 D5);
   - the only open steps are `verify_` steps, which belong to #23's loop;
   - the session is in Ask, Plan or Review mode;
   - the plan's task ended `interrupted` or `unknown_external_outcome`, which
     crash recovery decides (#45);
   - a rewind removed the plan (the active log no longer holds it);
   - the turn that last carried the plan left it untouched (§5.2).
3. The existing hand-over carry (token stop, tool-round stop, supersession) is
   unchanged and still takes precedence.
4. A carried plan the user approved stays approved. Picking it up does not put
   it up for review again. A carried plan that was never approved is reviewed
   before its first change, as any plan is.
5. A turn that carried a plan and has not yet started on it may propose a new
   plan in its place, for when the user has moved on. The new plan is reviewed
   like any other, and the old plan stays recorded under the task that made
   it.
6. Because the patch's task now holds the plan, the verification after it
   carries the plan, and #77 continues its open steps.

## 3. Non-goals

- Deciding from the user's words whether a message continues the work.
  "continue", "go on" and "now the docs" are all continuations, and a list of
  phrases would be wrong in both directions. The model, which reads the
  request, decides by keeping or replacing the plan (requirement 5).
- Carrying anything else across turns. Turn notes already carry (#65).
- A user control for dropping a plan. Replacing it, or one untouched turn,
  already ends the carry.
- Changing what a verification turn or #77's continuation carries.

## 4. Current State (before this spec)

- `carry_plan_from_the_previous_turn` looked only at the task immediately
  before the new one and returned early unless that task was handed over.
- `ProposePlan` refused any second plan in a turn, including a carried one:
  "This turn already has a plan."
- `read_plan_approved` answered for the task's own `plan_approved` event
  only. So a carried plan, including #77's continuation's, was put up for
  review again before its first change.

## 5. Design

### 5.1 The carry

`carry_plan_from_the_previous_turn` keeps the hand-over rule. When the previous
task was not handed over, it calls the new `carry_the_open_plan`:

1. Return unless `session_mode` is Code.
2. Read every plan in one pass (`read_session_plans`). Walk the tasks newest
   first, skipping the new one, to the first that has a plan.
3. Return if that task's status is `Interrupted` or `UnknownExternalOutcome`.
4. Return unless some open step's id does not start with `verify_`.
5. Return if `carried_plan_untouched` holds for that task (§5.2).
6. Otherwise `resume_plan(task, source)`: the same `plan_resumed` event the
   other carries write, with `resumedFrom` naming the source task.

### 5.2 One untouched turn ends the carry

`SessionStore::carried_plan_untouched(session, task)` is true when the task's
last plan event is `plan_resumed`, with no `plan_created`, `plan_revised` or
`plan_step_updated` for that task after it. In other words, the plan was
carried in and nothing in that turn moved it: no step closed or started, no
evidence was appended, and the turn proposed no plan of its own.

A plan whose carrying turn left it like that is not carried again. An unrelated
question after an open plan therefore costs one turn with the plan on the
panel, not every turn after it.

The cost is the other direction. A turn that reads files for an unrelated
question has those reads attached to the open step at turn end (#66 §5.3).
That counts as touched, so the plan carries one more turn. The model can still
replace it there.

### 5.3 Approval follows the plan

`read_plan_approved` folds the active log into:
- the set of tasks with `plan_approved`;
- for each task whose plan arrived by `plan_resumed`, the task it came from.

A task is approved when it, or any task its plan was carried from, has the
event. `plan_created` for a task clears both, because it is a plan the task
proposed itself. `plan_revised` does not clear them: a revision is written
just before the approval it belongs to (#21 §5.5).

This also changes #77's continuation, whose plan is carried into the
verification task. That plan no longer goes back to review before its next
patch when the user approved it once.

### 5.4 Replacing a carried plan

In `run_agentic_turn`, `ToolAction::ProposePlan` accepts a new plan when the
turn's plan satisfies `carried_plan_untouched`. It writes `plan_created` for
the turn's task, which `read_task_plan` takes over the earlier `plan_resumed`,
and it sets the loop's `plan_approved` to `false`, since the approval was for
the old steps. In every other case the refusal stands. Once a step of the
carried plan has closed or gathered evidence, that evidence is tied to it,
the same reason #21 §5.5 rules out mid-execution edits.

The `propose_plan` tool description gains one sentence: if the context shows a
plan carried forward from an earlier turn and the user has moved on, propose a
new plan before starting on the old one, and the earlier plan stays recorded.
It goes in the description rather than in the `plan_state` context item,
because verification turns and #77's continuation read the same item and
cannot replace their plan.

### 5.5 Documentation

`USER_GUIDE.md` → The Plan Panel gains "Picking an open plan up again".
`TROUBLESHOOTING.md`'s `plan_resumed` row and its paragraph on confirming a
carry name the new case.

## 6. Acceptance Criteria

All in `crates/workspace-engine/tests/plan_carry_over.rs`:

| Criterion | Test |
| --- | --- |
| A typed turn picks up an open plan and works on it; the model is told it may replace it | `a_typed_turn_picks_up_the_open_plan_and_works_on_it` |
| The latest plan is found past a turn that carried none; Ask mode carries nothing | `the_latest_plan_is_found_past_a_turn_that_carried_none` |
| A finished plan is not picked up | `a_finished_plan_is_not_picked_up` |
| Only `verify_` steps open: not picked up | `a_plan_whose_only_open_steps_are_verification_steps_is_not_picked_up` |
| One untouched turn ends the carry | `a_plan_a_turn_carried_and_left_untouched_is_not_carried_again` |
| A carried plan can be replaced before it moves, and the old one stays readable | `a_carried_plan_can_be_replaced_before_it_moves` |
| Once it moved, the refusal stands | `a_carried_plan_that_moved_cannot_be_replaced` |
| An approved plan is not reviewed again when picked up | `an_approved_plan_stays_approved_when_a_typed_turn_picks_it_up` |
| A replacement plan is reviewed | `a_replaced_plan_does_not_inherit_the_approval` |
| The reported session: plan left open, typed "continue", patch, apply, verify, and #77 continues with the plan's own steps | `verification_after_a_typed_turn_continues_the_open_plan` |

## 7. Implementation Notes and Verification

Falsified against the old code: with the `src/` changes reverted, the five
carry tests fail. The other five are guards that pass when nothing is carried,
so each rule they pin was falsified on its own:
- not following `resumedFrom` fails the approval test;
- refusing every replacement fails both replacement tests;
- keeping the approval on replacement fails `a_replaced_plan_…`.

Two tests in `token_ceiling.rs` pinned the narrow rule this spec replaces.
Their first turn completed with its plan's steps open:
- `a_turn_after_a_completed_task_starts_with_no_plan` now finishes its plan
  first, so it still checks that a finished plan is not carried;
- `a_turn_after_an_unsuperseded_task_starts_with_no_plan` became
  `a_turn_after_an_untouched_carry_starts_with_no_plan`: the second turn picks
  the plan up, and the third, after an untouched carry, starts clean.

The other carry tests (`round_budget.rs`, `plan.rs`, the crash case in
`token_ceiling.rs`), #66's `plan_turn.rs` and #77's `verification_loop.rs`
pass unchanged.

All of these passed locally:
- the fast checks (`scripts/quality-gate.sh fmt node lint typos deny`);
- scoped clippy for `workspace-engine`;
- the `workspace-engine` and `eval-harness` suites (2,243 tests).

The loopback-stub and watcher tests among them were run outside the agent
sandbox, which forbids binding a port. The workspace clippy and test suite
are left to CI.
