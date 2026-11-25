# Spec 114 context

Background for [`proposal.md`](proposal.md).

## Evidence

On 2026-11-16 the owner ran a planned task on Damaian's own repository and,
three times in one session, could not tell whether the task was done.

1. **A finished task read as broken.** The task went `incomplete` → `failed`
   in the same second. The report's headline was "Verification failed: 2
   worsened failures, 1 unchanged failure. 0 of 8 plan steps finished.", with
   three red "exit code 101" rows. Five tests failed in all. The owner reran
   them himself: four were order-dependent (they pass when run alone), and the
   fifth also passed alone. The task's required check passed 187/187 when he
   ran it directly. The report listed it as `required_check_missing`, because
   it never completed inside the task's run, not as passing. His words: "I was
   confused with the output if the task was actually done or not… If I am
   confused so will other users."
2. **A baseline looked like any other run.** He asked for a baseline run to be
   visibly different from the others: "maybe we need to visually
   differentiate a baseline run from others, to be clear to users."
3. **A question was buried under the report.** The assistant's answer ended
   with a question in prose: "There is nothing to repair… Would you like
   either of those?" A "no repair applied" report card followed it, with a
   badge, Files/Checks/Regressions rows and no buttons. He asked "where did
   it ask me — UI is confusing". The data-dense, button-free card read as the
   final word.

The common root is that a verification outcome has no scannable verdict
separate from its data. Two gaps were named that day:

- The report cannot say that a failing check looks pre-existing or not caused
  by the patch. That only happens if the user asks and the model looks again.
- `failed` covers both "a required check never finished" (rerun it) and "a
  required check ran and failed" (investigate), and the two read the same.

The source observation is in the local planning notes and is not linked from
here (AGENTS.md: planning documents).

## Survey: what exists today

Checked on 2026-11-25 on `main`, after spec 109 (Done 2026-11-25) and specs
112 and 113. Line numbers will drift.

### Completion gate and task status

- The type is `CompletionGateReport` (`completion.rs`), not
  `CompletionReport`. Its fields are `taskId`, `eligible`, `reasons:
  [{code, explanation}]`, `waivers` and `acceptedFailures: [{check,
  provenance, explanation}]` (the last is omitted when empty).
- The reason codes are `open_plan_steps`, `blocked_plan_steps`,
  `verification_due`, `verification_unfinished`, `pending_decision`,
  `required_check_missing`, `required_check_failed`, `required_check_stale`
  and `required_check_skipped`. Each comes with an engine explanation, for
  example "Required check {label} has no recorded result." or "Required check
  {label} failed: exit code N".
- `required_check_failed` is used for two things: a non-zero exit, and
  `CheckOutcome::Environment` ("could not run: {termination}"). Only the
  explanation tells them apart.
- `write_settlement` sets the status:
  - `Gate` with an eligible report → `complete`;
  - `Gate` otherwise → `incomplete`;
  - `VerificationFailed` → `failed`, **whatever the report says**.
- `chat/verify.rs` settles these verification outcomes through `Gate`:
  passed, nothing verified, no checks, documentation only, only pre-existing
  failures, build failed. These go through `VerificationFailed`: failed,
  repair limit exhausted, repair stalled, no repair applied. An answered turn
  settles `incomplete` first, and the completion verification that follows
  can then settle the same task `failed`. That is the "same second"
  transition. The 2026-11-16 log was not available to replay; the mechanism
  is what the code does now.
- **Claim holds, and it is worse than reported.** A `failed` task in an
  ordinary session shows no reasons at all:
  - `TaskControl::for_session` gives `Failed` the headline "Task failed", the
    detail "The task did not complete." and no `primary_action`.
  - `renderTaskControlInto` (`app.js`) returns without drawing when there is
    no primary action.
  - So only `incomplete` (with **Continue task**) gets a Task outcome card.
  - `TaskControl` carries `reasons` and `waivers` but not `acceptedFailures`.
    Those are folded into the detail string, and only for `complete` and
    `incomplete`.
- `SessionStore::continue_incomplete_task` refuses any status other than
  `incomplete`, and it compares the stored report exactly.

### Verification report

- **The engine writes the wording.** `VerificationReport::summary` writes
  the headline. Failures read "Verification failed: {failure_phrase}. {n} of
  {m} plan steps finished.", or "Verification stopped because no repair patch
  was applied and …".
- `failure_phrase` joins `provenance_count` phrases. Those count **checks**,
  not tests: "2 worsened failures" means two checks.
- **Claim holds: nothing says "not yours".** The provenance labels
  (`provenance_label`) are "Introduced regressions", "Worsened failures",
  "Pre-existing failures", "Unchanged failures", "Environment failures",
  "Uncertain provenance" and "Unknown provenance".
  - The summaries say "before the patch", but the baseline is the
    **task-start** tree (spec 109 D2).
  - `unchanged_failure` is only reached when the check's baseline also
    failed. It means "failing the same way as before this task", but its
    label and summary ("still fail the same way") do not say so.
- **Claim holds: the card is the last word.** `createVerificationReportCard`
  (`app.js`) is appended to the turn's bubble **after** the answer text.
  - Its badge is `report.outcome` with underscores replaced ("no repair
    applied").
  - Every row is always expanded: Files changed, Build, Checks
    passed/failed/skipped/stale/reused/deferred, Behaviour verified,
    Unverified steps, Regressions, one row per provenance group, What you can
    do, Outcome.
  - Its only button is **Continue repair**, at the repair limit (spec 96).
- A reloaded report is text: the stored message is `summary` plus
  `render()`.
- A selected check with no recorded result is listed under "Checks failed",
  with "no result was recorded".

### Baselines

- `verification_baseline` is a **session-log event**
  (`VerificationEvent::Baseline {taskId, check, baseline, tests}`, written by
  `BaselineCoordinator::record`). It is not a stream payload, and `app.js`
  never reads it. `CheckBaseline` is `passed`, `failed` (with findings),
  `declined`, `uncertain` or `unavailable`.
- While a baseline runs, the turn's status line shows engine progress
  phases: "Running baseline: {check}[, only the N failing tests]", then
  "Baseline passed/failed/could not run/cancelled/skipped: {check}"
  (`baseline.rs` `running_label` and `finished_label`, mapped to an ordinary
  `PhaseKind::Tool` in `chat/verify.rs`). Nothing marks the phase as a
  baseline, and nothing remains of it after the turn.
- A baseline that needs approval parks on the ordinary check card
  (`createCheckBatchCard`). The card uses the engine's `title` ("A check
  failed: is the failure new?") and `note` ("…Run it on the files as they
  were before this task's changes…").
  - `FAILURE_BASELINE_PROMPT` reaches only `BaselineCheck.prompt`, which the
    shell does not send.
- **One claim no longer holds.** The observation said a baseline command
  shows as a plain "Ran `cargo test`" row.
  - Those rows come from the model's own tool calls
    (`tool_call_summary`).
  - A baseline writes no chat message.
  - Since spec 109 removed the pre-apply and pre-command baselines, a
    baseline is visible only as a transient status phase or as its card. The
    gap remains (no marker, no record), but it looks different now.
- **New since the observation, and the main correctness finding.** Spec 109
  Task 4 (2026-11-18) narrowed a Rust test baseline to `-- --exact <failing
  tests>`: the failing tests run **alone** on the task-start tree.
  - An order-dependent test fails in the full run and passes alone. Its
    baseline therefore records `Passed`, `classify` returns `new_regression`,
    and an **automatic repair starts** for a failure the task did not cause.
  - Four of the five 2026-11-16 failures had this shape.
  - Spec 102 (non-goals) deferred flake classification to "a later
    specification". This is that specification, for this one shape.

### Questions and spec 113

Spec 113 requirement 4 tells the model, in Code and Plan modes with a user
present, to ask a choice with `ask_user` rather than in prose. That makes a
question card and locks the composer, which covers the "prose question" half
of the third instance. Whether a repair turn offers `ask_user` is not settled
by this survey. `tool_offer.rs` offers it when the turn has a question route,
and Task 4 checks this for the repair route. This spec covers the other half:
the report card must not read as the final word.

## Decisions

- **D1 — a verdict, owned by the engine (decided here).** The report gains
  one `verdict` `{kind, headline, detail}` computed in `verification.rs`.
  Renderers print it and never derive it (this is the rule provenance
  labels already follow).
  - `summary()` stays as it is, because it feeds the model's
    continuation prompts and eval assertions.
  - The text copy (`render()`) starts with the verdict headline, so a
    reloaded report says the same thing as the live card.
  - Precedence when several apply: `build_failed` > `caused_by_task` >
    `cause_unknown` > `not_caused_by_task` > `not_verified` > `passed`.
- **D2 — words, not new states (decided here).** No new task status, and no
  change to what `incomplete` and `failed` mean in the log; the status set is
  persisted and read by recovery, workers and the sidebar. The split the
  owner asked for is made in what is shown:
  - the verdict;
  - the Task outcome card's headline;
  - one new reason code, `required_check_could_not_run`, taking
    `CheckOutcome::Environment` out of `required_check_failed`. Old reports
    still read: the code is additive, and old logs keep the old one.
- **D3 — tests that pass alone (owner, 2026-11-25: "Rerun alone on current
  files").**
  - **When it applies:** a narrowed baseline (spec 109 D3) recorded `Passed`
    for some failing tests.
  - **The rerun:** the engine runs the same narrowed command once on the
    current tree, at the check's own directory in the user's checkout.
  - **Classification:** a test that passes there too is "passes when run
    alone". Its finding is not the task's. It is removed from the introduced
    set and is never a repair target.
  - **A check whose every introduced finding passes alone** gets the new
    provenance `passes_alone`:
    - it is not repair-eligible;
    - the gate accepts it as it accepts `pre_existing` (an entry in
      `acceptedFailures` with an explanation that names the tests);
    - the verdict counts it as not caused by the task.
  - **A mixed check** stays `new_regression` or `worsened`, with only the
    remaining findings introduced.
  - **The rerun never adds a card.** It runs only when its command would run
    without approval: auto-run, a matching rule, or spec 109 Task 5's
    Approve for this session. Otherwise it is skipped and classification is
    unchanged. The skip is recorded.
  - **It is recorded** as `verification_isolated_rerun {taskId, check,
    tests, passedAlone}`. Additive: no historical log has one, and replay
    must not invent one.
  - This is one rerun of one shape, not repeat-based flake detection; that
    stays out of scope.
- **D4 — a failed task explains itself and can be continued (owner,
  2026-11-25: "Card + Continue task").**
  - **The card:** `TaskControl` for `failed` gets a Task outcome card like
    `incomplete`, with its reasons grouped under three engine labels:
    **Fails** (`required_check_failed`), **Did not run or finish**
    (`required_check_missing`, `verification_unfinished`,
    `required_check_stale`, `required_check_skipped`,
    `required_check_could_not_run`, `verification_due`), and **Not caused by
    this task** (`acceptedFailures`, now carried in `TaskControl`).
  - **The headline names the cause:** "Task failed: a check fails" when
    anything is under Fails; otherwise "Task not verified: a check did not
    finish".
  - **The action:** the primary action is **Continue task**, through the
    existing route. `continue_incomplete_task` accepts `failed` as well, with
    the same exact-report comparison, and grants nothing.
  - **The exception:** when the verification ended
    `repair_limit_exhausted`, spec 96's **Continue repair** on the report is
    the action. The task card then has no button, and its detail points to
    it. One primary per surface (UI style guide §1).
  - `complete` keeps no card; its verdict says enough.
- **D5 — a baseline is marked as one (decided here; the owner's
  direction).**
  - **The progress phase is typed.** The baseline's progress phase carries a
    typed marker: a phase kind or a flag, which Task 4 chooses. The UI keys
    on the marker, never on label text.
  - **New wording:**
    - running: "Checking whether `{check}` failed before this task's
      changes", plus ", only the N failing tests" when narrowed;
    - finished: "Before this task's changes: `{check}` passed / failed /
      could not run / was skipped".
  - **One tag:** the UI draws one quiet **Baseline** tag (one class, one
    token, added to the style guide) on the running row, on a baseline's
    check card, and on the report's baseline rows.
  - **The record:** the report gains `baselines: [{check, result, tests,
    label}]`, built from the baselines `finish_verification` already
    reads. The finished turn then keeps a record of what was compared.
- **D6 — the report no longer buries what follows (decided here).**
  - **Collapsed:** in a live turn the report card shows the verdict
    headline and detail, and one collapsed disclosure ("Details · 3 checks ·
    2 files", `createDisclosure`) holding today's rows. The raw outcome badge
    is replaced by the verdict, coloured by kind with the existing state
    tokens.
  - **Continue repair** stays in the footer.
  - **Where the card goes:** it stays where it is in the turn. Collapsed to
    two lines, it no longer hides the answer above it. Spec 113 requirement
    4 covers whether a choice is asked in prose.
- **D7 — wording of the provenance groups (decided here).** The labels say
  whose failure it is, and the summaries say "before this task's changes"
  instead of "before the patch":

  | Class | New label |
  |---|---|
  | `new_regression` | "Caused by this task" |
  | `worsened` | "Made worse by this task" |
  | `pre_existing` | "Failed before this task too" |
  | `unchanged_failure` | "Failing as before this task" |
  | `passes_alone` | "Passes when run alone" |
  | environment | unchanged |
  | uncertain | unchanged |
  | unknown | unchanged |

  `provenance_count` follows the same wording. The enum's wire names do not
  change.
- **D8 — parallel work (decided here).**
  - Spec 33 Tasks 7–10 are open. Tasks 7 and 8 own `mcp.rs`, chat dispatch,
    `audit.rs` and `tests/foundation.rs`. Task 9 owns the settings panel in
    `app.js`, `index.html` and the stylesheet. Task 10 owns the user guide,
    troubleshooting and changelog.
  - This spec's engine tasks (1–3) edit `verification.rs`, `provenance.rs`,
    `baseline.rs`, `completion.rs`, `task_control.rs`, `session.rs` (the
    continue guard), `chat/verify.rs` and the shell's report JSON. They do
    not touch spec 33's files, so they may run alongside spec 33 Tasks 7–8.
  - Task 4 edits `app.js` and `style.css`, so it must not run at the same
    time as spec 33 Task 9.
  - Task 5 must not run at the same time as spec 33 Task 10.
