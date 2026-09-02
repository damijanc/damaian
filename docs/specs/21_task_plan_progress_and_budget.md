# Feature Spec: Task Plan, Progress, and Budget

Status: Not started
Order: 21 of 23
Roadmap: `docs/ROADMAP/02_phase_2_complete_task_workflow.md`, Phase 2, Work
Package 2 (Must). That directory is local-only and not committed, so the
reference is a name rather than a link; this spec is self-contained.
Related spec sections: `ai_coding_assistant_specification.md` section 7.1 (chat
interface UI states), section 7.6 (tool and action orchestrator), section 11
(error handling). Related implementation specs:
[`08_stop_and_progress.md`](08_stop_and_progress.md) (the per-turn progress and
cancellation this extends to multi-step tasks),
[`17_durable_task_state_and_crash_recovery.md`](17_durable_task_state_and_crash_recovery.md)
(the durable task state and append rules this persists through),
[`19_token_and_cost_accounting.md`](19_token_and_cost_accounting.md) (supplies
the token figures the ceiling is enforced against),
[`20_working_modes.md`](20_working_modes.md) (Plan mode produces plans it does
not execute).

## 1. Motivation

Damaian has no representation of work larger than a turn.

[Spec 08](08_stop_and_progress.md) made a turn stoppable and gave it distinct UI
states, and that was the right scope for the problem it solved. But a real task —
"add retry handling to the upload client and cover it with tests" — is a sequence
of steps whose intermediate state is currently invisible and unrecoverable. The
user sees a spinner and a stream of tool calls. If it goes wrong at step four,
there is nothing that says steps one through three happened, and nothing to
resume.

Two consequences follow, and the second is the more serious.

**Work is unbounded in the dimension that costs money.**
`agent_max_tool_rounds` bounds the loop by round count
(`crates/workspace-engine/src/config.rs:67`), which is a poor proxy: one round
that resends a 100k-token context costs more than twenty rounds that read three
small files. [Spec 19](19_token_and_cost_accounting.md) makes spend visible;
without a ceiling, visible is all it is.

**Completion is asserted, not demonstrated.** The model says it is done, and
Damaian relays that. There is no distinction between "the test passed" and "the
model believes the test would pass". That distinction is the whole value of the
completion report, and it cannot exist without steps that carry evidence.

## 2. Current State

- **No plan or progress model exists.** [Spec 08](08_stop_and_progress.md)
  delivered turn-level cancellation and UI states. There is no multi-step
  representation, no step status, and nothing to recover after restart beyond
  `TaskStatus`.
- **`TaskStatus` is per task, not per step.** Seven variants today
  (`crates/workspace-engine/src/session.rs:21-29`), extended to twelve by
  [spec 17](17_durable_task_state_and_crash_recovery.md).
- **A budget-stop shape already exists and works.** `agent_max_tool_rounds` is
  enforced in the chat loop, producing `tool_budget_exhausted_response`
  (`crates/workspace-engine/src/chat.rs:841`) and
  `TaskStatus::ToolBudgetExhausted` (`chat.rs:1205-1206`, `chat.rs:1227`). This
  is the pattern the token ceiling should follow rather than invent.
- **Related bounds**: `agent_tool_retry_limit` (`config.rs:73`) and
  `agent_web_debug_max_tool_rounds` (`config.rs:70`), both overlayable from
  repository config (`config.rs:308-315`).
- **Sessions are an append-only event log** with replay-based readers
  (`session.rs:237-260`), gaining a monotonic `seq` in
  [spec 17](17_durable_task_state_and_crash_recovery.md) §5.2.
- **No token accounting exists yet.** `ModelRun`
  (`crates/workspace-engine/src/model.rs:158-177`) has no usage fields;
  [spec 19](19_token_and_cost_accounting.md) adds them and
  `read_task_usage`.
- **Evidence sources already exist, unstructured.** `CommandExecution` carries
  `exit_code: Option<i32>`, `stdout`, `stderr`
  (`crates/workspace-engine/src/command_runner.rs:11-22`). `ProposedFilePatch`
  carries `base_hash` and the patch engine computes applied hashes
  (`patch_engine.rs:291`, `patch_engine.rs:79`).

## 3. Requirements

1. Non-trivial work is represented as ordered steps, each carrying a stable step
   ID, title, optional detail, status of `pending` / `in_progress` /
   `completed` / `blocked` / `skipped`, dependencies, start and completion time,
   and evidence or output references.
2. Only one primary step is `in_progress` unless parallel execution is explicit.
3. The current phase is shown: understanding, planning, editing, validating,
   reviewing, or complete.
4. Users can inspect and adjust a plan before implementation begins.
5. Progress persists and is recovered after restart, through the durable task
   state from [spec 17](17_durable_task_state_and_crash_recovery.md).
6. **A step is never marked complete only because the model says so.** Observable
   evidence is attached wherever one exists. Where none exists, the step is
   marked completed *unverified* and the completion report says so.
7. An enforced per-task token ceiling exists alongside `agent_max_tool_rounds`,
   using the accounting from [spec 19](19_token_and_cost_accounting.md). On
   reaching it, work stops cleanly at a step boundary, the plan is persisted, and
   the remaining steps are reported.

## 4. Non-goals

- Automatic plan generation quality. This spec defines the plan's structure,
  persistence, evidence rules, and budget behaviour. How good the model's plans
  are is measured by [spec 18](18_local_evaluation_harness.md), not fixed here.
- Parallel step execution. Requirement 2 leaves room for it; nothing in this spec
  runs steps concurrently. Subagents are Phase 6.
- A dependency solver. Dependencies are recorded and used to block a step whose
  prerequisite failed, not to reorder or optimise a plan.
- Cost ceilings in currency. The ceiling is in tokens, because tokens are what
  [spec 19](19_token_and_cost_accounting.md) can measure rather than estimate
  from user-supplied rates.
- Cross-task or per-session budgets. The ceiling is per task.
- Replacing `agent_max_tool_rounds`. Both bounds apply; whichever is reached
  first stops the task.
- A live per-turn token counter in the UI beyond what the plan panel shows.
- Editing a plan mid-execution. Requirement 4 is explicit that adjustment happens
  *before* implementation begins; §5.5 explains why.

## 5. Design

### 5.1 The plan

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StepStatus { Pending, InProgress, Completed, Blocked, Skipped }

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlanStep {
    pub id: String,
    pub title: String,
    pub detail: Option<String>,
    pub status: StepStatus,
    pub depends_on: Vec<String>,
    pub started_at_ms: Option<u128>,
    pub completed_at_ms: Option<u128>,
    pub evidence: Vec<Evidence>,
}
```

`Completed` deliberately has no `verified` boolean. Verification is a property of
the evidence a step carries, not a separate claim that could disagree with it —
§5.3.

A plan belongs to a task. `TaskPhase` (understanding, planning, editing,
validating, reviewing, complete) is recorded on the task and is derived from
step progress rather than set independently, so the phase cannot say "validating"
while every validation step is `pending`.

**When a plan is created**: a turn gets a plan when it is non-trivial, defined
mechanically as a turn that will propose a patch, run a mutating command, or has
more than one step in the model's own proposal. A single question, a single file
read, or a one-command turn gets no plan, because a one-step plan is ceremony —
[spec 08](08_stop_and_progress.md)'s turn states already cover it.

### 5.2 Persistence: appended, replayed

Following [spec 17](17_durable_task_state_and_crash_recovery.md) §5.2, plan state
is appended to the session log, never rewritten:

```json
{"seq":214,"eventType":"plan_created","taskId":"task_…","steps":[…]}
{"seq":231,"eventType":"plan_step_updated","taskId":"task_…","stepId":"step_…",
 "status":"in_progress","startedAtMs":…}
{"seq":248,"eventType":"plan_step_updated","taskId":"task_…","stepId":"step_…",
 "status":"completed","completedAtMs":…,
 "evidence":[{"kind":"commandExit","ref":"cmd_…","exitCode":0}]}
```

`SessionStore` gains `read_task_plan(session_id, task_id) -> Option<TaskPlan>`,
replaying `plan_created` and folding `plan_step_updated` events in `seq` order.
The newest event per step wins, the same rule `read_task_statuses`
(`session.rs:237`) already uses for task status.

This satisfies requirement 5 without a second store, per the roadmap's
instruction to extend the durable task state rather than add a parallel one. It
also means a crash mid-step loses nothing already recorded: the step's last
persisted status is its status, and
[spec 17](17_durable_task_state_and_crash_recovery.md)'s dangling
`action_started` marker says what was in flight inside it.

### 5.3 Evidence, and what "completed" is allowed to mean

Requirement 6 is the requirement most likely to be satisfied in letter and
violated in spirit, so the rule is mechanical: **a step's status is a function of
its evidence, and the model does not write it.**

```rust
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum Evidence {
    /// A command ran and exited. Carries the execution id and exit code.
    CommandExit { r#ref: String, exit_code: Option<i32> },
    /// A patch was applied. Carries patch id and resulting file hashes.
    PatchApplied { r#ref: String, files: Vec<(String, String)> },
    /// A check produced findings. Carries the finding ids.
    Findings { refs: Vec<String>, failing: usize },
    /// A file was read and its hash recorded at read time.
    FileRead { path: String, hash: String },
}
```

Every variant references something Damaian observed itself. There is no
`ModelAsserted` variant, and adding one would defeat the requirement.

The status rule:

| Evidence present | Status |
|---|---|
| `CommandExit` with `exit_code: Some(0)` | `completed` |
| `CommandExit` with a non-zero code | `blocked`, and the failure becomes a finding ([spec 22](22_findings_model_and_panel.md)) |
| `CommandExit` with `exit_code: None` | **not** `completed`. The command did not report an exit status, so nothing is known. `blocked` |
| `PatchApplied` with hashes matching what was written | `completed` |
| No evidence of any kind | `completed_unverified` in the report; `completed` in the plan, with an empty `evidence` vec |

`exit_code: None` is called out because `CommandExecution.exit_code` is
`Option<i32>` (`command_runner.rs:19`) — a killed or signalled command has no
code, and mapping absence to success is precisely how "never represent an unrun
check as passed" gets violated by a `unwrap_or(0)`.

An empty `evidence` vec is how requirement 6's unverified case is represented: the
step is done as far as the plan is concerned, and the completion report states
that nothing observable confirms it. Steps like "understand the existing retry
logic" legitimately have no evidence, and forcing a fake one would be worse than
admitting it.

### 5.4 The token ceiling

`agent_max_task_tokens` in `Config`, overlayable from repository config the same
way `agent_max_tool_rounds` is (`config.rs:308-315`). Default: unset, meaning no
ceiling, so this cannot break an existing configuration on upgrade.

Enforcement follows the existing `tool_budget_exhausted` pattern exactly
(`chat.rs:841`, `chat.rs:1205`):

- Checked **at step boundaries and between tool rounds**, not mid-stream. A
  ceiling that interrupts a model mid-response wastes the tokens already spent on
  that response, which is the opposite of the point.
- The check reads `read_task_usage` from
  [spec 19](19_token_and_cost_accounting.md). A task whose usage is
  `Estimated` is still checked against the ceiling — an estimated total is the
  best available number, and declining to enforce on it would make the ceiling
  inoperative for every provider that does not report usage.
- On reaching it: the current step is left at its persisted status, the plan is
  persisted, the task takes a terminal status, and the report names the steps that
  remain and the usage that was consumed.

A new `TaskStatus::TokenBudgetExhausted` sits beside the existing
`ToolBudgetExhausted` rather than reusing it. They are different facts with
different remedies — one means the work needed more rounds, the other that it
needed more money — and collapsing them would make the eval harness unable to
distinguish them.

The stop is recoverable: the plan survives, so the user can raise the ceiling and
resume, and the remaining steps are what resumption starts from.

### 5.5 Plan review before implementation

Requirement 4: after a plan is created and before any mutating step runs, the
plan is presented and the user may reorder, edit titles, delete steps, or
approve. This is a gate in Code mode and the natural terminus in Plan mode
([spec 20](20_working_modes.md)) — Plan mode produces the plan and stops, and
switching to Code carries it over intact.

Mid-execution editing is a non-goal, and the reason is worth recording: a step
already `completed` has evidence attached to a state of the repository, and
allowing the plan to be rewritten underneath that evidence produces a plan whose
history no longer describes what happened. A user who wants a different plan
mid-task should stop the task ([spec 08](08_stop_and_progress.md)) and start
another, with the checkpoint from
[spec 16](16_session_checkpoints_and_rewind.md) available to rewind first.

Adjustments are recorded as a `plan_revised` event carrying the new step list, so
the original plan and the user's revision are both in the log.

### 5.6 UI

A plan panel showing steps with status, the derived phase, and per-step evidence
where present. The current step is distinguished, and requirement 2 is visible: a
plan with two `in_progress` steps would be a bug the panel makes obvious.

The completion report distinguishes four outcomes per step — verified complete,
completed unverified, blocked, skipped — and the summary line never says
"complete" for a task with a blocked step.

### 5.7 Documentation

`docs/USER_GUIDE.md`: what a plan is, when one appears, how to adjust it, what
"completed unverified" means and why Damaian says it, and how to set a token
ceiling. `docs/TROUBLESHOOTING.md`: where plan events are in the session log, how
to read step evidence, and the difference between the two budget-exhausted
statuses.

## 6. Acceptance Criteria

- A multi-step task shows a plan before it starts editing, and a trivial turn
  produces no plan.
- The plan survives a restart mid-task with step statuses and evidence intact.
- Only one step is `in_progress` at a time — asserted by test.
- A step whose command exited non-zero is `blocked`, not `completed`.
- A step whose command has `exit_code: None` is not `completed` — asserted
  directly, since this is the `unwrap_or(0)` failure mode.
- A step with no observable evidence is reported as completed unverified, and the
  completion report says so.
- No `Evidence` variant can be produced from model output alone — asserted by the
  absence of such a constructor and a test that model-asserted completion does
  not mark a step complete.
- Reaching `agent_max_task_tokens` stops the task at a step boundary with a
  persisted plan, reports the remaining steps and the usage consumed, and sets
  `TokenBudgetExhausted` distinctly from `ToolBudgetExhausted`.
- A ceiling is enforced against an estimated usage total as well as a measured
  one.
- An unset ceiling imposes no limit, so existing configurations are unaffected.
- A plan revised by the user records both the original and the revision.
- The task phase is derived from step state and cannot contradict it.
- The five quality-gate commands from `AGENTS.md` pass, and the end-to-end
  fixture from [spec 23](23_verification_loop.md) exercises a plan through to a
  completion report.

## 7. Implementation Notes

To be completed during implementation. Record:

- The mechanical rule actually used to decide a turn is non-trivial, and how
  often it produced a plan for a turn that did not need one during testing.
- The default `agent_max_task_tokens` if one is chosen later, and the reasoning.
- Whether any step type in practice ends up with no available evidence, since a
  large share of unverified steps would mean the evidence model is missing a
  source rather than the work being genuinely unobservable.
