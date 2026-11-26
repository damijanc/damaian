# Feature Spec: A Verification Verdict You Can Read

Status: In progress (specified 2026-11-25; started 2026-11-25; Tasks 1–2 of five done, see [tasks.md](tasks.md))
Order: 114 of 114
Plan: none. Written from OBSERVATIONS 105: the owner's run of 2026-11-16, in
which a finished task's verification read as broken three times in one
session. The evidence, the code survey and the decisions are in
[context.md](context.md).
Depends on: [#102](../102_repair_provenance_and_stagnation.md) (provenance
classes and the completion gate's accepted failures),
[#101](../101_closed_task_completion.md) (the completion gate, Task outcome
card and **Continue task**), [#109](../109_fast_verification/proposal.md)
(failure-time baselines on the task-start tree and narrowed Rust test
baselines), [#112](../112_status_in_one_place/proposal.md) (one vocabulary of
states) and [#113](../113_one_pending_decision/proposal.md) (choices asked
with `ask_user`). All are built, so this spec is ready to start.

## 1. Motivation

After a task's verification, the owner could not tell whether the task was
done. The report ran to several lines with "worsened failures", "unchanged
failure" and red "exit code 101" rows, and its badge read "failed". The real
story was different: four failures were order-dependent tests that pass when
run alone, the fifth also passed alone, and the required check passed when he
ran it himself. In his words: "If I am confused so will other users."

Three things make this happen today (see [context.md](context.md) for the
code):

- **No verdict.** The report card shows data: an outcome badge and up to
  fifteen expanded rows. It never says, in plain words, whether the task's
  changes broke anything and what to do next. It sits after the assistant's
  answer, so it also buries a question asked in that answer.
- **"Not yours" is not visible, and is sometimes wrong.** The provenance
  labels never say "not caused by this task". `unchanged_failure` means
  "failed before this task too", and nothing says so.
  - Since spec 109 Task 4, a failing Rust test's baseline reruns that test
    alone. An order-dependent test then passes, the failure reads as
    introduced by the task, and a repair starts.
- **`failed` explains nothing.** A failed task gets no Task outcome card, so
  its reasons are never shown. A check that never finished (rerun it) and a
  check that fails (investigate) read the same. Nothing offers a way on.

The owner also asked for a baseline run to look like one.

## 2. Requirements

1. **A verdict first.** Every verification report carries an engine-owned
   `verdict` (`kind`, `headline`, `detail`). The kinds, in precedence order:

   | Kind | Headline | When |
   |---|---|---|
   | `build_failed` | "The change does not build" | the change does not build |
   | `caused_by_task` | "This task's changes break N checks" | any check has introduced findings |
   | `cause_unknown` | "Checks fail; Damaian could not tell whether this task caused it" | environment, uncertain or unknown failures, and nothing introduced |
   | `not_caused_by_task` | "Checks fail, but not because of this task's changes" | only pre-existing, as-before or passes-alone failures |
   | `not_verified` | "Not verified: N checks did not run or finish" | nothing failed, but something is due, missing, skipped or unfinished |
   | `passed` | "Checks passed" | none of the above |

   The detail is one sentence saying what happens next or what the user can
   do. The text copy of the report starts with the headline. `summary()`, the
   model-facing line, is unchanged.
2. **Whose failure it is.** The provenance group labels and summaries say
   whose failure it is and compare against "before this task's changes" (D7).
   Each failed check in the report has one engine line saying what its
   baseline found, for example "Before this task's changes: failed the same
   way". Enum wire names do not change.
3. **A baseline looks like a baseline.**
   - Its progress phase carries a typed marker.
   - Its running and finished lines say what it is for: "Checking whether
     `cargo test` failed before this task's changes".
   - The report lists the baselines it compared against (`baselines`).
   - The UI marks the running row, a baseline's check card and the report's
     baseline rows with one **Baseline** tag. It keys on data, never on
     label text.
4. **Tests that pass alone are not the task's (D3).**
   - **The rerun:** when a narrowed baseline passes, the engine reruns the
     same narrowed command once on the current files. It does this only when
     that run needs no approval.
   - **A finding that passes there too** is not introduced, and is never
     handed to repair.
   - **A check whose every introduced finding passes alone** is classified
     `passes_alone`. It is not repaired, and the completion gate accepts it
     with an explanation naming the tests.
   - **The record:** the rerun writes `verification_isolated_rerun`.
5. **A failed task explains itself (D4).**
   - **The card:** a `failed` task in an ordinary session shows a Task
     outcome card. Its headline names the cause ("Task failed: a check fails"
     or "Task not verified: a check did not finish"). Its reasons are grouped
     under **Fails**, **Did not run or finish** and **Not caused by this
     task**.
   - **The action:** it offers **Continue task** through the existing route,
     which now accepts `failed` and still grants nothing.
   - **The exception:** at the repair limit, spec 96's **Continue repair**
     stays the one action, and the card has no button.
   - **New reason code:** `CheckOutcome::Environment` gets its own code,
     `required_check_could_not_run`.
6. **The report does not bury what follows (D6).** A live report card shows
   the verdict, coloured by kind with the existing state tokens, and one
   collapsed **Details** disclosure holding today's rows. The raw outcome
   badge goes. **Continue repair** stays visible.
7. **Rules that do not change.**
   - No new task status, and no change to what makes a task `complete`,
     `incomplete` or `failed`.
   - No change to command policy: the passes-alone rerun never adds a card.
   - No change to which introduced findings repair receives, except that
     findings which pass alone are removed.
   - The model-facing `summary()` is unchanged.

## 3. Non-goals

- Repeat-based flake detection (running a check N times), or flake handling
  for checks that are not narrowed Rust test baselines.
- A new task status or a migration of existing logs.
- Rendering a reloaded report as a card. It stays text, and now starts with
  the verdict.
- Changing when a baseline runs (spec 109 D1) or which tree it uses (D2).
- Worker sessions' cards (spec 57 and spec 99 own them). The engine fields
  reach them unchanged.

## 4. Acceptance criteria

- Engine tests: each verdict kind is produced by a fixture report, with
  precedence pinned. For example, a report with one `pre_existing` and one
  `new_regression` check gives `caused_by_task`. `render()` starts with the
  headline, and `summary()` is byte-identical to before for the existing
  fixtures.
- Engine test: the 2026-11-16 shape. A check fails with three tests in the
  full run, the narrowed baseline passes all three, and on the current tree
  two of the three pass alone. The result is `new_regression` with only the
  third introduced; the repair request names only it; a
  `verification_isolated_rerun` event records both lists. When all three pass
  alone, the check is `passes_alone`, no repair starts, and the gate lists it
  in `acceptedFailures`.
- Engine test: the isolated rerun is skipped, with no card, when its command
  would need approval, and classification is as before.
- Engine tests: `TaskControl` for a `failed` task has the cause headline, the
  grouped reasons, `acceptedFailures`, and **Continue task**, except at the
  repair limit. `continue_incomplete_task` continues a `failed` task with a
  matching report and refuses a stale one. An environment failure of a
  required check reads `required_check_could_not_run`.
- Desktop-shell tests (`app_js_…`): the report card draws `verdict` and a
  collapsed disclosure, and no longer prints `report.outcome` as its badge; a
  baseline's marker draws the Baseline tag; a failed task's control draws
  its card.
- In the running app (inspection server on its own port and data dir):
  - a seeded failed verification shows a two-line card with the verdict, and
    the answer above it stays visible;
  - the Task outcome card names the cause and continues the task;
  - a baseline's running row carries the Baseline tag.
- The deterministic eval tier passes. One new scenario covers a narrowed
  baseline whose tests pass alone.
- `USER_GUIDE.md` explains the verdict, the Baseline tag, "passes when run
  alone" and the failed task's card. `TROUBLESHOOTING.md` explains
  `verification_isolated_rerun`, `passes_alone` and
  `required_check_could_not_run`.
