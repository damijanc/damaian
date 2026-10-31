# Feature Spec: Continue the Task After Verification

Status: Done 2026-10-30 (specified and built the same day; local checks in §7,
the full gate is left to CI).
Order: 77 of 77
Plan: none. A defect reported in use, like #73–#76: an exception to the rule
in [`README.md`](README.md).
Depends on: [#21](21_task_plan_progress_and_budget/proposal.md) (task plans),
[#23](23_verification_loop/proposal.md) (verification loop) and
[#66](66_plan_left_open_at_turn_end/proposal.md) (open-plan reminder), all
built.

## 1. Motivation

On 2026-10-30 Damaian was given an implementation task and made a nine-step
plan. Its first patch was a single `edit_file`. The user applied it, and #23's
verification turn proposed two checks, which were declined. The verification
then ended the task with its report: "1 of 9 plan steps finished". Nothing
went back to the plan. To the user, the session looked stopped.

Three things combine to cause this:

- A patch ends the turn that proposed it (#03), because the user has to
  review it.
- #23 §5.1 then starts a continuation turn that carries the plan, but its only
  jobs are to verify, repair and report.
- Nothing after the report resumes the task.

A task that needs more than one patch therefore stops after the first, every
time.

## 2. Requirements

1. When a verification ends without failing (outcome `passed`,
   `nothing_verified` since [#78](78_skipped_checks_are_not_passed.md),
   `no_checks_available` or `only_documentation_changed`) and the carried plan
   has steps that are not settled and are not the verification's own
   `verify_*` steps, the same turn continues the task. The model gets:
   - a fixed instruction to continue from the plan;
   - the user's latest request in the session;
   - the verification's summary line;
   - the open steps by title and id.
2. The continuation is an ordinary agentic turn with the same tools, round
   budget, pauses and #66 open-plan reminder. Its next patch ends it as any
   patch does, and the verification after that patch comes back here. The
   cycle ends when the plan has no open steps.
3. The report is still appended to the conversation before the continuation,
   and the turn's result keeps it, so the report card is still drawn.
4. A failed verification, a repair-limit stop and `no_repair_applied` are not
   continued: #23's repair and report own them. A change made without a plan,
   such as the one-shot patch flow, ends with the report as before.
5. Both ways a verification can end are covered: a direct `verify_due`, and
   resuming after the check card.

## 3. Non-goals

- Continuing after a failed verification automatically. The repair loop
  already asks for a fix, and continuing past a failure would build on a
  broken change. Since
  [#96](96_continue_repair_at_the_limit.md), the repair limit's report offers a
  **Continue repair** action; that is the user's explicit decision to start
  exactly one more repair attempt, not an automatic continuation.
- Treating a verification with every check skipped as anything other than
  what #23 calls it. That a skipped-only run was recorded `passed` was a
  separate defect, fixed by [#78](78_skipped_checks_are_not_passed.md). The continuation's instruction says only that verification
  "finished", and the summary line it quotes says what actually ran.
- Asking the user before continuing. Applying a patch that a task proposed is
  already the go-ahead for that task. Every later patch and command still
  goes through its own approval.

## 4. Design

`ChatOrchestrator::verify_due` and `resume_after_check_decision` produce the
report as before, in `verify_due_report` and `check_decision_report`. They then
pass the result to `continue_task_after_verification`. That method returns the
result unchanged unless:

- the result carries a report;
- the task is `complete`;
- the plan has open steps of its own.

Otherwise it builds the continuation the way `run_repair_turn` builds a repair:

- context assembly with the plan and the changed files;
- the session's conversation and notes;
- the system prompt with handoffs offered as for any non-worker session;
- the context-review pause when it is on;
- `run_agentic_turn` with default options.

The returned result's `verification_report` is set to the report.

The instruction is `CONTINUE_AFTER_VERIFICATION_PROMPT`. The user's request is
the latest task prompt in the session that is not Damaian's own, i.e. one that
does not start with the verification prompt.

## 5. Acceptance Criteria

- An applied patch whose task has two open steps passes verification. The model
  is then called with the instruction, both open steps and not the finished
  one. The turn's response is the model's answer, the result still carries the
  `passed` report, and the report message is in the session.
- The same patch with no open steps makes no model call and ends with the
  report.
- A failing check with the repair limit at 0 makes no model call and ends
  `failed`.

## 6. Implementation Notes

The tests are at the end of `workspace-engine/tests/verification_loop.rs`. In
the first test, the #66 reminder follows the continuation, because the mock's
answer leaves steps open. That is the intended combination, so the test checks
the first request.

The UI needed no change. `runVerificationStream` already passes the result to
`appendProposals`, so a continuation's patch, command, plan or question card
is drawn the same way a repair patch is.

## 7. Verification

The `workspace-engine`, `eval-harness` and `desktop-shell` suites passed
locally (2,454 tests). The fast checks and scoped clippy are in the change's
hand-back. The workspace clippy and test suite are left to CI.
