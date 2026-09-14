# Feature Spec: Task Plan, Progress, and Budget

Status: Done. Planning read this design against the tree on 2026-09-11 and
found eight statements the code contradicts — including that a "task" is one
turn, that the round-budget pattern §5.4 says to copy would make the ceiling
spend *more* than the ceiling, and that the session log has no failure outcome
for evidence to be read from. All eight are reconciled in
[`context.md`](context.md) §3 and were carried through
[`tasks.md`](tasks.md)'s fourteen tasks.

Two questions §7 asked turned out to have answers worth reading before the
design: there is **no mechanical triviality rule** and there cannot be one, and
one step type had no evidence because a source the enum already allowed for was
never recorded. Both are in §7.
Order: 21 of 23
Roadmap: `docs/ROADMAP/02_phase_2_complete_task_workflow.md`, Phase 2, Work
Package 2 (Must). That directory is local-only and not committed, so the
reference is a name rather than a link; this spec is self-contained.
Also in this spec: [`context.md`](context.md) (motivation, current state, and
the corrections found while planning), [`tasks.md`](tasks.md) (execution order
and progress).
Related spec sections: `ai_coding_assistant_specification.md` section 7.1 (chat
interface UI states), section 7.6 (tool and action orchestrator), section 11
(error handling). Related implementation specs:
[`08_stop_and_progress.md`](../08_stop_and_progress.md) (the per-turn progress and
cancellation this extends to multi-step tasks),
[`17_durable_task_state_and_crash_recovery/proposal.md`](../17_durable_task_state_and_crash_recovery/proposal.md)
(the durable task state and append rules this persists through),
[`19_token_and_cost_accounting/proposal.md`](../19_token_and_cost_accounting/proposal.md) (supplies
the token figures the ceiling is enforced against),
[`20_working_modes.md`](../20_working_modes.md) (Plan mode produces plans it does
not execute).

Motivation and current state moved to [`context.md`](context.md) when this spec
took the folder layout. **Read its §3 before implementing:** eight of this
document's statements assume behaviour the code does not have, and that section
says what each one has to become. The three that change this design rather than
just its facts are §3.1 (a task is one turn), §3.3 (the round-budget pattern
cannot be copied) and §3.4 (no failure outcome exists to read evidence from).

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
   state from [spec 17](../17_durable_task_state_and_crash_recovery/proposal.md).
6. **A step is never marked complete only because the model says so.** Observable
   evidence is attached wherever one exists. Where none exists, the step is
   marked completed *unverified* and the completion report says so.
7. An enforced per-task token ceiling exists alongside `agent_max_tool_rounds`,
   using the accounting from [spec 19](../19_token_and_cost_accounting/proposal.md). On
   reaching it, work stops cleanly at a step boundary, the plan is persisted, and
   the remaining steps are reported.

## 4. Non-goals

- Automatic plan generation quality. This spec defines the plan's structure,
  persistence, evidence rules, and budget behaviour. How good the model's plans
  are is measured by [spec 18](../18_local_evaluation_harness/proposal.md), not
  fixed here.
- Parallel step execution. Requirement 2 leaves room for it; nothing in this spec
  runs steps concurrently. Subagents are Phase 6.
- A dependency solver. Dependencies are recorded and used to block a step whose
  prerequisite failed, not to reorder or optimise a plan.
- Cost ceilings in currency. The ceiling is in tokens, because tokens are what
  [spec 19](../19_token_and_cost_accounting/proposal.md) can measure rather than estimate
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

> `TaskPhase` is a **new type**, not an extension of the existing `PhaseKind`
> ([`context.md`](context.md) §3.7). `PhaseKind` says which stage of the agent
> loop is executing and drives the spinner; `TaskPhase` says what the work is
> about. They are orthogonal — a `Model` phase occurs during all six — and
> merging them yields a type whose variants are not mutually exclusive.
>
> §3.1: a task is one turn, so a plan's steps are steps of the agent loop,
> bounded by `agent_max_tool_rounds` (default 8), not stages of a multi-turn
> project.

**When a plan is created**: a turn gets a plan when it is non-trivial, defined
mechanically as a turn that will propose a patch, run a mutating command, or has
more than one step in the model's own proposal. A single question, a single file
read, or a one-command turn gets no plan, because a one-step plan is ceremony —
[spec 08](../08_stop_and_progress.md)'s turn states already cover it.

### 5.2 Persistence: appended, replayed

Following [spec 17](../17_durable_task_state_and_crash_recovery/proposal.md) §5.2, plan state
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
[spec 17](../17_durable_task_state_and_crash_recovery/proposal.md)'s dangling
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

> Two corrections to the sketch above, from [`context.md`](context.md) §3.5 and
> §3.6. The `ref` is a `markerId` from spec 17, not a `cmd_…` id: execution ids
> never reach the session log and the audit log that holds them expires, so the
> exit code is the evidence and the reference is only a breadcrumb. And
> `Findings` is deferred until [spec 22](../22_findings_model_and_panel.md)
> exists to produce the ids it holds; the enum is `#[non_exhaustive]` so adding
> it later breaks nothing.

The status rule:

| Evidence present | Status |
|---|---|
| `CommandExit` with `exit_code: Some(0)` | `completed` |
| `CommandExit` with a non-zero code | `blocked`, and the failure becomes a finding ([spec 22](../22_findings_model_and_panel.md)) |
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

`agent_max_task_tokens` in `Config`. Default: unset, meaning no ceiling, so this
cannot break an existing configuration on upgrade.

> Corrected by [`context.md`](context.md) §3.8: **not** overlayable the same way
> `agent_max_tool_rounds` is. That key is in the "Free" block, which a
> repository may set freely. A repository that raises a ceiling the user set low
> spends the user's money, so the ceiling is restrict-only — a repository may
> lower it or set one where none existed, never raise one and never remove one.

Enforcement reuses the `tool_budget_exhausted` *plumbing* — a flag carried out
of the loop, a distinct `TaskStatus`, a distinct audit status — but **not its
control flow**. [`context.md`](context.md) §3.3: `force_final` does not stop the
loop, it makes one more model call with tools dropped, and on a context that has
grown all turn that is the most expensive call of the turn. A ceiling whose
enforcement action is to spend more than the ceiling is not a ceiling, so the
token check stops before the call rather than forcing a final one.

- Checked **at step boundaries and between tool rounds**, not mid-stream. A
  ceiling that interrupts a model mid-response wastes the tokens already spent on
  that response, which is the opposite of the point.
- The check reads `read_task_usage` from
  [spec 19](../19_token_and_cost_accounting/proposal.md). A task whose usage is
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

> [`context.md`](context.md) §3.1 makes this concrete. A task is one turn, so a
> resumed turn is a *new* task with a new id, and `read_task_plan` keyed on task
> id would not find the plan that was persisted. Resumption carries the plan
> forward explicitly through a `plan_resumed` event naming the task it came
> from; without that mechanism "the plan survives" is true of the log and false
> of the user.

### 5.5 Plan review before implementation

Requirement 4: after a plan is created and before any mutating step runs, the
plan is presented and the user may reorder, edit titles, delete steps, or
approve. This is a gate in Code mode and the natural terminus in Plan mode
([spec 20](../20_working_modes.md)) — Plan mode produces the plan and stops, and
switching to Code carries it over intact.

Mid-execution editing is a non-goal, and the reason is worth recording: a step
already `completed` has evidence attached to a state of the repository, and
allowing the plan to be rewritten underneath that evidence produces a plan whose
history no longer describes what happened. A user who wants a different plan
mid-task should stop the task ([spec 08](../08_stop_and_progress.md)) and start
another, with the checkpoint from
[spec 16](../16_session_checkpoints_and_rewind.md) available to rewind first.

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
- A non-zero exit reaches the session log at all — asserted directly, because
  today every tool arm records `"ok"` regardless of outcome
  ([`context.md`](context.md) §3.4) and every evidence criterion above is
  vacuous until it does.
- A resumed turn recovers the plan of the task it resumes, not an empty one
  (both ways a turn hands work over: a ceiling stop, and a crash resume from
  [spec 45](../45_crash_recovery_prompt.md) §5.8, which was added after this
  spec landed — until then the crash case silently re-planned from scratch)
  (§3.1).
- A repository config may lower `agent_max_task_tokens` and may not raise it
  (§3.8).
- `update_task_status` stamps `completed_at_ms` for `TokenBudgetExhausted` —
  asserted directly, since its terminal list is hand-written and no existing
  test covers an omission (§3.2).
- Every quality-gate command from `AGENTS.md` passes, and a plan runs end to end
  through to a completion report in
  [spec 18](../18_local_evaluation_harness/proposal.md)'s harness. **Restated
  from spec 23**, which is Not started and cannot supply a fixture; §3.6. The
  harness is Done, runs deterministically in CI, and has been exercised against
  a live provider, which is the stronger gate of the two.

## 7. Implementation Notes

### There is no mechanical triviality rule, and that is the answer

This section asked for "the mechanical rule actually used to decide a turn is
non-trivial". None exists, and the reason is worth recording rather than
papering over: **the engine cannot make that judgement before the work starts.**
Anything it could measure up front — prompt length, keyword shape, file count
— would be a guess about work nobody has begun. §5.1 also requires the plan to
exist *before* the first mutating step, so a rule derived from what the turn
went on to do arrives too late to be a plan at all.

So the decision belongs to the model, expressed as a tool it may call. The only
guidance is the `propose_plan` description:

> Propose an ordered plan for non-trivial work, before starting it. Use this
> when the task needs more than one step — a single question or a single file
> read needs no plan.

Two structural guards back it up. The schema sets `minItems: 2`, so a one-step
plan cannot be expressed; and a turn may propose a plan only once — a second
call is refused rather than replacing the first, because steps already carry
evidence tied to a state of the repository.

**How often it over-fires is not yet measured, and cannot be measured by CI.**
The deterministic tier scripts every tool call, so the model is not choosing
there: `planned_task` produces a plan because the scenario file says to. The
question needs the live tier and a corpus of real prompts. Until then the
honest position is that this is unquantified — recording an unmeasured number
would be worse than recording none.

### No default `agent_max_task_tokens` was chosen

The setting is `Option<u64>` and defaults to `None`. No default was picked
because no honest one exists: the right ceiling depends on the user's provider
rates, repository size, and what they consider a request worth. A number low
enough to protect a careless user would truncate legitimate large refactors on
a big repository, and one high enough never to interfere would not protect
anyone.

Runaway loops are already bounded by `agent_max_tool_rounds`, which defaults to
8. That is the safety net; this ceiling is a *budget*, and a budget with a
default nobody chose is a budget nobody owns. The minimum accepted value is
1000 — below that a turn would stop before doing anything, which is
indistinguishable from Damaian being broken, so a typo is refused at parse time
rather than surfacing later as a hang.

### One step type had no evidence, and the evidence was there all along

This section anticipated that a large share of unverified steps would mean "the
evidence model is missing a source rather than the work being genuinely
unobservable". That is exactly what happened, and it was found by asking this
question rather than by a failing test.

`Evidence::FileRead` existed in the enum from Task 2. `status_from_evidence`
handled it and `TaskPhase` mapped it to `Understanding` — but **nothing ever
constructed one**. `evidence_for` covered `CommandExit` and `PatchApplied` and
returned `None` for everything else, so a step whose work was "read the retry
helper" reported *completed unverified* even though the engine had the path and
the content hash in hand at the moment of the read. Now fixed:
`ActionOutcome::FileRead` carries both out of the arm, and a failed read still
mints nothing — a step must not be confirmed by the fact that Damaian *tried*
to look at something.

It also makes `TaskPhase::Understanding` a derived answer rather than only the
`None` fallback.

**What still mints no evidence,** deliberately or otherwise:

| Tool | Evidence | Why |
|------|----------|-----|
| `run_command` | `CommandExit` | — |
| patch apply | `PatchApplied` | recorded from `edit.rs`, after the proposing turn ends |
| `read_file` | `FileRead` | — |
| `search_codebase` | none | a result list is a claim about relevance, not an observation about the repository |
| `read_git_status`, `read_git_diff` | none | worth revisiting: both observe real state and could carry a hash |
| `mcp_call`, `web_diagnostic` | none | the outcome is a remote system's, and §4 rules out probing to find out what it did |
| spec 22 findings | `Evidence::Findings` | **not implemented** — spec 22 does not exist to produce the ids it would hold |

`Evidence` is `#[non_exhaustive]` and `status_from_evidence`'s match is
exhaustive in-crate, so adding a variant is a compile error at the one place
that must decide what it means. That is the intended behaviour, not an
oversight.

The one measured figure, from `planned_task` in CI: 2 steps planned, 1 verified,
1 completed unverified. The unverified one is a summarising step with genuinely
nothing to observe, which is the honest half of the distinction.

### How the phase is derived

`TaskPhase` is computed from the plan rather than tracked, so it cannot
contradict the steps it describes. In order:

1. No steps at all → `Planning`.
2. Every step `Completed` or `Skipped` → `Complete`.
3. `awaiting_review` → `Reviewing`.
4. Otherwise, the **newest** piece of evidence across all steps:
   `CommandExit` → `Validating`, `PatchApplied` → `Editing`, `FileRead` or no
   evidence at all → `Understanding`.

`awaiting_review` is a parameter rather than something the plan knows. Whether a
patch is sitting in front of a human is a fact about the task, not about the
step list, and guessing it from the steps would have been a fabrication — the
alternative was dropping the variant, which would have made the phase lie by
omission during the one state a user most wants named.

**`Planning` is nearly unreachable in practice.** A plan is created with its
steps already in it, so a zero-step plan only exists between `TaskPlan::new`
and the first push — a window inside one function that never reaches the log.
It is kept because it is the honest answer for a plan with no steps, and
because a `phase()` that had to assume non-empty input would be a precondition
nothing enforces.

### The ceiling is checked before the call, not after it

§5.4 said to follow the `tool_budget_exhausted` pattern "exactly". That pattern
detects exhaustion *after* a model call and then makes one more with tools
dropped, which for a token ceiling would spend past the ceiling to discover it
had been crossed — see [`context.md`](context.md) §3.3. The check therefore sits
at the top of the loop, before the call: a turn stops at the last round whose
spending was under the ceiling, never after the one that crossed it.
