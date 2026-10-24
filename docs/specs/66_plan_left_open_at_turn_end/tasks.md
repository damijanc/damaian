# A Plan Left Open When the Turn Ends — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Implements:** [`proposal.md`](proposal.md) in full · corrections and the
decisions it left open in [`context.md`](context.md)
**Started:** not yet (planned 2026-10-24)

## Progress

| Task | State | Notes |
|---|---|---|
| 1 · `TaskPlan::open_steps` and one definition of open | Not started | **Edits `chat.rs`.** Reads [`context.md`](context.md) §1 and §8. Replaces the two inline "open step" filters in `chat.rs` |
| 2 · Persisted evidence and the per-turn fields | Not started | **Edits `chat.rs`.** Adds two `#[serde(default)]` fields to `PendingChatTurn` and seeds them on resume. Read D6 |
| 3 · The approved command's exit reaches the step | Not started | **Edits `chat.rs`.** The command-approval resume; D14 |
| 4 · Turn-end evidence attach and `plan_left_open` | Not started | **Edits `chat.rs`.** Normal end, cancel, failure and declined fallback; never on a pause. D6 |
| 5 · The reminder in the loop | Not started | **Edits `chat.rs` and `session.rs`.** Once per turn, after the hook, budget-gated, two-answers rule, declined plan skipped; D1–D5, D10, D12 |
| 6 · The reminder across pauses and with hooks | Not started | **Edits `chat.rs` only if a test finds missing wiring.** Approval, plan-review, question, *Continue*, restart, `before_task_complete` order |
| 7 · The panel: `active`, summary, "not finished", live event | Not started | **Edits `chat.rs`.** `TaskStatus::is_active`, `summary(active)`, `plan_json(active)`, `TurnProgress::PlanFinished`, `renderPlanPanel`; D7–D9 |
| 8 · The session-log reminder line and the next-turn seed | Not started | **Edits `chat.rs`.** `ChatMessage.kind` rendering and the `build_model_prompt` filter; D10 |
| 9 · Eval fields and the deterministic scenarios | Not started | `RecordedPlan.remindersSent`/`leftOpenAtEnd`/`openStepEvidence`, five asserts, two scenarios, count 23 → 25 |
| 10 · Docs, acceptance walk, live run, close | Not started | Do not run the full workspace gate locally; it runs on CI. Mark Done everywhere |

**Goal:** When a turn would end while its plan still has pending or in-progress
steps, Damaian sends one reminder asking the model to close the steps it
finished, keeps every step's evidence across pauses and at the end of the turn,
and stops calling an ended turn's open step "running". Nothing is closed on the
model's behalf: a step's status still comes from what Damaian observed, and an
open step stays open.

**Architecture:** Two new per-turn facts travel with the existing
`PendingChatTurn`: `step_evidence` (the buffer that today dies with the loop)
and `plan_reminder_sent` (so a resume cannot remind twice). The reminder is
built from `TaskPlan::open_steps()` at the two `Answered` completion points,
after `before_task_complete` allows, and only when the round budget leaves room
for a tool-carrying call and the token ceiling's pre-call check still passes. A
true turn end attaches the buffer to the open step through
`SessionStore::append_step_evidence` and audits `plan_left_open`; a pause
carries it instead. The plan JSON and live plan event gain `active`, derived
from an exhaustive `TaskStatus::is_active`, and the panel labels an ended turn's
`in_progress` step "not finished". A declined plan's steps are `Skipped` by the
user's decision, so they are not open. The reminder is an ordinary user message
in the request and a tagged `plan_reminder` entry in the session log.

**Tech Stack:** Rust 2024, the existing `serde`/`serde_json` dependencies, the
existing session log, vanilla JavaScript in `app.js`. No new dependencies.

## Global Constraints

Every task's requirements implicitly include this section.

- **Read [`context.md`](context.md) in full before Task 1.** Several of its
  decisions override the proposal. Do not re-derive any of these from
  `proposal.md` alone:
  - §3 D1: `before_task_complete` runs **first**; the reminder only after it
    allows. The hook may fire twice on a reminded turn, and both re-entries
    charge the same action budget;
  - §3 D2: no reminder on *Answer now*;
  - §3 D3: a question is a pause, never a completion, and its answer is not an
    approval;
  - §3 D4: the two-answers rule. The pre-reminder answer is the response only on
    a true completion; a pause or budget stop after the reminder keeps its own
    response;
  - §3 D5: a declined plan's non-terminal steps are `Skipped`;
  - §3 D6: a pause carries the buffer; a true end settles it;
  - §3 D7: `active` is an exhaustive `TaskStatus` match and is **not**
    `!is_terminal()`; `Interrupted` and `UnknownExternalOutcome` are inactive;
  - §3 D10: the session-log reminder is tagged and skipped by the next turn's
    `build_model_prompt`;
  - §3 D11: the eval fields live on `RecordedPlan`, and reminders are a count.
- **#66 edits `chat.rs` in Tasks 1–8.** Of the in-flight specs, #26 (context
  assembly, in progress) and #65 (exploration memory, planned in parallel) both
  edit `chat.rs`. #66 may pair only with a spec that never edits `chat.rs`;
  neither qualifies, so #66 runs as a solo `chat.rs` track. Before Task 1, read
  the progress tables of #26 and #65. If either is in `chat.rs`, wait or agree
  which track rebases; do not edit the same function in parallel.
- **`PendingChatTurn` is shared with #61, #50 and #65.** Its seven construction
  sites are `chat.rs:3496`, `:3765`, `:4141`, `:4253`, `:4376`, `:4760` and
  `:4881`. Every literal is exhaustive (no `..`), so the compiler finds them;
  add both fields to all seven. If #65 has landed, add its `turn_notes`/call-list
  fields in the same rebase. Old pending turns must load: every new field is
  `#[serde(default)]`, and Task 2 tests the stripped-file case.
- **A step is never closed because the turn ended.** `append_step_evidence`
  attaches; it does not recompute a status. No task may call
  `status_from_evidence` on attach, mark an open step complete, or skip a step
  except the user's declined-plan decision in Task 5. #21 requirement 6 stands.
- **The reminder is at most once per turn**, across every resume, restart and
  *Continue*; never on *Answer now*; never on a pause; never when the round or
  token budget has no room for a tool-carrying call. It is a nudge, not an
  approval and not a closure.
- **The audit carries ids and counts, never step titles.** `plan_reminder_sent`
  and `plan_left_open` go through `AuditLog::record`, which redacts.
- **Tests first, and falsify every load-bearing test.** Write the named failing
  test, confirm it fails, implement, then break what the test guards, confirm it
  fails, restore, and record the mutation in the Progress row. This repository
  has a history of tests that passed without testing anything.
- **Fixtures:** engines built in tests set `enable_index_watcher: false` and a
  `data_dir` inside the temporary repository. Model-dependent paths use
  `MockModelAdapter` (its `requests` records each round) or
  `DAMAIAN_MOCK_MODEL_RESPONSE`. Do not touch the user's real Damaian data.
- **A private Cargo target directory per worktree.** Prefix every `cargo`
  command with `CARGO_TARGET_DIR="$PWD/target"` from the worktree root.
- **Scoped checks per task; the full gate runs on CI only** (AGENTS.md, since
  2026-10-15). Each task runs `scripts/quality-gate.sh fmt node lint typos deny`,
  `cargo clippy -p <crate> --all-targets --locked -- -D warnings` for every
  crate it touches, and the named test files. Do not run
  `cargo clippy --workspace` or `cargo nextest run --workspace` locally.
- **Never `git commit` or `git push` unasked.** Each task ends by showing the
  change and the check results and asking. One-line commit subject, no body.

## File Structure

| File | Change |
|---|---|
| `crates/workspace-engine/src/plan.rs` | `TaskPlan::open_steps` (Task 1); `PlanReport::summary(active)` (Task 7) |
| `crates/workspace-engine/src/session.rs` | `TaskStatus::is_active` (Task 7); `ChatMessage.kind` and `append_message_with_kind` (Task 5) |
| `crates/workspace-engine/src/chat.rs` | **Tasks 1–8.** `open_steps` call sites; `PendingChatTurn` fields and seeds; command-resume evidence; turn-end settle and audit; the reminder and declined plan; the final plan event; the prompt filter |
| `crates/workspace-engine/tests/plan.rs` | `open_steps` and summary tests; every existing `summary()` call updated |
| `crates/workspace-engine/tests/plan_turn.rs` | The turn-level reminder, two-answers, pause, cancel and final-event tests |
| `crates/workspace-engine/tests/round_budget.rs` | Pause persistence, old pending file, *Continue* state |
| `crates/workspace-engine/tests/clarification.rs` | Question-pause evidence and reminder state |
| `crates/workspace-engine/tests/hooks.rs` | Hook-order tests |
| `crates/workspace-engine/tests/foundation.rs`, `provider_limits.rs` | Cancelled and failed turn ends |
| `crates/desktop-shell/src/lib.rs` | `plan_json(active)`, `TurnEvent::Plan { active }`, task-state `active`; tests |
| `crates/desktop-shell/src/worker.rs` | Map `PlanFinished` into the worker's plan progress |
| `crates/desktop-shell/static/app.js` | `renderPlanPanel` "not finished"; render the tagged reminder as a system line |
| `crates/eval-harness/src/record.rs`, `runner.rs`, `assertions.rs`, `scenario.rs` | `RecordedPlan` fields, the new asserts, the run's last message |
| `crates/eval-harness/scenarios/plan_answer_without_complete_step.toml`, `plan_left_open_at_end.toml`, `planned_task.toml` | Two new scenarios; `planned_task` asserts no reminder |
| `crates/eval-harness/tests/harness.rs` | Scenario tests; count guard 23 → 25 |
| `docs/USER_GUIDE.md`, `docs/TROUBLESHOOTING.md`, `docs/specs/README.md`, `CHANGELOG.md`, `proposal.md`, this file | Task 10 |

## Interface reference

Checked on 2026-10-24. Re-check a line before editing near it; #26 and #65 will
move some of these.

```rust
// plan.rs
impl TaskPlan {
    pub fn open_steps(&self) -> Vec<&PlanStep>;   // Pending | InProgress, plan order
}
impl PlanReport {
    pub fn summary(&self, active: bool) -> String;
}

// session.rs
impl TaskStatus {
    pub fn is_active(&self) -> bool;              // exhaustive, 15 variants
}
pub struct ChatMessage {
    // …existing…
    pub kind: Option<String>,                     // "plan_reminder" or None
}
impl SessionStore {
    pub fn append_message_with_kind(
        &self, session_id: &str, task_id: Option<&str>, role: &str,
        content: &str, kind: Option<&str>,
    ) -> Result<ChatMessage>;
}

// chat.rs
struct PendingChatTurn {
    // …existing…
    #[serde(default)] step_evidence: Vec<crate::plan::Evidence>,
    #[serde(default)] plan_reminder_sent: bool,
}
fn plan_reminder_message(plan: &TaskPlan) -> String;
enum TurnProgress { /* … */ PlanFinished(TaskPlan) }

// desktop-shell
pub(crate) fn plan_json(plan: &TaskPlan, active: bool) -> String;
enum TurnEvent { /* … */ Plan { plan: Box<TaskPlan>, active: bool } }
```

Key current lines: `run_agentic_turn` `chat.rs:2505`; step local `:2731`;
completion points `:3259` (after the hook at `:3247`) and `:3301` (after the
hook at `:3290`); `complete_step` `:3975`–`:4053`; `Answered => Complete`
`:4658`; `PendingChatTurn` `:5696`; seven literals `:3496`, `:3765`, `:4141`,
`:4253`, `:4376`, `:4760`, `:4881`; `finish_cancelled_turn` `:4987`;
`plan_review_response` `:6692`; tool descriptions `:6794`, `:6802`;
`build_model_prompt` `:6101`; `bounded_messages` `:6161`; `plan_json`
`lib.rs:3923`; `renderPlanPanel` `app.js:7482`.

## Order and parallelism

```
1 ──> 2 ──> 3 ──> 4 ──> 5 ──> 6 ──> 7 ──> 8 ──> 9 ──> 10
```

Sequential. Task 2 must land before Task 5 (the reminder needs the fields); Task
4 before Task 9 (the `plan_left_open` record); Task 5 before Tasks 6–9. Tasks 1,
2, 4, 5, 7 and 8 all edit `chat.rs`; no task may run beside #26 or #65 while
they edit the same file.

---

## Task 1: `TaskPlan::open_steps` and one definition of open

**Requirements:** 1, §5.1. **Proposal:** §5.1. **Context:** §1, §8.
**Files:** `plan.rs`, `chat.rs` (the two inline filters), `tests/plan.rs`.

- [ ] **Step 1: Write the failing tests** in `crates/workspace-engine/tests/plan.rs`
  (import `TaskPlan`, `StepStatus`, and the existing `step()` helper):
  - `open_steps_are_the_pending_and_in_progress_ones_in_plan_order`: a plan with
    `Completed`, `InProgress`, `Skipped`, `Pending`, `Blocked` yields exactly the
    `InProgress` and `Pending` step ids, in plan order;
  - `a_plan_with_only_terminal_steps_has_no_open_steps`: Completed, Blocked and
    Skipped only → empty;
  - `a_blocked_step_is_not_open`: one Blocked step → empty, so the reminder can
    never be sent for it.
- [ ] **Step 2: Run them and confirm they fail to compile** on the missing
  method: `CARGO_TARGET_DIR="$PWD/target" cargo nextest run -p workspace-engine --test plan`.
- [ ] **Step 3: Implement.** Add `TaskPlan::open_steps(&self) -> Vec<&PlanStep>`
  beside `phase()`, filtering `!step.status.is_terminal()`. Doc comment: open is
  `Pending | InProgress`; `Blocked` is terminal for this purpose because #21
  already stops the handoff and tells the model. Replace the two inline
  definitions with it:
  - `carry_plan_from_the_previous_turn` (`chat.rs:2431`–`:2436`);
  - `token_budget_exhausted_response` (`chat.rs:7510`–`:7523`).
  No behaviour change; the existing token-ceiling and plan-carry tests pin it.
- [ ] **Step 4: Falsify.** Make `open_steps` include `Blocked` → the blocked
  test fails; exclude `InProgress` → the order test fails. Restore both.
- [ ] **Step 5: Scoped checks.**

  ```bash
  scripts/quality-gate.sh fmt node lint typos deny
  CARGO_TARGET_DIR="$PWD/target" cargo clippy -p workspace-engine --all-targets --locked -- -D warnings
  CARGO_TARGET_DIR="$PWD/target" cargo nextest run -p workspace-engine --test plan --test plan_turn
  ```

- [ ] **Step 6: Update this task's Progress row** (what landed, checks,
  mutations) and ask before committing. Subject:
  `Give a plan one definition of an open step`.

## Task 2: Persisted evidence and the per-turn fields

**Requirements:** 3 (first half), §5.3. **Context:** §3 D6, §5.
**Files:** `chat.rs`; tests `plan_turn.rs`, `round_budget.rs`,
`clarification.rs`.

- [ ] **Step 1: Write the failing tests.**
  - `a_plan_review_pause_carries_the_evidence_it_has_accrued`
    (`plan_turn.rs`): propose a two-step plan; in the next round script
    `read_file` on `src/a.rs` **and** a `propose_patch` call, so the read runs
    and the patch triggers the review gate. Read the pending JSON from
    `.damaian/chat/pending/<id>.json`: `step_evidence` holds a `fileRead` for
    `src/a.rs`. Approve the plan, script the patch again followed by
    `complete_step`, and assert the closed step's `evidence` still contains the
    read. (Reuse `decide`, `patch_call` and `plan_of`.)
  - `a_question_pause_carries_the_evidence_it_has_accrued`
    (`clarification.rs`): a plan, a read, then `ask_user`; the pending JSON has
    the read; answer and `complete_step`; the step's evidence has it.
  - `a_round_budget_pause_carries_the_evidence_and_the_reminder_flag`
    (`round_budget.rs`): with a plan and a read before the budget, the pending
    JSON has `step_evidence` non-empty and `plan_reminder_sent: false`.
  - `a_pending_turn_written_before_step_evidence_still_loads`
    (`round_budget.rs`): take the pending JSON, `remove("step_evidence")` and
    `remove("plan_reminder_sent")`, rewrite it, resume with *Continue*; the
    resume succeeds. Follow the pattern at
    `tests/exploration_rounds.rs:565`–`:600`.
- [ ] **Step 2: Run them and confirm they fail** (or fail to compile on the
  missing fields): the three test files named above.
- [ ] **Step 3: Implement.**
  - Add to `PendingChatTurn` beside `exploration_rounds`/`round_budget`:
    `#[serde(default)] step_evidence: Vec<crate::plan::Evidence>` and
    `#[serde(default)] plan_reminder_sent: bool`, with comments saying why a
    pause carries them and why the defaults keep old files loadable.
  - Add `step_evidence: Vec<Evidence>` and `plan_reminder_sent: bool` parameters
    to `run_agentic_turn` (both `mut`), delete the `Vec::new()` initialiser at
    `:2731`, and seed from the parameters. The turn starts at empty/false.
  - Pass the pending fields at all six resume call sites (`:1430`, `:1555`,
    `:1707`, `:1847`, `:1949`, `:1983`) and empty/false at the initial ask
    (`:1055`).
  - Add both parameters to `pause_for_round_budget` and
    `pause_for_provider_fallback` and write them at `:4881` and `:4760`. Write
    them at the other five literals from the in-scope locals.
  - Correct the comment at `chat.rs:2717`–`:2726`: a crash loses the evidence of
    the running step **unless** the turn was paused, because a paused turn now
    carries it.
- [ ] **Step 4: Falsify.** (1) Seed `step_evidence` from `Vec::new()` on resume →
  the plan-review resume loses the read, the first test fails. (2) Save an empty
  vector at a pause → the pending-JSON assertion fails. (3) Remove
  `#[serde(default)]` from `step_evidence` → the old-file resume fails to load.
  Restore each.
- [ ] **Step 5: Scoped checks.**

  ```bash
  scripts/quality-gate.sh fmt node lint typos deny
  CARGO_TARGET_DIR="$PWD/target" cargo clippy -p workspace-engine --all-targets --locked -- -D warnings
  CARGO_TARGET_DIR="$PWD/target" cargo nextest run -p workspace-engine --test plan_turn --test round_budget --test clarification
  ```

- [ ] **Step 6: Update the Progress row** and ask before committing. Subject:
  `Carry a turn's step evidence across its pauses`.

## Task 3: The approved command's exit reaches the step

**Requirements:** 3 (second half), §5.3. **Context:** §1, §3 D14.
**Files:** `chat.rs`; tests `plan_turn.rs` (or `command_lifecycle.rs`).

- [ ] **Step 1: Write the failing test**
  `an_approved_command_reaches_the_open_step_as_evidence` in `plan_turn.rs`:
  propose a plan; approve the plan review; then script a command that needs
  approval (the `sed -i '' 's/1/2/' src/a.rs` call used by
  `round_budget.rs:100`) so the turn pauses for the command; approve it with
  `resume_after_command_decision`; then script `complete_step` and read the plan
  through `plan_of`. Assert the closed step's evidence contains an item with
  `kind: "commandExit"`, `exitCode: Some(0)`, and the purpose the classifier
  assigned, **and** any read that preceded the pause. Also assert the same
  `commandId` appears on the `stored_command_executed` audit event.
- [ ] **Step 2: Run it and confirm it fails**: the command runs (the audit has
  it) but the plan's evidence does not.
- [ ] **Step 3: Implement.** In `resume_after_command_decision_with_options`,
  after `run_proposal` returns `record`, push
  `evidence_for(&ActionOutcome::CommandExit(record.execution.exit_code,
  record.execution.purpose), &record.execution.id)` onto the mutable
  `step_evidence` local taken from `pending.step_evidence`, before calling
  `run_agentic_turn`. Use the execution id, not the finished
  `awaiting_approval` marker (D14). Do not add a marker around the resumed
  execution.
- [ ] **Step 4: Falsify.** Push nothing → the test fails; push
  `ActionOutcome::Ok` → `evidence_for` returns `None` and the test fails; use
  `pending.proposal_id` as the marker id → change the test to assert the id
  matches the `stored_command_executed.commandId`; it fails. Restore.
- [ ] **Step 5: Scoped checks.**

  ```bash
  scripts/quality-gate.sh fmt node lint typos deny
  CARGO_TARGET_DIR="$PWD/target" cargo clippy -p workspace-engine --all-targets --locked -- -D warnings
  CARGO_TARGET_DIR="$PWD/target" cargo nextest run -p workspace-engine --test plan_turn --test command_lifecycle
  ```

- [ ] **Step 6: Update the Progress row** and ask before committing. Subject:
  `Count an approved command's exit as plan evidence`.

## Task 4: Turn-end evidence attach and `plan_left_open`

**Requirements:** 4, 5, 7 (the audit half), §5.3. **Context:** §3 D6.
**Files:** `chat.rs`; tests `plan_turn.rs`, `foundation.rs`,
`provider_limits.rs`, `round_budget.rs`.

- [ ] **Step 1: Write the failing tests.**
  - `a_turn_that_answers_with_a_step_open_keeps_it_open_with_its_evidence`
    (`plan_turn.rs`): plan, read `src/a.rs`, answer. At this task there is no
    reminder yet, so the answer ends the turn. Assert the step is still
    `InProgress`, its evidence has the read, and the audit has one
    `plan_left_open` with `openSteps=1`, `reminderSent=false`, `stopReason` the
    answered value. **Task 5 will add a post-reminder scripted answer to this
    test; leave a comment saying so.**
  - `a_turn_that_runs_out_of_rounds_keeps_its_step_open`
    (`plan_turn.rs`): a plan then action rounds to the budget; `ToolBudget` is
    the stop, the step stays `InProgress` with its evidence, and `plan_left_open`
    says `tool_budget`.
  - `a_stopped_turn_keeps_its_step_open_with_its_evidence` (`plan_turn.rs` with
    a local cancel helper modelled on `foundation.rs:1388`, or in
    `foundation.rs` if the helper is reusable): cancel after a read; the step
    stays open with the read; the audit says `cancelled`.
  - `a_provider_failure_keeps_its_step_open` (`provider_limits.rs`): a failing
    model transport after a plan and read; the task is `Failed`, the step has
    the evidence, the audit says `failed`.
  - `a_pause_does_not_attach_or_audit` (`round_budget.rs` or `plan_turn.rs`): a
    command-approval pause with evidence in the pending turn writes **no**
    `plan_left_open` and no new `plan_step_updated` for the open step; the
    evidence is in the pending JSON instead.
- [ ] **Step 2: Run them and confirm they fail** (no `plan_left_open`, no
  attached evidence).
- [ ] **Step 3: Implement.**
  - Add a helper, e.g.
    `fn settle_plan_at_turn_end(&self, session: &Session, task: &Task, plan: Option<&mut TaskPlan>, step_evidence: &mut Vec<Evidence>, reminder_sent: bool, stop_reason: &str)`.
    It returns early with no plan. It finds the `InProgress` step, extends its
    in-memory evidence with the buffer, and calls
    `append_step_evidence` once per item, best-effort (`let _ =`). If
    `plan.open_steps()` is non-empty, audit `plan_left_open` with `taskId`,
    `sessionId`, `openSteps`, `reminderSent`, `stopReason`. Clear the buffer.
  - Call it at the normal `(None, None)` end before `update_task_status`;
    extend `finish_cancelled_turn` (`:4987`) with the plan, buffer, reminder flag
    and sink and call it with `"cancelled"`; call it with `"failed"` before both
    `Failed` writes + `return Err` (`:3104` uses
    `update_task_status_with_kind`, `:3112` uses `update_task_status`); and in
    the declined provider fallback (`:1758`–`:1780`) using
    `pending.step_evidence` and the plan read from the log.
  - Do **not** call it when `pending` or `pending_question` is set: that is a
    pause.
- [ ] **Step 4: Falsify.** Call it on a pause → the pause test fails; recompute
  the step status after attaching → the "still InProgress" assertion fails; skip
  the cancel path → the cancel test fails; omit the audit → the audit assertion
  fails; put a step title in an audit field → assert the title is absent from
  the audit line and it fails. Restore.
- [ ] **Step 5: Scoped checks.**

  ```bash
  scripts/quality-gate.sh fmt node lint typos deny
  CARGO_TARGET_DIR="$PWD/target" cargo clippy -p workspace-engine --all-targets --locked -- -D warnings
  CARGO_TARGET_DIR="$PWD/target" cargo nextest run -p workspace-engine --test plan_turn --test foundation --test provider_limits --test round_budget
  ```

- [ ] **Step 6: Update the Progress row** and ask before committing. Subject:
  `Attach a turn's step evidence when the turn ends`.

## Task 5: The reminder in the loop

**Requirements:** 1, 2, 4 (declined plan), 5, 7; §5.2. **Context:** §3 D1–D5,
D10, D12; §4.5.
**Files:** `chat.rs`, `session.rs`; tests `plan_turn.rs`, inline `chat.rs`.

- [ ] **Step 1: Write the failing tests** in `crates/workspace-engine/tests/plan_turn.rs`
  unless noted. Use `MockModelAdapter::new_sequence_with_tool_calls` and inspect
  `adapter.requests`.
  1. `an_answer_with_open_steps_gets_one_reminder_and_closes_after_it`: a
     two-step plan; a read; answer "The report is written."; then one round with
     two `complete_step` calls; then "Done.". Assert exactly one
     `plan_reminder_sent` audit event (openSteps 2); the request after the answer
     contains the reminder text (as a user message); `result.response ==
     "The report is written."`; the session log has that answer, a user entry
     with `kind: "plan_reminder"`, and "Done." after it; both steps are terminal.
  2. `a_second_answer_ends_the_turn_with_the_steps_still_open`: plan, read,
     answer, then answer again. One reminder; the steps are still open; the first
     answer is the response; the second is the last assistant message;
     `plan_left_open` says `reminderSent=true`.
  3. `a_closed_plan_gets_no_reminder`: plan, read, `complete_step`, answer. Zero
     `plan_reminder_sent`; exactly the scripted model calls.
  4. `a_plan_with_only_a_blocked_step_gets_no_reminder`: a command that exits
     non-zero, `complete_step` (the step becomes `Blocked`), answer. Zero
     reminders.
  5. `a_planless_turn_gets_no_reminder` (pin): an ordinary answer with no plan;
     zero reminders and today's response.
  6. `a_reminder_is_not_sent_on_answer_now`: reach a round-budget pause, answer
     *Answer now*, let the forced call answer; zero reminders.
  7. `a_reminder_is_not_sent_when_the_round_budget_has_no_room`: configure the
     action limit so the answer arrives at the forced-final call (the round gate
     is false); zero reminders and `Complete`.
  8. `the_reminder_gate_needs_room_under_the_token_ceiling` (inline `chat.rs`
     test of the spend helper): with a ceiling and a recorded spend at or above
     it, the gate is false; below it, true. If the helper is a method needing a
     store, use a `SessionStore` in a temp data dir.
  9. `a_declined_plan_is_marked_skipped_and_never_reminded`: plan; pause for
     review; decline; answer. Both steps are `Skipped`; zero reminders; the
     summary names the skipped steps and does not say "outstanding"; the answer
     is the response.
  10. `the_reminder_text_names_every_open_step_and_the_in_progress_one` (inline
      `chat.rs` test of `plan_reminder_message`): it lists only open steps, marks
      the `InProgress` one "(in progress)", numbers them from 1, and names
      `complete_step`.
  11. `the_reminder_round_is_charged_to_the_action_budget`: after the reminder,
      the `model_request_prepared` record's `toolRound` is one higher, and
      `explorationRounds` did not move.
- [ ] **Step 2: Run them and confirm they fail.**
- [ ] **Step 3: Implement.**
  - Add `plan_reminder_message(plan: &TaskPlan) -> String` beside
    `plan_review_response` (`chat.rs:6692`), using `open_steps()` and the
    proposal §5.2 text.
  - Add a gate helper (e.g.
    `plan_reminder_note(&self, session, task, plan, sent, action_rounds, max_rounds, answer_now) -> Result<Option<(String, usize)>>`)
    returning the message and the open-step count only when: a plan exists, open
    steps are non-empty, `!sent`, `!answer_now`, `action_rounds + 1 < max_rounds`
    (D12), and the token ceiling's pre-call check passes against the spend
    **after** the answer round. Share the spend read with the top-of-loop check
    (`chat.rs:2841`–`:2872`) so they cannot drift.
  - At both completion points — after `before_task_complete` returns `None` at
    `:3247`–`:3258` and `:3290`–`:3300` — when the gate allows:
    1. push the answer with its reasoning to `messages`;
    2. append the answer to the session log if non-empty and remember it as the
       pre-reminder answer;
    3. append the reminder as a user message to `messages`, and to the session
       log via `append_message_with_kind(..., Some("plan_reminder"))`;
    4. set `plan_reminder_sent = true`, audit `plan_reminder_sent` with
       `taskId`, `sessionId`, `openSteps`;
    5. `action_rounds += 1` and `continue`.
  - At the loop end, apply D4: when the turn truly completes (`Answered`, no
    pending proposal or question), `ChatTurnResult.response` is the pre-reminder
    answer; append the post-reminder `response` as a second assistant message if
    non-empty. When it pauses or ends at a budget, keep the loop's response and
    append it as today.
  - Declined plan (D5): in `resume_after_plan_decision`'s `!approved` branch,
    mark every non-terminal step `Skipped`, set `completed_at_ms`, write each
    through `update_plan_step`, and send the plan on `sink`.
  - Add `ChatMessage.kind: Option<String>`, `append_message_with_kind`, and the
    optional `"kind"` field in `message_json` (the write side; Task 8 adds the
    read/parse and the UI).
- [ ] **Step 4: Falsify.** (1) Send the reminder for a closed plan → test 3
  fails. (2) Remove the `plan_reminder_sent` check → a second reminder appears
  and test 1's one-event assertion fails. (3) Use `action_rounds < max_rounds`
  for the gate → test 7 sends a reminder the model cannot act on and fails.
  (4) Skip the Answer-now check → test 6 fails. (5) Leave a declined plan open →
  test 9 fails. (6) Drop the `kind` from the log append → the session-log
  assertion fails. Restore each.
- [ ] **Step 5: Scoped checks.**

  ```bash
  scripts/quality-gate.sh fmt node lint typos deny
  CARGO_TARGET_DIR="$PWD/target" cargo clippy -p workspace-engine --all-targets --locked -- -D warnings
  CARGO_TARGET_DIR="$PWD/target" cargo nextest run -p workspace-engine --test plan_turn --test plan --lib
  ```

- [ ] **Step 6: Update the Progress row** and ask before committing. Subject:
  `Remind a turn once to close the plan steps it finished`.

## Task 6: The reminder across pauses and with hooks

**Requirements:** 2, 3; §5.2's "once per turn". **Context:** §3 D1, D3, D6;
§4.1–§4.3.
**Files:** `chat.rs` only if a test finds missing wiring; tests `plan_turn.rs`,
`round_budget.rs`, `clarification.rs`, `hooks.rs`.

This task is mostly tests: Tasks 2 and 5 should already persist and seed both
fields. If one fails, fix the wiring it names and record the deviation.

- [ ] **Step 1: Write the failing tests.**
  1. `the_reminder_is_not_sent_twice_across_a_command_approval`
     (`plan_turn.rs`): plan/read/answer → reminder; the post-reminder round
     requests an approval command; the pending JSON has
     `plan_reminder_sent: true`; approve; the turn answers; exactly one
     `plan_reminder_sent` event.
  2. `the_reminder_survives_a_restart` (`plan_turn.rs`): the same pause, then a
     second `WorkspaceEngine` over the same `data_dir`; resume; still one
     reminder.
  3. `the_reminder_is_not_sent_twice_across_a_plan_review`
     (`plan_turn.rs`): reminder, then a mutating call raises the plan review
     (the plan was not approved); approve; answer; one reminder.
  4. `continue_keeps_the_reminder_state_and_evidence`
     (`round_budget.rs`): reminder, then action rounds to the round-budget
     pause; the pending JSON has the flag and the evidence; *Continue*; answer;
     one reminder and the step's evidence still reaches the plan.
  5. `a_question_pause_keeps_the_reminder_state` (`clarification.rs`): reminder,
     `ask_user`, pause; the pending JSON has the flag; answer; one reminder.
  6. `a_plan_reminder_waits_for_before_task_complete` (`hooks.rs`): a
     `before_task_complete` hook that denies once (a counter file) and then
     allows. The first request after the answer contains the hook note and
     **not** "Your plan still has open steps"; after the allow, the next request
     contains the reminder; exactly one `plan_reminder_sent`.
  7. `a_hook_that_always_denies_never_gets_a_reminder` (`hooks.rs`): the turn
     ends at the budget with zero reminders and the task
     `ToolBudgetExhausted` (give the test a nextest timeout).
- [ ] **Step 2: Run them and confirm they fail** if the wiring is missing; where
  a test passes immediately, say so in the Progress row and keep it as the
  regression guard.
- [ ] **Step 3: Implement only what a test proves missing.** Likely candidates:
  a resume that forgot to pass `pending.plan_reminder_sent`/`step_evidence`; the
  question/round-budget literal missing a field; the reminder placed before the
  hook. Do not restructure Tasks 2 or 5.
- [ ] **Step 4: Falsify.** Reset the flag in one resume → its test fails; drop
  the evidence in `pause_for_round_budget` → test 4 fails; move the reminder
  before the hook → test 6 fails. Restore.
- [ ] **Step 5: Scoped checks.**

  ```bash
  scripts/quality-gate.sh fmt node lint typos deny
  CARGO_TARGET_DIR="$PWD/target" cargo clippy -p workspace-engine --all-targets --locked -- -D warnings
  CARGO_TARGET_DIR="$PWD/target" cargo nextest run -p workspace-engine --test plan_turn --test round_budget --test clarification --test hooks
  ```

- [ ] **Step 6: Update the Progress row** and ask before committing. Subject:
  `Keep one plan reminder across a turn's pauses`.

## Task 7: The panel: `active`, the summary, "not finished", the live event

**Requirements:** 6, §5.4. **Context:** §3 D7–D9.
**Files:** `session.rs`, `plan.rs`, `chat.rs`, `desktop-shell/src/lib.rs`,
`desktop-shell/src/worker.rs`, `static/app.js`; tests `tests/plan.rs`,
`tests/plan_turn.rs`, inline `desktop-shell/src/lib.rs`.

- [ ] **Step 1: Write the failing tests.**
  - `the_states_whose_turn_is_still_running_are_exactly_the_non_terminal_work_states`
    (`session.rs` `mod tests` or `tests/plan.rs`): over `TaskStatus::all()`, the
    true set is `Created`, `PreparingContext`, `WaitingForModel`, `RunningTool`,
    `WaitingForApproval`, `Paused`, `ApplyingPatch`, `Validating`.
  - `the_two_crash_states_are_not_running_even_though_they_are_not_terminal`:
    `Interrupted` and `UnknownExternalOutcome` are `!is_terminal()` and
    `!is_active()`.
  - `an_ended_plan_summary_says_not_marked_done`: a plan with two open steps,
    `summary(false)` is exactly "Turn ended with 2 steps not marked done.";
    `summary(true)` is today's "2 steps outstanding.".
  - `an_ended_plan_with_a_blocked_step_still_names_it`: blocked + open,
    `summary(false)` contains both "blocked" and "Turn ended with".
  - `a_complete_plan_s_summary_ignores_active`: a complete plan's summary is the
    same for both values.
  - Update every existing `report.summary()` call in `tests/plan.rs`
    (`:1114`, `:1135`, `:1154`, `:1172`, `:1187`, and the assertions that call
    it a second time in their failure message) to `summary(true)`, so today's
    wording is pinned.
  - `an_ended_turn_s_plan_is_not_active` (inline `desktop-shell/src/lib.rs`):
    `task_states_json` with a `Complete` task and an `InProgress` step emits
    `"active":false` and the "Turn ended …" summary; a `RunningTool` task emits
    `"active":true`.
  - `the_plan_event_carries_whether_the_turn_is_active` (inline lib.rs, near
    `:5111`): `TurnProgress::Plan` → `"active":true`,
    `TurnProgress::PlanFinished` → `"active":false`.
  - `the_turn_reports_its_plan_as_finished_when_it_ends` (`plan_turn.rs`): collect
    `TurnProgress::PlanFinished`; it arrives once at a true end, with the open
    step and its attached evidence. `a_pause_reports_no_finished_plan`: a
    command-approval pause emits none.
- [ ] **Step 2: Run them and confirm they fail** (missing variants/fields).
- [ ] **Step 3: Implement.**
  - `session.rs`: `TaskStatus::is_active` with the exhaustive match and the doc
    comment distinguishing it from `is_terminal` (D7).
  - `plan.rs`: `PlanReport::summary(&self, active: bool)`; when
    `!active && outstanding > 0`, lead with "Turn ended with N steps not marked
    done" and keep the blocked/finished/skipped parts (D8).
  - `desktop-shell/src/lib.rs`: `plan_json(plan, active)`; `plan_proposal_json`
    passes `true`; `task_states_json` parses `statuses.get(id)` with
    `TaskStatus::parse` and passes `is_active()`; `TurnEvent::Plan { plan,
    active }`; `turn_progress_event` maps `Plan`/`PlanFinished`.
  - `worker.rs`: map `PlanFinished` to `plan_json(&plan, false)` on the plan
    progress kind (and `Plan` to true).
  - `chat.rs`: add `TurnProgress::PlanFinished(TaskPlan)` and a
    `TurnSink::plan_finished`; call it after Task 4's settle at the normal end,
    in `finish_cancelled_turn`, and before the two failure returns. Do not call
    it on a pause.
  - `app.js`: in `renderPlanPanel`, an `in_progress` step gets "running" when
    `plan.active !== false`, and "not finished" otherwise. Pending steps stay
    unlabelled. Update the `STEP_OUTCOME_LABELS` comment if needed.
- [ ] **Step 4: Falsify.** Use `!is_terminal()` for `active` → the crash test
  fails; always true → the ended test fails; make `summary` ignore `active` →
  the wording test fails; drop `PlanFinished` → the plan_turn test fails; keep
  the "running" label unconditional → the JSON has `active:false` but the UI
  still says running (manual check; the test does not catch it, so record the
  manual check). Restore.
- [ ] **Step 5: Scoped checks.**

  ```bash
  scripts/quality-gate.sh fmt node lint typos deny
  CARGO_TARGET_DIR="$PWD/target" cargo clippy -p workspace-engine --all-targets --locked -- -D warnings
  CARGO_TARGET_DIR="$PWD/target" cargo clippy -p desktop-shell --all-targets --locked -- -D warnings
  CARGO_TARGET_DIR="$PWD/target" cargo nextest run -p workspace-engine --test plan --test plan_turn
  CARGO_TARGET_DIR="$PWD/target" cargo nextest run -p desktop-shell --lib
  node --check crates/desktop-shell/static/app.js
  npm run lint:web
  ```

- [ ] **Step 6: Check the UI by hand.** Rebuild the shell, run it on a spare port
  with a scratch `DAMAIAN_DATA_DIR`, drive a turn that leaves a step open, and
  confirm the panel says "not finished" and "Turn ended with N steps not marked
  done." Record what was checked.
- [ ] **Step 7: Update the Progress row** and ask before committing. Subject:
  `Stop calling an ended turn's plan step running`.

## Task 8: The session-log reminder line and the next-turn seed

**Requirements:** 7, §5.5. **Context:** §3 D10.
**Files:** `session.rs` (the read/parse side; Task 5 landed the write side),
`desktop-shell/src/lib.rs`, `static/app.js`, `chat.rs`; tests inline.

- [ ] **Step 1: Write the failing tests.**
  - `a_message_with_a_kind_round_trips_and_an_old_one_reads_none`
    (`session.rs` `mod tests`): `append_message_with_kind(..., Some("plan_reminder"))`
    reads back with the kind; an ordinary message reads `None`; an old
    `message_appended` line without the field parses.
  - `the_reminder_is_a_system_line_in_a_reloaded_session` (inline
    `desktop-shell/src/lib.rs`): a session log with a tagged reminder serialises
    through `/api/session`'s `message_json` with `"kind":"plan_reminder"`.
  - `a_plan_reminder_does_not_seed_the_next_turn` (inline `chat.rs`): call
    `build_model_prompt` with a prior message whose kind is `plan_reminder`; its
    text is absent. An ordinary user message is still present (the negative
    control).
- [ ] **Step 2: Run them and confirm they fail.**
- [ ] **Step 3: Implement.**
  - `session.rs`: parse `kind` in `parse_message_event` (Task 5 wrote it); the
    question transcript entries set `None`; an old line without the field reads
    `None`.
  - `desktop-shell/src/lib.rs`: `message_json` forwards `message.kind`.
  - `app.js` `renderMessages`: when `message.kind === "plan_reminder"`, render
    through `appendChatMessage("system", message.content)`; do not treat it as
    an answer, a rewind anchor or a question.
  - `chat.rs` `build_model_prompt`: skip `ChatMessage`s with
    `kind == Some("plan_reminder")` in the recent-message seed (D10).
- [ ] **Step 4: Falsify.** Drop `kind` in `parse_message_event` → the shell test
  fails; include tagged entries in `build_model_prompt` → the seed test fails;
  render it as a user bubble → manual check (record). Restore.
- [ ] **Step 5: Scoped checks.**

  ```bash
  scripts/quality-gate.sh fmt node lint typos deny
  CARGO_TARGET_DIR="$PWD/target" cargo clippy -p workspace-engine --all-targets --locked -- -D warnings
  CARGO_TARGET_DIR="$PWD/target" cargo clippy -p desktop-shell --all-targets --locked -- -D warnings
  CARGO_TARGET_DIR="$PWD/target" cargo nextest run -p workspace-engine --test plan_turn
  CARGO_TARGET_DIR="$PWD/target" cargo nextest run -p desktop-shell --lib
  node --check crates/desktop-shell/static/app.js
  npm run lint:web
  ```

- [ ] **Step 6: Update the Progress row** and ask before committing. Subject:
  `Show a plan reminder as a system line, not as the user's words`.

## Task 9: Eval fields and the deterministic scenarios

**Requirements:** §5.5, acceptance "the deterministic scenario passes in CI".
**Context:** §2, §3 D11, §6.
**Files:** `crates/eval-harness/src/record.rs`, `runner.rs`, `assertions.rs`,
`scenario.rs`, `scenarios/plan_answer_without_complete_step.toml`,
`scenarios/plan_left_open_at_end.toml`, `scenarios/planned_task.toml`,
`tests/harness.rs`.

- [ ] **Step 1: Write the failing tests** in
  `crates/eval-harness/tests/harness.rs`:
  - `a_plan_answer_without_complete_step_gets_one_reminder_and_closes_after_it`;
  - `a_plan_left_open_at_end_records_the_reminder_and_the_open_step`;
  - `the_planned_task_scenario_gets_no_reminder`;
  - update `every_scenario_runs_and_none_is_blocked`'s count from 23 to 25 and
    its comment.
- [ ] **Step 2: Run them and confirm they fail** (the scenarios do not exist).
- [ ] **Step 3: Implement.**
  - `record.rs`: `RecordedPlan` gains `reminders_sent: u64`,
    `left_open_at_end: bool`, `open_step_evidence: u64`, with doc comments
    (camelCase on the wire).
  - `runner.rs`: fill them from the final plan and the trace —
    `trace.count("plan_reminder_sent")`, `trace.count("plan_left_open") > 0`, and
    the evidence items on non-terminal steps. Add
    `Run.last_assistant_message: Option<String>`, read from the final task's
    `read_messages` after the turn. Fill it wherever the `Run` is built; if the
    live path has no final task log at that point, leave it `None` and keep the
    assertion `deterministic_only`.
  - `assertions.rs`/`scenario.rs`: add `plan_reminders`, `plan_verified`,
    `plan_unverified`, `plan_outstanding`, `plan_left_open`,
    `plan_open_evidence` and `last_assistant_message_contains`
    (`deterministic_only`; the first five read `RecordedPlan`, the last reads
    `Run.last_assistant_message`).
  - `scenarios/plan_answer_without_complete_step.toml`: a three-step plan; one
    `ls src` (a read-only command that runs unasked); the answer "The report is
    written."; a round with three `complete_step` calls; the answer "Done.".
    Assert `response_contains` the report, `plan_reminders = 1`,
    `plan_verified = 1`, `plan_unverified = 2` (the read's evidence on step 1),
    `plan_outstanding = 0`, `last_assistant_message_contains = ["Done."]`,
    `plan_left_open = false`, `final_task_status = "complete"`.
  - `scenarios/plan_left_open_at_end.toml`: plan, `ls src`, the answer, then a
    second answer with no calls. Assert `plan_reminders = 1`,
    `plan_outstanding = 3`, `plan_left_open = true`, `plan_open_evidence = 1`,
    the first answer is the response, and the last message is the second answer.
  - `scenarios/planned_task.toml`: add `plan_reminders = 0`.
- [ ] **Step 4: Falsify.** Make `reminders_sent` a constant 1 → the planned_task
  zero-reminder scenario fails; make `left_open_at_end` always false → the
  second scenario fails; drop `open_step_evidence` to 0 → the second scenario
  fails; read `last_assistant_message` from the turn response → the "Done."
  assertion fails. Restore.
- [ ] **Step 5: Scoped checks.**

  ```bash
  scripts/quality-gate.sh fmt node lint typos deny
  CARGO_TARGET_DIR="$PWD/target" cargo clippy -p eval-harness --all-targets --locked -- -D warnings
  CARGO_TARGET_DIR="$PWD/target" cargo nextest run -p eval-harness
  CARGO_TARGET_DIR="$PWD/target" cargo run -p eval-harness -- run --tier deterministic
  ```

- [ ] **Step 6: Update the Progress row** and ask before committing. Subject:
  `Measure a plan left open at turn end in the eval harness`.

## Task 10: Docs, acceptance walk, live run, close

**Requirements:** §5.6, §6, §7. **Files:** `docs/USER_GUIDE.md`,
`docs/TROUBLESHOOTING.md`, `proposal.md` §7, `docs/specs/README.md`,
`CHANGELOG.md`, this file.

- [ ] **Step 1: Walk every acceptance criterion in §6** against the finished
  tree and record the per-criterion evidence in `proposal.md` §7 (a table like
  spec 31's §7.1). Name the test for each; say which criteria are manual.
- [ ] **Step 2: Live run per §6.** Re-run §1's prompt against
  `deepseek-v4-flash` on this repository with an isolated `DAMAIAN_DATA_DIR`.
  Accepted if the turn either closes its steps after the reminder or ends with
  the panel reading "not finished", and the step evidence lists the commands it
  ran. Record in §7 and here: which happened, how many tokens the reminder round
  cost, and whether the final response was the report. If the CLI cannot observe
  the panel without a UI, use the desktop shell or a temporary `#[ignore]` driver
  as spec 62 did; delete the driver and data directory afterwards.
- [ ] **Step 3: Documentation.**
  - `docs/USER_GUIDE.md` → **The Plan Panel** / **What each step says**: what
    "not finished" and "Turn ended with N steps not marked done." mean; that the
    answer is still the answer; that a declined plan shows its steps as skipped.
  - `docs/TROUBLESHOOTING.md` → **Plans and step evidence**: the plan says a
    step is open after the turn ended (not stuck); how to read
    `plan_reminder_sent` and `plan_left_open`; the tagged
    `"kind":"plan_reminder"` message; why a declined plan is skipped.
- [ ] **Step 4: Close per AGENTS.md "When a spec becomes Done".**
  - `Status:` in `proposal.md`, the README implementation-order row, this
    file's Progress table and its `**Started:** … — **Done:** …` header.
  - `grep -rn "66_plan_left_open_at_turn_end" docs/ CHANGELOG.md`: no
    `Depends on:` line names #66, so nothing is promoted; update
    `docs/specs/README.md` → **What to build next** to remove #66 from the
    ready set and keep the pairing facts truthful.
  - Remove #66's line from `CHANGELOG.md`'s `Unreleased` (it is present today:
    "Stop showing a finished turn's plan as still running…").
  - Run `npm run specs:check -- --strict`.
- [ ] **Step 5: Checks.** Fast checks and scoped clippy/tests locally; the full
  seven-check gate runs on **CI**, not locally (AGENTS.md since 2026-10-15), and
  the user pushes. Record the CI result here when it is green, and say plainly
  that the workspace clippy and test suite were left to CI.
- [ ] **Step 6: Ask before committing.** Subject:
  `Close the plan-left-open spec with docs and acceptance results`.
