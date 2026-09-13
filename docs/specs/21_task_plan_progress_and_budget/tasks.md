# Task Plan, Progress, and Budget Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Implements:** [`proposal.md`](proposal.md) · background and corrections in [`context.md`](context.md)
**Started:** not yet

**Goal:** Give a turn an ordered plan whose steps are marked complete by
evidence the engine observed rather than by the model's say-so, persist it in
the append-only session log so a restart recovers it, and enforce a token
ceiling that stops the turn *before* the call that would cross it.

**Architecture:** `TaskPlan` and `PlanStep` live in a new
`workspace-engine/src/plan.rs`, persisted as `plan_created` /
`plan_step_updated` / `plan_revised` / `plan_resumed` events and replayed by
`SessionStore::read_task_plan` the same way `read_task_statuses` replays status.
Evidence is minted only inside the chat orchestrator's tool arms, from values
the engine holds at the call site, and keys on spec 17's `markerId`. The ceiling
reads spec 19's `read_task_usage` and is checked at the top of the agent loop,
before the request is built.

**Tech Stack:** Rust 2024 (workspace edition), no new dependencies. `serde` /
`serde_json` for the event payloads; the SSE and `app.js` plumbing already
exists for spec 08's phase events and is extended, not replaced.

## Global Constraints

Every task's requirements implicitly include this section.

- **Read [`context.md`](context.md) §3 first.** Eight of the proposal's
  statements describe behaviour the code does not have. Each task below already
  accounts for its correction; do not "fix" a task back to the proposal's
  original wording.
- **A task is one turn** (§3.1). Everywhere the proposal says "per task", read
  "per turn". Do not introduce a second task-like object to make the wording
  true.
- **The model never writes a step status and never mints evidence** (requirement
  6). Every `Evidence` value is constructed from a variable the engine holds,
  in an arm of the orchestrator, never from parsed model output. There is no
  `ModelAsserted` variant and no `Evidence` constructor reachable from a tool
  argument.
- **`exit_code: None` is not success.** A `unwrap_or(0)` anywhere near an exit
  code is a defect. Absence means the command was killed or signalled and
  nothing is known.
- **The session log is append-only.** Spec 17's rule. Plan state is appended,
  never rewritten, and never back-patched onto an earlier event.
- **The ceiling must never cause a call it was meant to prevent** (§3.3). The
  check runs before the request is built, not after the response arrives.
- **No prompt text, repository content, or API key in any plan event.** Step
  titles are model-authored text and are redacted through `SecretScanner` before
  they are written, the same as any other model output.
- **Clippy warnings are errors.** Fix rather than suppress; an `#[allow(...)]`
  needs a comment saying why.
- **Every quality-gate command from `AGENTS.md` must pass** at the end of every
  task. That is seven commands and the list in `AGENTS.md` is authoritative —
  read it, do not rely on a remembered list. `typos` and
  `node --check crates/desktop-shell/static/app.js` are the two that get missed.
- **Commit messages:** one subject line, no body, no `Co-Authored-By`. Rationale
  belongs in this plan and the proposal. Never cite commit SHAs in
  documentation.
- **Never commit without asking.** Finish the task, run the gate, show the
  change, then ask.
- **Never switch branches.** Other processes write to this tree concurrently.

## Progress

| Task | State | Notes |
|---|---|---|
| 1. Tool outcomes tell success from failure | **done** | Also repairs #18's `tool_and_model_error_rate`; see the note below |
| 2. `TaskPlan` and `PlanStep` types | **done** | `Evidence` is `#[non_exhaustive]`; see the note below |
| 3. Plan persistence and replay | **done** | `revise_plan` landed here rather than in Task 11; see the note below |
| 4. Evidence, minted at the call site | **done** | Construction only; attachment moved to 4b — see the note below |
| **4b. The plan lifecycle in a turn** | **done** | Two new tools; `PatchApplied` attachment blocked on Task 10 — see the note below |
| 5. Step status is a function of evidence | **done** | Done before 4b, which depends on it |
| 6. `TaskPhase`, derived | **done** | `awaiting_review` is an argument, not a guess — see the note below |
| 7. `TokenBudgetExhausted` status | **done** | Three guards caught the change; the UI still has no label — see the note below |
| 8. `agent_max_task_tokens` config, restrict-only | **done** | Restrict-only, diverging from the three existing `agent_*` bounds — see the note below |
| 9. Ceiling enforcement in the loop | **done** | A weak assertion here survived the mutation — see the note below |
| 10. Plan carry-over on resume | **done** | Unblocks 4b's patch evidence and 6's `Editing` phase — see the note below |
| 11. Plan review gate | **done** | "Mutating" is not decidable from the action alone — see the note below |
| 12. Plan panel and completion report | **done** | Closed Task 7's and Task 11's debts; found a pre-existing duplicate-stop-row bug — see the note below |
| 13. Harness coverage and documentation | not started | |

## File Structure

- `crates/workspace-engine/src/plan.rs` — **new.** `StepStatus`, `PlanStep`,
  `TaskPlan`, `Evidence`, `TaskPhase`, and the pure functions that decide a
  step's status from its evidence and a plan's phase from its steps. Pure and
  dependency-free so the rules are testable without a session, a model, or a
  filesystem.
- `crates/workspace-engine/src/session.rs` — plan event writers and
  `read_task_plan`; `TaskStatus::TokenBudgetExhausted`.
- `crates/workspace-engine/src/chat.rs` — outcome-aware `finish_action` calls,
  evidence construction in the tool arms, the ceiling check, the plan gate.
- `crates/workspace-engine/src/config.rs` — `agent_max_task_tokens`,
  restrict-only.
- `crates/desktop-shell/src/lib.rs` + `static/app.js` — the plan panel and the
  `plan` SSE event.
- `crates/eval-harness/src/` — plan metrics read from the log.

---

### Task 1: Tool outcomes tell success from failure

The prerequisite. Until the log can say a command failed, every evidence rule
below is built on a value that is always `"ok"` ([`context.md`](context.md)
§3.4). This task is worth landing on its own even if the rest slips: it also
fixes spec 18's `tool_and_model_error_rate`, which reads 0.000 by construction.

**Files:**
- Modify: `crates/workspace-engine/src/chat.rs` (the tool arms and the single
  `finish_action(action_marker, "ok")` they converge on)
- Test: `crates/workspace-engine/tests/crash_recovery.rs`

**Interfaces:**
- Consumes: `SessionStore::finish_action(marker, outcome)`, unchanged.
- Produces: `action_finished` events whose `outcome` is one of `ok`, `failed`,
  `conflict`, `awaiting_approval`, `awaiting_review`, and, for a command, an
  `exitCode` field on the event. Tasks 4 and 5 read both.

- [ ] **Step 1: Write the failing test**

In `crates/workspace-engine/tests/crash_recovery.rs`:

```rust
#[test]
fn a_command_that_exits_non_zero_is_not_recorded_as_ok() {
    let fixture = Fixture::new();
    let task = fixture
        .store
        .create_task(&fixture.session_id, "run the failing check", "mock", "m")
        .unwrap();
    let marker = fixture
        .store
        .start_action(&task, "run_command", "false", true)
        .unwrap();
    fixture
        .store
        .finish_command_action(marker, Some(1))
        .unwrap();

    let outcomes = read_action_outcomes(&fixture.store, &fixture.session_id);
    assert_eq!(outcomes, vec![("failed".to_string(), Some(1))]);
}

#[test]
fn a_command_killed_without_an_exit_code_is_not_recorded_as_ok() {
    let fixture = Fixture::new();
    let task = fixture
        .store
        .create_task(&fixture.session_id, "run the killed check", "mock", "m")
        .unwrap();
    let marker = fixture
        .store
        .start_action(&task, "run_command", "sleep 100", true)
        .unwrap();
    fixture.store.finish_command_action(marker, None).unwrap();

    // Not `ok`: absence of an exit code is absence of knowledge, and this is
    // the `unwrap_or(0)` failure mode the spec calls out by name.
    let outcomes = read_action_outcomes(&fixture.store, &fixture.session_id);
    assert_eq!(outcomes, vec![("unknown".to_string(), None)]);
}
```

Write `read_action_outcomes` as a test helper in the same file that reads the
session log and returns `(outcome, exitCode)` per `action_finished` event, in
log order.

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p workspace-engine --test crash_recovery a_command_ -- --nocapture`
Expected: FAIL, `no method named finish_command_action`.

- [ ] **Step 3: Add `finish_command_action` to `SessionStore`**

In `crates/workspace-engine/src/session.rs`, beside `finish_action`:

```rust
/// [`Self::finish_action`] for a command, recording the exit code alongside
/// the outcome.
///
/// Split from `finish_action` rather than folded into it because the outcome
/// is *derived* from the exit code and must not be passed in separately —
/// two callers could then disagree, and the disagreement would be silent.
///
/// `None` maps to `"unknown"`, never to `"ok"`. A killed or signalled command
/// reports no code (`command_runner.rs`, `output.status.code()`), and mapping
/// absence to success is how "never represent an unrun check as passed" gets
/// violated. Spec 21 §5.3.
pub fn finish_command_action(
    &self,
    marker: ActionMarker,
    exit_code: Option<i32>,
) -> Result<()> {
    let outcome = match exit_code {
        Some(0) => "ok",
        Some(_) => "failed",
        None => "unknown",
    };
    let code = match exit_code {
        Some(code) => format!(",\"exitCode\":{code}"),
        None => String::new(),
    };
    self.append_session_event(
        &marker.session_id,
        "action_finished",
        &format!(
            "{{\"markerId\":\"{}\",\"taskId\":\"{}\",\"action\":\"{}\",\"ref\":\"{}\",\"outcome\":\"{}\"{}}}",
            escape_json(&marker.id),
            escape_json(&marker.task_id),
            escape_json(&marker.action),
            escape_json(&marker.reference),
            outcome,
            code
        ),
    )
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p workspace-engine --test crash_recovery a_command_`
Expected: PASS.

- [ ] **Step 5: Mutation-test the `None` arm**

Change `None => "unknown"` to `None => "ok"`. Run the tests. The second test
must fail. Restore the line. A test that does not fail here is not testing the
failure mode the spec names.

- [ ] **Step 6: Route the command arm through it**

In `crates/workspace-engine/src/chat.rs`, the `ToolAction::Command` arm holds
`record.execution` one statement before the shared `finish_action`. Carry the
exit code out of the arm and finish the marker there rather than at the shared
site. The shared `finish_action(action_marker, "ok")` stays for the arms whose
dispatch genuinely succeeded, but it no longer runs for a command.

The mechanical shape: make the `match tool_action` block produce
`(assistant_summary, tool_result_text, ActionOutcome)` where

```rust
/// What the arm observed, so the marker is finished with the tool's outcome
/// rather than the dispatch's.
enum ActionOutcome {
    /// The dispatch succeeded and the tool reported nothing to the contrary.
    Ok,
    /// A command ran; the code is whatever the process reported.
    CommandExit(Option<i32>),
    /// The tool itself reported a failure — an MCP `is_error`, a browser
    /// diagnostic that could not run. Not a command, so no exit code.
    Failed,
}
```

and finish the marker once, after the block, on that value. Set
`ActionOutcome::Failed` in the MCP arm when `result.is_error` or the call
returned `Err`, and in the web-diagnostic arm when `browser_tool_result_failed`.

- [ ] **Step 7: Assert the eval harness now sees a non-zero error rate**

In `crates/eval-harness/tests/harness.rs`, extend
`the_error_rate_counts_failures_and_not_outcomes_awaiting_a_human` with a case
whose scenario runs a failing command and assert the rate is above zero. Before
this task the metric could not move; the test that proves it now can is the
point.

- [ ] **Step 8: Run the full gate, then ask before committing**

Run all seven commands from `AGENTS.md`. Proposed message:
`Record what a tool reported, not just that it was dispatched`

#### What this task actually did

All seven gate commands pass. Three departures from the plan as written, all
widenings rather than narrowings:

- **`ActionOutcome::Failed` covers the read-only arms too**, not just MCP and
  the browser. A `read_file` on a restricted path, a `git status` that errored —
  both previously recorded `"ok"`. The text fed back to the model already said
  "Cannot read …", so the log was the only place the failure was invisible.
- **`finish_action` and `finish_command_action` share a private
  `write_action_finished`** rather than duplicating the event format. Two copies
  of that `format!` would drift, and the exit-code field has to be optional in
  exactly one of them.
- **A clean exit records `exitCode: 0` as well as `"ok"`.** The plan only
  required the failure cases to be distinguishable. Carrying the value on
  success too is what lets Task 4 build `Evidence::CommandExit` from the log
  alone rather than from a value it has to keep alive alongside it.

The mutation check ran as specified: `None => "ok"` fails exactly
`a_command_killed_without_an_exit_code_is_not_recorded_as_ok` and nothing else.

One observation for proposal §7. The engine's outcome vocabulary is now `ok`,
`failed`, `unknown`, `conflict`, `awaiting_approval`, `awaiting_review` — and
`is_tool_error` in the harness already classified an unrecognised outcome as an
error (fail-closed), so `"failed"` was counted correctly the moment it started
being written. That is the fail-closed default earning its keep rather than a
coincidence, and it is worth not "simplifying" away.

---

### Task 2: `TaskPlan` and `PlanStep` types

**Files:**
- Create: `crates/workspace-engine/src/plan.rs`
- Modify: `crates/workspace-engine/src/lib.rs` (module + re-exports)
- Test: `crates/workspace-engine/tests/plan.rs` (new)

**Interfaces:**
- Produces: `StepStatus`, `PlanStep`, `TaskPlan`, `Evidence` — every later task
  depends on these names and shapes.

- [ ] **Step 1: Write the failing test**

```rust
use workspace_engine::plan::{Evidence, PlanStep, StepStatus, TaskPlan};

#[test]
fn a_plan_round_trips_through_json_unchanged() {
    let plan = TaskPlan {
        task_id: "task_1".to_string(),
        created_at_ms: 1_700_000_000_000,
        steps: vec![PlanStep {
            id: "step_1".to_string(),
            title: "Add the retry helper".to_string(),
            detail: None,
            status: StepStatus::Completed,
            depends_on: Vec::new(),
            started_at_ms: Some(1),
            completed_at_ms: Some(2),
            evidence: vec![Evidence::CommandExit {
                marker_id: "action_1".to_string(),
                exit_code: Some(0),
            }],
        }],
    };
    let text = serde_json::to_string(&plan).unwrap();
    assert_eq!(serde_json::from_str::<TaskPlan>(&text).unwrap(), plan);
    // camelCase on the wire: the shell reads these events directly.
    assert!(text.contains("\"exitCode\":0"));
    assert!(text.contains("\"markerId\":\"action_1\""));
}

#[test]
fn only_one_step_may_be_in_progress() {
    let mut plan = TaskPlan::new("task_1", 0);
    plan.steps.push(step("step_1", StepStatus::InProgress));
    plan.steps.push(step("step_2", StepStatus::InProgress));
    assert!(plan.violates_single_in_progress());
}
```

Write `step(id, status)` as a test helper building a `PlanStep` with empty
everything else.

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test -p workspace-engine --test plan`
Expected: FAIL, `unresolved import workspace_engine::plan`.

- [ ] **Step 3: Write `plan.rs`**

```rust
//! The plan a turn works through, and the evidence that says a step is done.
//!
//! Pure: no session, no model, no filesystem. The rules in here are the
//! substance of requirement 6, and they are worth testing without a turn
//! around them.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StepStatus {
    Pending,
    InProgress,
    Completed,
    Blocked,
    Skipped,
}

/// Something Damaian observed itself.
///
/// There is deliberately no `ModelAsserted` variant: requirement 6 exists
/// because the model's claim is exactly what must not count, and a variant
/// carrying one would let a step be marked complete by assertion through a
/// type that looks like evidence.
///
/// `#[non_exhaustive]` because `Findings` joins this enum when spec 22 exists
/// to produce finding ids (`context.md` §3.6), and that must not be a
/// breaking change for the shell.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
#[non_exhaustive]
pub enum Evidence {
    /// A command ran and exited.
    ///
    /// `marker_id`, not a `CommandExecution` id: execution ids reach only the
    /// audit log, which expires on `audit_retention_days`, while the marker is
    /// in the session log beside this very event. The `exit_code` is the
    /// evidence; the marker is the breadcrumb. `context.md` §3.5.
    #[serde(rename_all = "camelCase")]
    CommandExit {
        marker_id: String,
        exit_code: Option<i32>,
    },
    /// A patch was applied, with the hash actually written per file.
    #[serde(rename_all = "camelCase")]
    PatchApplied {
        marker_id: String,
        files: Vec<PatchedFile>,
    },
    /// A file was read, with its hash at read time.
    FileRead { path: String, hash: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PatchedFile {
    pub path: String,
    /// What was written, which for a partial-hunk accept differs from the
    /// patch's own `new_hash` (`patch_engine.rs`, `applied_hash`).
    pub applied_hash: String,
}

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
    /// Empty is meaningful: the step is done and nothing observable confirms
    /// it, which the completion report states rather than hides.
    pub evidence: Vec<Evidence>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskPlan {
    pub task_id: String,
    pub created_at_ms: u128,
    pub steps: Vec<PlanStep>,
}

impl TaskPlan {
    pub fn new(task_id: impl Into<String>, created_at_ms: u128) -> Self {
        Self {
            task_id: task_id.into(),
            created_at_ms,
            steps: Vec::new(),
        }
    }

    /// Requirement 2. Surfaced as a predicate rather than enforced in a setter
    /// so the panel and the tests can both assert it against a replayed plan,
    /// which is where a violation would actually show up.
    pub fn violates_single_in_progress(&self) -> bool {
        self.steps
            .iter()
            .filter(|step| step.status == StepStatus::InProgress)
            .count()
            > 1
    }
}
```

Add `pub mod plan;` to `crates/workspace-engine/src/lib.rs` and re-export
`Evidence`, `PlanStep`, `StepStatus`, `TaskPlan` beside the other `pub use`
lines.

- [ ] **Step 4: Run it to verify it passes**

Run: `cargo test -p workspace-engine --test plan`
Expected: PASS.

- [ ] **Step 5: Run the full gate, then ask before committing**

Proposed message: `Add the plan and evidence types a turn works through`

#### What this task actually did

All seven gate commands pass; 23 test binaries, up from 22.

Two additions beyond the plan's sketch, both about the same hazard:

- **`PatchedFile` is a named struct, not a `Vec<(String, String)>`.** The
  proposal's tuple pair gives no indication which element is the path and which
  the hash, and both are strings, so transposing them would produce a plausible
  record and a silently wrong one. The field name `applied_hash` also documents
  at the type level that it is not `new_hash`.
- **`an_absent_exit_code_survives_the_round_trip_as_absent`** was not in the
  plan. Serde is exactly the boundary where `None` could become `0` without
  anyone writing the `unwrap_or(0)` the spec warns about — a `#[serde(default)]`
  added later for an unrelated reason would do it. The test pins the wire form
  so that change fails loudly.

Mutation-checked the predicate rather than only reading it: `> 1` → `> 2` fails
`only_one_step_may_be_in_progress` and nothing else.

`violates_single_in_progress` is a predicate rather than an invariant enforced
in a setter, and the doc comment says why: a plan is replayed from an
append-only log, so the violation that matters is one visible in the
*assembled* plan after a crash or an out-of-order append — which a setter never
sees.

---

### Task 3: Plan persistence and replay

**Files:**
- Modify: `crates/workspace-engine/src/session.rs`
- Test: `crates/workspace-engine/tests/plan.rs`

**Interfaces:**
- Consumes: `TaskPlan`, `PlanStep`, `StepStatus` from Task 2;
  `append_session_event`, `active_events`, `parsed_events` from `session.rs`.
- Produces: `SessionStore::create_plan(&Task, &TaskPlan)`,
  `SessionStore::update_plan_step(&Task, &PlanStep)`,
  `SessionStore::read_task_plan(session_id, task_id) -> Result<Option<TaskPlan>>`.

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn the_newest_event_per_step_wins_on_replay() {
    let fixture = Fixture::new();
    let task = fixture.task("add retry handling");
    let mut plan = TaskPlan::new(&task.id, 0);
    plan.steps.push(step("step_1", StepStatus::Pending));
    plan.steps.push(step("step_2", StepStatus::Pending));
    fixture.store.create_plan(&task, &plan).unwrap();

    let mut first = plan.steps[0].clone();
    first.status = StepStatus::InProgress;
    fixture.store.update_plan_step(&task, &first).unwrap();
    first.status = StepStatus::Completed;
    first.evidence = vec![Evidence::CommandExit {
        marker_id: "action_1".to_string(),
        exit_code: Some(0),
    }];
    fixture.store.update_plan_step(&task, &first).unwrap();

    let replayed = fixture
        .store
        .read_task_plan(&fixture.session_id, &task.id)
        .unwrap()
        .expect("a plan was created");
    assert_eq!(replayed.steps[0].status, StepStatus::Completed);
    assert_eq!(replayed.steps[0].evidence.len(), 1);
    // Untouched, and still in its original position.
    assert_eq!(replayed.steps[1].status, StepStatus::Pending);
    assert_eq!(replayed.steps[1].id, "step_2");
}

#[test]
fn a_plan_from_another_task_in_the_same_session_is_not_returned() {
    let fixture = Fixture::new();
    let first = fixture.task("one");
    let second = fixture.task("two");
    let mut plan = TaskPlan::new(&first.id, 0);
    plan.steps.push(step("step_1", StepStatus::Pending));
    fixture.store.create_plan(&first, &plan).unwrap();

    assert!(
        fixture
            .store
            .read_task_plan(&fixture.session_id, &second.id)
            .unwrap()
            .is_none()
    );
}

#[test]
fn a_torn_final_line_does_not_discard_the_plan_before_it() {
    let fixture = Fixture::new();
    let task = fixture.task("add retry handling");
    let mut plan = TaskPlan::new(&task.id, 0);
    plan.steps.push(step("step_1", StepStatus::Pending));
    fixture.store.create_plan(&task, &plan).unwrap();
    fixture.append_raw("{\"seq\":99,\"eventType\":\"plan_step_upda");

    let replayed = fixture
        .store
        .read_task_plan(&fixture.session_id, &task.id)
        .unwrap();
    assert!(replayed.is_some());
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p workspace-engine --test plan`
Expected: FAIL, `no method named create_plan`.

- [ ] **Step 3: Implement the three methods**

In `session.rs`, following `read_task_statuses`'s shape:

```rust
/// Appends the plan a turn will work through.
pub fn create_plan(&self, task: &Task, plan: &TaskPlan) -> Result<()> {
    self.append_session_event(
        &task.session_id,
        "plan_created",
        &serde_json::to_string(plan)?,
    )
}

/// Appends one step's new state. The step is written whole rather than as a
/// delta: a partial update would need the reader to know which absent field
/// means "unchanged" and which means "cleared", and spec 17's rule is that
/// the log says what is true, not what changed.
pub fn update_plan_step(&self, task: &Task, step: &PlanStep) -> Result<()> {
    self.append_session_event(
        &task.session_id,
        "plan_step_updated",
        &format!(
            "{{\"taskId\":\"{}\",\"step\":{}}}",
            escape_json(&task.id),
            serde_json::to_string(step)?
        ),
    )
}

/// The task's plan as of the newest event for each step.
///
/// Reads **active** events, unlike `read_task_usage`: a rewind moves the
/// conversation back and the plan is part of the conversation, not a fact
/// about money that was spent regardless.
pub fn read_task_plan(
    &self,
    session_id: &str,
    task_id: &str,
) -> Result<Option<TaskPlan>> {
    let Ok(content) = fs::read_to_string(self.session_log_path(session_id)) else {
        return Ok(None);
    };
    let mut plan: Option<TaskPlan> = None;
    for event in active_events(&content) {
        match event.event_type.as_str() {
            "plan_created" | "plan_revised" => {
                let Ok(created) =
                    serde_json::from_value::<TaskPlan>(event.payload.clone().into())
                else {
                    continue;
                };
                if created.task_id == task_id {
                    plan = Some(created);
                }
            }
            "plan_step_updated" => {
                let Some(current) = plan.as_mut() else {
                    continue;
                };
                if event.text("taskId").as_deref() != Some(task_id) {
                    continue;
                }
                let Some(updated) = event
                    .payload
                    .get("step")
                    .cloned()
                    .and_then(|value| serde_json::from_value::<PlanStep>(value).ok())
                else {
                    continue;
                };
                if let Some(existing) =
                    current.steps.iter_mut().find(|step| step.id == updated.id)
                {
                    *existing = updated;
                }
            }
            _ => {}
        }
    }
    Ok(plan)
}
```

A step id the plan does not contain is ignored rather than appended: a plan's
step list is set by `plan_created` and `plan_revised`, and letting an update
introduce a step would let the log grow a plan nobody wrote.

- [ ] **Step 4: Run to verify it passes**

Run: `cargo test -p workspace-engine --test plan`
Expected: PASS.

- [ ] **Step 5: Mutation-test the task filter**

Delete the `created.task_id == task_id` guard. The second test must fail.
Restore it.

- [ ] **Step 6: Run the full gate, then ask before committing**

Proposed message: `Persist a turn's plan in the session log and replay it back`

#### What this task actually did

All seven gate commands pass; 11 tests in `tests/plan.rs`.

**`revise_plan` landed here, not in Task 11.** The reader already had to treat
`plan_revised` as a plan-establishing event to fold updates correctly, so
writing the event in a later task would have left a reader handling a case
nothing could produce — untestable, and the kind of dead branch that rots.
Task 11 now only has to call it and gate on it.

**Four tests beyond the plan's three**, each pinning a decision the plan stated
but did not verify:

- `an_update_naming_a_step_the_plan_does_not_have_is_ignored` — the plan's prose
  said a step list comes only from `plan_created`/`plan_revised`. Mutation-checked
  by making the reader `push` the unknown step: fails exactly this test.
- `a_task_with_no_plan_reports_none_rather_than_an_empty_plan` — `None` and a
  zero-step plan are different facts (a trivial turn versus a plan that proposed
  nothing), and the completion report will have to tell them apart.
- `a_plan_event_records_its_kind_so_the_log_is_readable_by_hand` — §5.7 promises
  `TROUBLESHOOTING.md` will tell a user where plan events are. That promise is
  now a test rather than an intention.
- `a_rewind_past_a_plan_takes_the_plan_with_it` — **the one worth reading twice.**
  `read_task_plan` uses `active_events` while `read_task_usage` deliberately does
  not, and that asymmetry was a doc comment with nothing behind it. Swapping
  `active_events` for `parsed_events` fails only this test. Without it the
  distinction was an assertion about intent, not behaviour.

**One thing deliberately not done.** `append_plan` does not overwrite the plan's
own `task_id` from the `Task` it is handed, even though that would look like
tightening. Task 10's `resume_plan` writes a plan whose `task_id` it has
rewritten on purpose; re-deriving it here would send the carried plan straight
back to the task it came from, and the carry-over would silently do nothing.

---

### Task 4: Evidence, minted at the call site

**Files:**
- Modify: `crates/workspace-engine/src/chat.rs`
- Modify: `crates/workspace-engine/src/edit.rs` (the patch-apply marker)
- Test: `crates/workspace-engine/tests/plan.rs`

**Interfaces:**
- Consumes: `ActionOutcome` from Task 1; `Evidence`, `PatchedFile` from Task 2.
- Produces: `fn evidence_for(outcome: &ActionOutcome, marker_id: &str) -> Option<Evidence>`
  in `chat.rs`, and evidence attached to the in-progress step when an arm
  completes.

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn evidence_cannot_be_built_from_model_output() {
    // The guard is structural: `Evidence` has no variant carrying a model
    // claim, and no constructor takes a tool argument. This test pins the
    // structure so adding one is a deliberate, visible act rather than a
    // convenience someone reaches for under deadline.
    let source = std::fs::read_to_string(
        concat!(env!("CARGO_MANIFEST_DIR"), "/src/plan.rs"),
    )
    .unwrap();
    assert!(
        !source.contains("ModelAsserted"),
        "requirement 6: evidence is what Damaian observed, never what the model claimed"
    );
}

#[test]
fn a_failing_command_produces_evidence_carrying_its_code() {
    let evidence = evidence_for(&ActionOutcome::CommandExit(Some(1)), "action_1");
    assert_eq!(
        evidence,
        Some(Evidence::CommandExit {
            marker_id: "action_1".to_string(),
            exit_code: Some(1),
        })
    );
}

#[test]
fn a_command_with_no_exit_code_still_produces_evidence() {
    // Not `None`: "we ran it and learned nothing" is a different fact from
    // "we did not run it", and Task 5 needs the difference to block the step
    // rather than mark it unverified-complete.
    let evidence = evidence_for(&ActionOutcome::CommandExit(None), "action_1");
    assert_eq!(
        evidence,
        Some(Evidence::CommandExit {
            marker_id: "action_1".to_string(),
            exit_code: None,
        })
    );
}
```

`evidence_for` is `pub(crate)`; expose it to the integration test through a
`#[doc(hidden)] pub` re-export or move these three into a `#[cfg(test)]` module
inside `chat.rs`. Prefer the latter — it is a unit of `chat.rs`, not of the
public API.

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p workspace-engine evidence`
Expected: FAIL, `cannot find function evidence_for`.

- [ ] **Step 3: Implement `evidence_for` and wire the arms**

```rust
/// The evidence an arm's outcome supports, or `None` where the engine observed
/// nothing worth recording.
///
/// `ActionOutcome::Ok` from a read-only tool yields `None` rather than a
/// synthetic success: a `read_file` that succeeded says nothing about whether
/// the step it served is done, and manufacturing evidence from it is exactly
/// the letter-not-spirit failure requirement 6 guards against.
pub(crate) fn evidence_for(
    outcome: &ActionOutcome,
    marker_id: &str,
) -> Option<Evidence> {
    match outcome {
        ActionOutcome::CommandExit(exit_code) => Some(Evidence::CommandExit {
            marker_id: marker_id.to_string(),
            exit_code: *exit_code,
        }),
        ActionOutcome::Ok | ActionOutcome::Failed => None,
    }
}
```

In the loop, after finishing the marker, push `evidence_for(...)` onto the
in-progress step and persist it with `update_plan_step`. Where no plan exists
(a trivial turn, §5.1), this is a no-op.

In `edit.rs`, the apply path already computes `applied_hash` per file; attach an
`Evidence::PatchApplied` carrying the marker id and one `PatchedFile` per file
written. `applied_hash`, not `new_hash` — a partial-hunk accept writes content
that differs from the proposal.

- [ ] **Step 4: Run to verify it passes**

Run: `cargo test -p workspace-engine evidence`
Expected: PASS.

- [ ] **Step 5: Run the full gate, then ask before committing**

Proposed message: `Attach evidence to a step from what the tool arm observed`

#### What this task actually did — and the gap it exposed

**This plan has no task that creates a plan or advances its steps.** Task 4 as
written says "push `evidence_for(...)` onto the in-progress step", which
presumes a plan with an in-progress step exists. Nothing produces one. Task 11
*gates* on a plan, Task 6 derives a phase *from* steps, Task 12 renders them —
but §5.1's creation rule ("a turn gets a plan when it will propose a patch, run
a mutating command, or has more than one step in the model's own proposal") and
requirement 1's step advancement were never given a task. That is a real hole in
the plan, not a detail, and it is now **Task 4b** below.

So Task 4 delivered the half that is testable today — evidence *construction* —
and left attachment to 4b. What landed:

- **`evidence_for` in `chat.rs`**, the single place an `ActionOutcome` becomes
  an `Evidence`. Five unit tests, including the two the plan did not ask for:
  `ActionOutcome::Ok` and `ActionOutcome::Failed` both yield `None`. A
  `read_file` that worked says nothing about whether the step it served is done,
  and minting a success record from it would make every step look verified.
- **`PatchApplyResult::applied`**, carrying the hash actually written per file.
  Mutation-checked against the obvious wrong version: using the patch's
  `new_hash` fails `applies_only_selected_hunk_and_allows_rollback_afterward`,
  because a partial-hunk accept writes content the proposal never described.
- **The structural guard** that no `ModelAsserted` variant exists. Adding one
  fails the test by name.

**One thing tried and reverted.** `edit.rs`'s apply path builds an
`Evidence::PatchApplied` naturally — marker and hashes are both in hand there —
but with nothing to attach it to the statement was `let _evidence = …`:
construction whose result is discarded. That is dead code wearing the costume of
wiring, and it would have read as "done" in review. It belongs in 4b, where
there is a step to attach it to.

**A note on the guard test's first failure.** It failed immediately — on
`plan.rs`'s own doc comment explaining why `ModelAsserted` does not exist. The
fix was to strip comments before scanning, and the reason is worth keeping: a
guard that cannot tell a declaration from prose about the absence of one gets
deleted by the next person who documents the rule, who is exactly the person it
protects.

---

### Task 4b: The plan lifecycle in a turn

Missing from the original plan; see Task 4's note. This is what makes a plan
exist during a turn, and every later task that reads one depends on it.

**Files:**
- Modify: `crates/workspace-engine/src/chat.rs`
- Modify: `crates/workspace-engine/src/edit.rs` (attach `PatchApplied`)
- Test: `crates/workspace-engine/tests/plan.rs`

**Interfaces:**
- Consumes: `SessionStore::create_plan` / `update_plan_step` (Task 3),
  `evidence_for` (Task 4), `status_from_evidence` (Task 5).
- Produces: a plan created during a non-trivial turn, with exactly one step
  `InProgress` at a time and evidence attached as each completes.

- [ ] **Step 1: Decide the triviality rule, and write its test first**

§5.1 defines non-trivial as "will propose a patch, run a mutating command, or
has more than one step in the model's own proposal". The first two are only
knowable *after* the model's first response, which is the constraint the rule
has to be written around — a plan cannot be created before the turn starts
without guessing. Assert both directions: a single-question turn produces
`read_task_plan == None`, and a patch-proposing turn produces a plan.

Record the rule actually used in proposal §7, which asks for it by name along
with how often it produced a plan for a turn that did not need one.

- [ ] **Step 2: Assert the single-in-progress invariant against a real turn**

`TaskPlan::violates_single_in_progress` exists and is unit-tested against
hand-built plans. Requirement 2 is about *runs*, so drive a multi-step turn and
assert the replayed plan never violates it — checking after each
`update_plan_step`, not only at the end, since a transient double would be
exactly the bug and invisible at the terminus.

- [ ] **Step 3: Attach evidence as each step completes**

Push `evidence_for(&action_outcome, marker_id)` onto the in-progress step and
persist with `update_plan_step`. In `edit.rs`, attach the
`Evidence::PatchApplied` built from `result.applied` — the construction Task 4
deliberately did not leave behind as dead code.

- [ ] **Step 4: Set the step's status with `status_from_evidence`, never directly**

The model never writes a step status (§5.3). Assert it: a turn whose model
output claims completion while its command exited non-zero must leave the step
`Blocked`.

- [ ] **Step 5: Run the full gate, then ask before committing**

Proposed message: `Give a non-trivial turn a plan and advance it as work lands`

#### What this task actually did

All seven gate commands pass; 24 test binaries, 7 tests in the new
`tests/plan_turn.rs`. **Task 5 was done first**, since step 4 here depends on
`status_from_evidence`.

**The design decision §5.1 left open: who writes the plan.** §5.1 defines
non-trivial as a turn that "will propose a patch, run a mutating command, or has
more than one step in the model's own proposal". The first two are only knowable
*after* the model's first response, and the third presumes the model can propose
steps — which nothing let it do. A plan derived from the tool calls a turn
happened to make would not be a plan at all: it is a log, it cannot exist before
the work, and §5.5 requires the plan to be reviewable *before* any mutating step
runs. So the model must author it.

Two tools, and the split between them is requirement 6:

- **`propose_plan { steps: [{title, detail?}] }`** — the model supplies titles
  and nothing else. Status, timings and evidence are the engine's to write. A
  proposal of fewer than two steps decodes to `None`, so §5.1's "a one-step plan
  is ceremony" is enforced at the one place that decides rather than left to the
  prompt.
- **`complete_step {}`** — takes no arguments *by design*. The model asks to move
  on; it does not say how the step ended. The engine calls
  `status_from_evidence` on what accrued while the step was open. An `outcome`
  argument here would have handed the model the exact field this spec exists to
  keep out of its reach, and it would have looked perfectly reasonable in a
  schema.

Titles and details are redacted through `SecretScanner` before they are written:
they are model-authored text rendered in the panel and stored in the log, so a
secret echoed into one must not survive there.

**A blocked step does not hand off.** When `status_from_evidence` returns
`Blocked`, the next step stays `Pending` — its prerequisite failed, and opening
it would build on work that did not happen. That is §4's "dependencies are
recorded and used to block a step whose prerequisite failed", falling out of the
status rule rather than needing a solver.

**`no_point_in_the_log_ever_has_two_steps_in_progress` is the test to read.**
Requirement 2 is about runs, and asserting it on the final plan would have been
worthless: the handoff writes the closing step and the opening one as two
separate appends, so a transient double is exactly the shape the bug takes and
is invisible at the terminus. The test replays every log prefix instead.
Mutation-checked by writing the two appends in the wrong order — it fails with
`two steps in progress after 24 events`, while the **final** state stays
correct. An end-state assertion would have passed.

**One piece is genuinely blocked, and not by choice.** `Evidence::PatchApplied`
cannot be attached yet. A patch is *proposed* in one turn — the chat loop breaks
out to await review — and *applied* later through
`edit.rs::apply_stored_patch`, after that turn has ended. The proposing turn's
step is still open, but `step_evidence` is turn-local and gone, and the plan
itself is keyed to a task that is over. Attaching it needs the cross-turn plan
continuation from **Task 10**, which is the same mechanism resume needs for the
same underlying reason: a task is one turn (`context.md` §3.1).

Two consequences for Task 10 to honour:

- `CompleteStep` currently does `open.evidence = evidence` (replace). Once
  something else can append evidence to a persisted step, that must become an
  extend, or the apply-path evidence will be silently dropped by the next
  `complete_step`.
- `PatchApplyResult::applied` (added in Task 4) has no consumer until then. It
  is tested and correct; it is waiting.

**A turn that never calls `complete_step` leaves its step `InProgress`.** That is
honest — the work was not declared finished — but it means a completed task can
hold an open step, and Task 6's phase derivation must not read that as
"Complete". Noted there rather than papered over here.

---

### Task 5: Step status is a function of evidence

**Files:**
- Modify: `crates/workspace-engine/src/plan.rs`
- Test: `crates/workspace-engine/tests/plan.rs`

**Interfaces:**
- Produces: `fn status_from_evidence(evidence: &[Evidence]) -> StepStatus`, and
  `PlanStep::is_unverified()`.

- [ ] **Step 1: Write the failing test, one case per table row**

```rust
#[test]
fn a_clean_exit_completes_the_step() {
    assert_eq!(
        status_from_evidence(&[command_exit(Some(0))]),
        StepStatus::Completed
    );
}

#[test]
fn a_non_zero_exit_blocks_the_step() {
    assert_eq!(
        status_from_evidence(&[command_exit(Some(1))]),
        StepStatus::Blocked
    );
}

#[test]
fn an_absent_exit_code_blocks_the_step() {
    // The `unwrap_or(0)` failure mode, asserted directly per §6.
    assert_eq!(
        status_from_evidence(&[command_exit(None)]),
        StepStatus::Blocked
    );
}

#[test]
fn one_failure_among_successes_still_blocks() {
    assert_eq!(
        status_from_evidence(&[command_exit(Some(0)), command_exit(Some(1))]),
        StepStatus::Blocked
    );
}

#[test]
fn no_evidence_completes_the_step_but_marks_it_unverified() {
    assert_eq!(status_from_evidence(&[]), StepStatus::Completed);
    let step = PlanStep { evidence: Vec::new(), ..completed_step() };
    assert!(step.is_unverified());
}

#[test]
fn a_step_with_evidence_is_not_unverified() {
    let step = PlanStep {
        evidence: vec![command_exit(Some(0))],
        ..completed_step()
    };
    assert!(!step.is_unverified());
}
```

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test -p workspace-engine --test plan status_from_evidence`
Expected: FAIL, `cannot find function status_from_evidence`.

- [ ] **Step 3: Implement**

```rust
/// Requirement 6's rule, mechanically. The model does not appear in this
/// function's inputs, which is the point of it existing.
///
/// Empty evidence completes the step: a step like "understand the retry logic"
/// genuinely has nothing observable behind it, and inventing a fake observation
/// would be worse than saying so. `PlanStep::is_unverified` is how the
/// completion report says so.
pub fn status_from_evidence(evidence: &[Evidence]) -> StepStatus {
    let blocked = evidence.iter().any(|item| match item {
        // `Some(0)` and nothing else. `None` means the command was killed or
        // signalled, so nothing is known, and "nothing is known" is not
        // "it passed".
        Evidence::CommandExit { exit_code, .. } => *exit_code != Some(0),
        Evidence::PatchApplied { .. } | Evidence::FileRead { .. } => false,
    });
    if blocked {
        StepStatus::Blocked
    } else {
        StepStatus::Completed
    }
}
```

The `match` is exhaustive over a `#[non_exhaustive]` enum within its own crate,
so adding `Findings` later will fail to compile here — which is correct: a new
evidence kind must be given a blocking answer deliberately.

```rust
impl PlanStep {
    /// Completed with nothing observable behind it. Requirement 6's second
    /// sentence, and what the completion report prints as "completed
    /// unverified".
    pub fn is_unverified(&self) -> bool {
        self.status == StepStatus::Completed && self.evidence.is_empty()
    }
}
```

- [ ] **Step 4: Run to verify they pass**

Run: `cargo test -p workspace-engine --test plan`
Expected: PASS.

- [ ] **Step 5: Mutation-test the `None` arm**

Change `*exit_code != Some(0)` to `matches!(exit_code, Some(code) if *code != 0)`
— which is the plausible wrong version, treating `None` as fine.
`an_absent_exit_code_blocks_the_step` must fail. Restore.

- [ ] **Step 6: Run the full gate, then ask before committing**

Proposed message: `Decide a step's status from its evidence rather than a claim`

#### What this task actually did

Done **before** Task 4b, which depends on it — the plan's numbering had them the
other way round.

Mutation ran exactly as specified: `*exit_code != Some(0)` rewritten as
`matches!(exit_code, Some(code) if *code != 0)` — the plausible version someone
writes when "simplifying" — fails only `an_absent_exit_code_blocks_the_step`.

Two tests beyond the plan's six:

- `a_blocked_step_is_not_reported_as_unverified`. "Unverified" means completed
  with nothing behind it; a blocked step is not completed at all. Without this,
  `is_unverified` could have been written as `evidence.is_empty()` and a blocked
  step with no evidence would have been reported as a soft pass — turning a
  failure into a completion, which is the laundering §5.3 exists to forbid.
- `a_patch_applied_completes_the_step`, so the non-`CommandExit` arms are pinned
  rather than left to the `_ => false` reading.

The match over `Evidence` is exhaustive in-crate even though the enum is
`#[non_exhaustive]`, so `Findings` cannot be added later without someone
deciding here whether it blocks a step. That is the deferral from
`context.md` §3.6 doing work rather than merely postponing it.

---

### Task 6: `TaskPhase`, derived

**Files:**
- Modify: `crates/workspace-engine/src/plan.rs`
- Test: `crates/workspace-engine/tests/plan.rs`

**Interfaces:**
- Produces: `TaskPhase` and `TaskPlan::phase() -> TaskPhase`.

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn the_phase_cannot_contradict_the_steps() {
    // §6: derived, never set. A plan whose validation step has not started
    // must not report `Validating` however the steps are labelled.
    let mut plan = TaskPlan::new("task_1", 0);
    plan.steps.push(step("step_1", StepStatus::InProgress));
    plan.steps.push(step("step_2", StepStatus::Pending));
    assert_ne!(plan.phase(), TaskPhase::Complete);

    for item in plan.steps.iter_mut() {
        item.status = StepStatus::Completed;
    }
    assert_eq!(plan.phase(), TaskPhase::Complete);
}

#[test]
fn a_blocked_step_keeps_the_plan_out_of_complete() {
    let mut plan = TaskPlan::new("task_1", 0);
    plan.steps.push(step("step_1", StepStatus::Completed));
    plan.steps.push(step("step_2", StepStatus::Blocked));
    assert_ne!(plan.phase(), TaskPhase::Complete);
}
```

- [ ] **Step 2: Run to verify it fails**

Expected: FAIL, `cannot find type TaskPhase`.

- [ ] **Step 3: Implement**

```rust
/// What the work is about, distinct from `PhaseKind`, which says which stage
/// of the agent loop is executing (`context.md` §3.7). Orthogonal axes: a
/// `PhaseKind::Model` occurs during every one of these six.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskPhase {
    Understanding,
    Planning,
    Editing,
    Validating,
    Reviewing,
    Complete,
}
```

Derive it from the steps: `Complete` only when every step is `Completed` or
`Skipped`; otherwise the phase of the first step that is not. Map a step to a
phase from its evidence kinds and its position — record the exact rule chosen in
proposal §7, since §7 asks for it.

- [ ] **Step 4: Run to verify it passes, then run the full gate and ask**

Proposed message: `Derive the task phase from step state so it cannot contradict it`

#### What this task actually did

All seven gate commands pass; 28 tests in `tests/plan.rs`.

**`phase(awaiting_review: bool)` takes an argument, and that is the finding.**
Five of requirement 3's six phases are derivable from steps. `Reviewing` is not:
a patch waiting on a human is a fact about the *task*, not about its steps.
Three ways out, and only one is honest — guess it from the steps (inventing it),
drop the variant (contradicting requirement 3), or take it as an argument. The
argument makes the dependency visible at every call site instead of burying a
guess inside the derivation. There is still no setter, so §5.1's rule holds: the
phase cannot be stored, and so cannot disagree with the steps.

**Precedence, and why `Blocked` is not terminal here.** Completion outranks
review — a finished plan has no work left for a review to gate. And a `Blocked`
step keeps the plan out of `Complete`, because work that failed is work
outstanding; §5.6 requires the summary never to say "complete" for a plan
holding one. `Skipped` *is* terminal: someone decided it was not needed, which
is an answer rather than a failure.

**Newest evidence, not "any".** "Any command anywhere" would pin the phase to
whatever the turn did first — a step that read a file and then ran a check is
validating, not understanding.

Three mutations run, each killing exactly one test: `Blocked` counted as
terminal, `.next_back()` → `.next()`, and review ranked above completion.

**Two honest limits, recorded here and belonging in proposal §7.**

- **`Planning` is nearly unreachable.** A plan is created with its first step
  already `InProgress` in the same event, so the only way to observe `Planning`
  is an empty plan. Not a bug, but the phase set is effectively five in
  practice, and §7 should say so rather than let a reader infer six working
  phases from the enum.
- **`Editing` cannot be reached yet.** It needs `Evidence::PatchApplied`, which
  cannot be attached until Task 10's cross-turn continuation exists — a patch is
  applied after the turn that proposed it has ended (see Task 4b). The arm is
  written and covered through `status_from_evidence`; what is missing is a
  producer. **Task 10 should re-check this when that lands.**

`a_turn_that_never_finished_its_step_is_not_complete` pins the case Task 4b
flagged: a turn can end without calling `complete_step`, leaving the step open,
and the phase must not round that up to `Complete`.

---

### Task 7: `TokenBudgetExhausted` status

**Files:**
- Modify: `crates/workspace-engine/src/session.rs`
- Test: `crates/workspace-engine/tests/crash_recovery.rs`

**Interfaces:**
- Produces: `TaskStatus::TokenBudgetExhausted`, terminal, no side effect in
  flight.

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn a_token_budget_stop_records_a_completion_time() {
    // `update_task_status` decides this from a hand-written list of terminal
    // statuses (`context.md` §3.2). The compiler does not check that list, and
    // `TaskStatus::all()` does not reach it, so a new terminal status omitted
    // there gets no timestamp and nothing else fails.
    let fixture = Fixture::new();
    let task = fixture.task("spend it all");
    let stopped = fixture
        .store
        .update_task_status(&task, TaskStatus::TokenBudgetExhausted, None)
        .unwrap();
    assert!(stopped.completed_at_ms.is_some());
}

#[test]
fn every_terminal_status_records_a_completion_time() {
    // The general form, so the next terminal status added cannot repeat this.
    let fixture = Fixture::new();
    for status in TaskStatus::all().into_iter().filter(TaskStatus::is_terminal) {
        let task = fixture.task("t");
        let stopped = fixture
            .store
            .update_task_status(&task, status.clone(), None)
            .unwrap();
        assert!(
            stopped.completed_at_ms.is_some(),
            "{} is terminal but records no completion time",
            status.as_str()
        );
    }
}
```

- [ ] **Step 2: Run to verify they fail**

Expected: FAIL, `no variant named TokenBudgetExhausted`.

- [ ] **Step 3: Add the variant**

Add to the enum, `all()`, `as_str()` (`"token_budget_exhausted"`), and
`is_terminal()`. Leave it out of `may_have_side_effect_in_flight` — the stop
happens before a call, so nothing is in flight.

Then replace `update_task_status`'s hand-written `matches!` with
`updated.status.is_terminal()`, so the two can never disagree again.

- [ ] **Step 4: Run to verify they pass**

Run: `cargo test -p workspace-engine`
Expected: PASS. The spec-17 state tests that enumerate `all()` will also need
the new variant classified; that is the mechanism working as designed.

- [ ] **Step 5: Run the full gate, then ask before committing**

Proposed message: `Tell a turn stopped for tokens from one stopped for rounds`

#### What this task actually did

All seven gate commands pass; 24 test binaries.

**The prediction in `context.md` §3.2 held exactly.** The compiler and the
existing tests caught the variant in three places, and the fourth — the one they
could not reach — was the one the plan singled out:

1. `crash_recovery.rs`'s `expected()` matrix refused to compile until recovery's
   answer for the new state was written down. Classified `NotRecovered`, and for
   a reason of its own beyond "it is terminal": the token check runs *before* a
   model call rather than after one (§3.3), so the stop lands where nothing was
   in flight to begin with.
2. `every_state_and_crash_shape_recovers_the_way_the_matrix_says` failed on
   `cells`: 42 against an expected 39. That guard exists so the matrix cannot
   silently shrink, and it correctly reported that it had grown.
3. `terminal_states_are_exactly_these` failed with the new value in the list.
4. **`update_task_status` failed nothing.** Its hand-written terminal list is
   outside everything `TaskStatus::all()` reaches, so `TokenBudgetExhausted`
   would have been written with no `completedAtMs` and no test would have
   noticed.

Point 4 is now structurally impossible: the `matches!` is replaced by
`updated.status.is_terminal()`, so the two cannot disagree again. Mutation-checked
by restoring the hand-written list — both
`a_token_budget_stop_records_a_completion_time` and
`every_terminal_status_records_a_completion_time` fail, which is what "silent"
looked like before.

**One thing deliberately left for Task 12.** `app.js` reads
`taskStatus === "tool_budget_exhausted"` in three places to mark a turn that ran
out of rounds. A token stop currently renders as an ordinary completed turn —
wrong, and invisible. It needs its own label with its own remedy ("raise the
ceiling" rather than "narrow the request"), and that is UI work belonging with
the plan panel. **Task 12 must not ship without it**, or the distinction this
task exists to create stops at the engine boundary.

---

### Task 8: `agent_max_task_tokens` config, restrict-only

**Files:**
- Modify: `crates/workspace-engine/src/config.rs`
- Test: `crates/workspace-engine/tests/foundation.rs`

**Interfaces:**
- Produces: `Config::agent_max_task_tokens: Option<u64>`.

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn a_repository_may_lower_a_token_ceiling_but_not_raise_it() {
    // `context.md` §3.8. The three existing `agent_*` bounds are "Free", which
    // a repository may set either way. A ceiling is money: lowering it is a
    // nuisance, raising one the user set low spends their money, and spec 34
    // exists to stop that.
    let mut config = Config::default();
    config.agent_max_task_tokens = Some(50_000);

    let rejected = config.apply_overlay_scoped(
        overlay_with("agent_max_task_tokens", "200000"),
        ConfigScope::Repository,
    );
    assert_eq!(config.agent_max_task_tokens, Some(50_000));
    assert!(rejected.iter().any(|key| key.key == "agent_max_task_tokens"));

    config.apply_overlay_scoped(
        overlay_with("agent_max_task_tokens", "10000"),
        ConfigScope::Repository,
    );
    assert_eq!(config.agent_max_task_tokens, Some(10_000));
}

#[test]
fn a_repository_may_set_a_ceiling_where_the_user_set_none() {
    let mut config = Config::default();
    assert_eq!(config.agent_max_task_tokens, None);
    config.apply_overlay_scoped(
        overlay_with("agent_max_task_tokens", "10000"),
        ConfigScope::Repository,
    );
    assert_eq!(config.agent_max_task_tokens, Some(10_000));
}

#[test]
fn an_unset_ceiling_survives_a_round_trip_through_config_show() {
    // Omitted, not written as 0: a `0` read back would be a ceiling of zero.
    let config = Config::default();
    assert!(!config.config_show_lines().iter().any(|line| {
        line.starts_with("agent_max_task_tokens")
    }));
}
```

- [ ] **Step 2: Run to verify they fail, then implement**

Eight touch points (§3.8): the `Config` field, the `ConfigOverlay` field, the
exhaustive destructure in `apply_overlay_scoped`, the apply site, the key parser
at the `agent_*` arms, `config_show`'s line list, the overlay serializer, and
`Config::default`. The compiler catches the destructure; it does not catch
`config_show` or the serializer, which is why the round-trip test above exists.

The apply site goes in the restrict-only block beside `restrict_only_flag`, not
in the "Free" block:

```rust
// Restrict-only, unlike the three `agent_*` round bounds above it. Lowering a
// ceiling is a nuisance; raising one the user set low spends the user's money,
// which is a capability. `context.md` §3.8.
if let Some(value) = agent_max_task_tokens {
    restrict_only_ceiling(
        &mut self.agent_max_task_tokens,
        value,
        "agent_max_task_tokens",
        trusted,
        &mut rejected,
    );
}
```

```rust
/// Takes the *lower* of the two at an untrusted scope, and whatever was given
/// at a trusted one. `None` means no ceiling, so any value is a tightening.
fn restrict_only_ceiling(
    current: &mut Option<u64>,
    value: u64,
    key: &str,
    trusted: bool,
    rejected: &mut Vec<RejectedConfigKey>,
) {
    if trusted {
        *current = Some(value);
        return;
    }
    match *current {
        Some(existing) if value >= existing => {
            rejected.push(RejectedConfigKey::new(key, RepositoryKeyClass::Forbidden));
        }
        _ => *current = Some(value),
    }
}
```

Parse with a minimum: a ceiling of zero stops every turn before its first call,
which is indistinguishable from Damaian being broken. Reject below 1000.

- [ ] **Step 3: Run to verify they pass, run the full gate, then ask**

Proposed message: `Add a per-task token ceiling a repository may lower but not raise`

#### What this task actually did

All seven gate commands pass; 138 tests in `tests/foundation.rs`.

**The classification is the whole task, and it diverges from its neighbours.**
`agent_max_tool_rounds`, `agent_web_debug_max_tool_rounds` and
`agent_tool_retry_limit` all sit in the block commented "Free: preferences and
budgets, with no capability behind them", which a cloned repository may set in
either direction. `agent_max_task_tokens` does not go there. Lowering a ceiling
is a nuisance; **raising one the user deliberately set low spends the user's
money**, which is a capability, and spec 34 exists to stop exactly that. So it
uses the `restrict_only_*` shape instead: a repository may lower a ceiling or
set one where none existed, never raise one and never remove one.

`None` means no ceiling, so any value is a tightening — which is why a
repository setting the first ceiling is allowed rather than refused.

Three mutations, each caught:

- Classified as Free (`self.agent_max_task_tokens = Some(value)`) — the change
  someone makes to match the neighbouring keys. Fails
  `a_repository_may_lower_a_token_ceiling_but_not_raise_it`.
- Over-tightened so an existing ceiling can never be lowered either. Fails the
  same test's second half. (It does *not* fail
  `a_repository_may_set_a_ceiling_where_the_user_set_none`, because that path
  goes through the `None` arm — the mutation is narrower than "forbid
  everything", and the tests pin the two cases separately for that reason.)
- Minimum dropped to zero. Fails `config_rejects_a_ceiling_too_low_to_be_meaningful`.

**A minimum of 1000, refused at parse time.** A ceiling of zero — or of ten —
stops every turn before its first model call, which from the user's side is
indistinguishable from Damaian being broken. Refusing the typo where it is
typed beats surfacing it later as a hang nobody can explain.

**Two touch points the compiler does not catch**, exactly as `context.md` §3.8
predicted. The exhaustive destructure in `apply_overlay_scoped` forced six of
the eight; `config_show` and the overlay serializer it did not.

The `config_show` line is emitted **only when the ceiling is set** — printing
`0` for "no ceiling" would read back as a ceiling of zero, a far worse
configuration than the default. And
`an_unset_ceiling_is_omitted_rather_than_written_as_zero` asserts *both*
directions: absence when unset, and presence when set. Without the second half
the test would have passed even if the key were never emitted at all, which is
what it looked like before I added the `config_show` line — a green test proving
nothing.

---

### Task 9: Ceiling enforcement in the loop

**Files:**
- Modify: `crates/workspace-engine/src/chat.rs`
- Test: `crates/workspace-engine/tests/token_accounting.rs`

**Interfaces:**
- Consumes: `Config::agent_max_task_tokens`, `SessionStore::read_task_usage`,
  `TaskStatus::TokenBudgetExhausted`.

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn a_turn_stops_before_the_call_that_would_cross_the_ceiling() {
    // `context.md` §3.3: the round-budget pattern spends one more call on
    // crossing. For a money ceiling that is backwards, and because context
    // grows all turn it is the most expensive call of the turn. Count the
    // calls the adapter actually received.
    let fixture = Fixture::with_ceiling(10_000);
    let adapter = CountingAdapter::spending(6_000);
    let result = fixture.run_turn("add retry handling", adapter.clone());

    assert_eq!(adapter.calls(), 2, "a third call would cross the ceiling");
    assert_eq!(result.task.status, TaskStatus::TokenBudgetExhausted);
}

#[test]
fn an_estimated_total_is_enforced_against() {
    // §5.4: declining to enforce on an estimate would make the ceiling
    // inoperative for every provider that does not report usage.
    let fixture = Fixture::with_ceiling(10_000);
    let adapter = CountingAdapter::spending(6_000).reporting_no_usage();
    let result = fixture.run_turn("add retry handling", adapter.clone());
    assert_eq!(result.task.status, TaskStatus::TokenBudgetExhausted);
}

#[test]
fn an_unset_ceiling_imposes_no_limit() {
    let fixture = Fixture::with_no_ceiling();
    let adapter = CountingAdapter::spending(10_000_000);
    let result = fixture.run_turn("add retry handling", adapter.clone());
    assert_ne!(result.task.status, TaskStatus::TokenBudgetExhausted);
}

#[test]
fn the_stop_reports_the_remaining_steps_and_the_usage_consumed() {
    let fixture = Fixture::with_ceiling(10_000);
    let result = fixture.run_turn("add retry handling", CountingAdapter::spending(6_000));
    assert!(result.response.contains("remaining"));
    // The figure, not a placeholder.
    assert!(result.response.contains("12,000") || result.response.contains("12000"));
}
```

- [ ] **Step 2: Run to verify they fail, then implement**

At the **top of the loop**, beside the existing cancellation check and before
the `ModelRequest` is built:

```rust
// Before the request, not after the response (`context.md` §3.3). The round
// budget spends one more call on crossing; a money ceiling that did the same
// would spend its most expensive call proving it had run out.
//
// Absent usage is zero here, not "unknown": `read_task_usage` omits a task
// with no events because "not recorded" and "used nothing" differ for a
// *report*, but for a *ceiling* nothing spent is nothing spent, and treating
// absence as unenforceable would disable the ceiling for the first call of
// every turn. `context.md` §3.9.
if let Some(ceiling) = self.config.agent_max_task_tokens {
    let spent = self
        .session_store
        .read_task_usage(&session.id)?
        .get(&task.id)
        .map(|usage| usage.input_tokens + usage.output_tokens)
        .unwrap_or(0);
    if spent >= ceiling {
        break (
            ModelRun::not_started(&self.config.model_provider, &self.config.model_name),
            token_budget_exhausted_response(ceiling, spent, &plan),
            None,
            None,
            StopReason::TokenBudget,
        );
    }
}
```

The loop's five-tuple already carries `tool_budget_exhausted: bool`. Replace it
with a `StopReason` enum (`None`, `ToolBudget`, `TokenBudget`) rather than
adding a second boolean — two booleans admit a state where both are true, which
the audit status string would then have to pick between arbitrarily.

Write `token_budget_exhausted_response` beside `tool_budget_exhausted_response`,
naming the ceiling, the spend, and the steps still `Pending`.

- [ ] **Step 3: Run to verify they pass**

- [ ] **Step 4: Mutation-test the stop position**

Move the check to the bottom of the loop. `a_turn_stops_before_the_call_that_would_cross_the_ceiling`
must fail on the call count while the status assertion still passes — which is
the whole reason that test counts calls rather than only reading the status.
Restore.

- [ ] **Step 5: Run the full gate, then ask before committing**

Proposed message: `Stop a turn before the call that would cross its token ceiling`

#### What this task actually did

All seven gate commands pass; 25 test binaries, 5 tests in the new
`tests/token_ceiling.rs`. (New file rather than `token_accounting.rs`, which is
store-level and has no turn harness. Its name says what it holds.)

**The finding: my first version of the key test was worthless, and the mutation
is what showed it.**

`a_turn_stops_before_the_call_that_would_cross_the_ceiling` was written as
`calls < unbounded` — the ceiling turn makes fewer model calls than an
unconstrained one. It passed. Then the mutation — moving the check to the loop
tail — **also passed**, and so did a version enforcing it the round budget's
way. Of course they did: any early stop makes fewer calls than a turn that runs
to the round limit. The assertion measured "stopped early", which was never in
doubt, and not "stopped without paying for the discovery", which is the entire
point.

It is now an **exact** count: 3, measured rather than assumed. Two calls record
enough estimated usage to reach 1000 tokens, and the third iteration stops
before its request is built. Re-running the `force_final` mutation now fails
with `left: 4, right: 3` — the one extra call that shape spends. That single
call is the most expensive of the turn, because context grows monotonically
across rounds.

This is the pattern spec 18 §7 named, hit again from the other side: *a metric
whose expected value equals its failure value needs a test that moves it*. Here
the expected and failure values differed by one call, and a `<` comparison
could not see the difference. Worth remembering that a passing mutation check
is a result about the **test**, not about the code.

**`StopReason` replaces the loop's `tool_budget_exhausted: bool`.** Two booleans
would admit a state where both are set, and the task status and the audit
status string would each have to pick one arbitrarily. Seven `break` sites
converted; the compiler found them all.

**Absent usage is zero here, not "unknown".** `read_task_usage` omits a task
with no events on purpose — "not recorded" and "used nothing" differ for a
*report*. For a *ceiling* they do not: nothing spent is nothing spent, and
treating absence as unenforceable would disable the bound for the first call of
every turn, which is the only call some turns make (`context.md` §3.9).

**An estimated total is enforced against**, and the test says why it is
meaningful: `MockModelAdapter` reports no usage, so everything it records is
`Estimated`. The test asserts that precondition rather than assuming it, so it
fails loudly if the mock ever starts reporting measured usage — at which point
it would be silently testing the wrong thing.

The stop message names the ceiling, the actual spend, and the steps still
outstanding. Without the figures a user cannot tell a ceiling set too low from a
turn that genuinely ran away — opposite problems with opposite fixes.

---

### Task 10: Plan carry-over on resume

**Files:**
- Modify: `crates/workspace-engine/src/session.rs`, `chat.rs`
- Test: `crates/workspace-engine/tests/plan.rs`

**Interfaces:**
- Produces: `plan_resumed` event; `SessionStore::resume_plan(&Task, from_task_id)`.

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn a_resumed_turn_recovers_the_plan_of_the_task_it_resumes() {
    // `context.md` §3.1: a task is one turn, so resuming creates a *new* task
    // id and `read_task_plan` would find nothing. "The plan survives" is true
    // of the log and false of the user without this.
    let fixture = Fixture::new();
    let first = fixture.task("add retry handling");
    let mut plan = TaskPlan::new(&first.id, 0);
    plan.steps.push(step("step_1", StepStatus::Completed));
    plan.steps.push(step("step_2", StepStatus::Pending));
    fixture.store.create_plan(&first, &plan).unwrap();

    let second = fixture.task("add retry handling");
    fixture.store.resume_plan(&second, &first.id).unwrap();

    let carried = fixture
        .store
        .read_task_plan(&fixture.session_id, &second.id)
        .unwrap()
        .expect("the plan carried over");
    assert_eq!(carried.steps[0].status, StepStatus::Completed);
    assert_eq!(carried.steps[1].status, StepStatus::Pending);
    // The evidence came with it: a completed step's proof must not be dropped
    // by the act of resuming, or the resumed plan would re-run work it can
    // already show was done.
    assert_eq!(carried.steps[0].evidence, plan.steps[0].evidence);
}
```

- [ ] **Step 2: Run to verify it fails, then implement**

`resume_plan` reads the source task's plan, rewrites `task_id` to the new task,
and appends it as `plan_created` with a `resumedFrom` field naming the source.
Both plans stay in the log; neither is rewritten.

- [ ] **Step 3: Run to verify it passes, run the full gate, then ask**

Proposed message: `Carry a plan forward when a stopped turn is resumed`

#### What this task actually did

All seven gate commands pass; 25 test binaries. This task closes the two debts
Tasks 4b and 6 recorded against it.

**`resume_plan` carries a plan onto a new task, and the carry is narrow.** Only
when the previous task in the session ended `TokenBudgetExhausted` *and* its
plan still has work outstanding. A normally-completed task has nothing to
resume, and carrying a plan into an unrelated next question would put steps on
the panel the user never asked for. `plan_resumed` is its own event kind, so the
log shows the carry rather than hiding it as a second `plan_created`, and the
original task keeps its plan readable under its own id.

**`append_step_evidence` is the producer that was missing.** A patch is proposed
in one turn and applied later, after that turn has ended — so `edit.rs` has
evidence and no in-memory plan to put it on. It appends to whichever step is
open, and does nothing when none is: a closed step reached its status from its
own evidence, and adding to it afterwards would change the answer to a question
already settled. `PatchApplyResult::applied` (Task 4) finally has its consumer.

**Task 6's `TaskPhase::Editing` is now reachable**, and
`a_step_whose_patch_landed_reports_the_editing_phase` covers it. Until this task
it was a variant nothing could ever return.

**The mutation that survived, and what it cost to fix.** Task 4b predicted that
`complete_step`'s `open.evidence = accrued` would silently drop evidence
recorded outside the turn, and I changed it to `extend` here. Then the mutation
— putting `= accrued` back — **passed every test**. The fix was correct and
completely unproven: nothing in the suite had a step carrying evidence at
`complete_step` time.

`completing_a_carried_step_keeps_evidence_the_turn_never_saw` now drives the
real path end to end: a turn stops at the ceiling with a step open, the patch it
proposed is applied afterwards, the user raises the ceiling, the resumed turn
carries the plan and closes the step. Re-running the mutation fails it with
`left: 0, right: 1` — the evidence gone, and with it the step's verification.

Second time in two tasks that a mutation check reported on the *test* rather
than the code. Both times the code was right and the test was worthless, which
is the failure mode that ships quietly.

**One ordering subtlety.** `complete_step` now computes
`status_from_evidence(&open.evidence)` *after* the extend, not from the accrual
alone. Judging a step on a fraction of what is known about it is the same defect
as dropping the evidence, one step removed — a carried step with a failing
command behind it would have come out `Completed`.

---

### Task 11: Plan review gate

**Files:**
- Modify: `crates/workspace-engine/src/chat.rs`
- Test: `crates/workspace-engine/tests/plan.rs`

**Interfaces:**
- Consumes: the `PendingChatTurn` pause/resume machinery that already backs
  command approval.
- Produces: `plan_revised` event; a plan proposal in `ChatTurnResult`.

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn no_mutating_step_runs_before_the_plan_is_approved() {
    let fixture = Fixture::new();
    let result = fixture.run_turn_proposing_a_patch("add retry handling");
    assert!(result.plan_proposal.is_some());
    assert_eq!(fixture.patches_applied(), 0);
    assert_eq!(fixture.mutating_commands_run(), 0);
}

#[test]
fn a_revision_keeps_the_original_plan_in_the_log() {
    // §5.5: both the original and the user's revision are in the log, so the
    // history still describes what was proposed as well as what was run.
    let fixture = Fixture::new();
    let task = fixture.task("add retry handling");
    let mut original = TaskPlan::new(&task.id, 0);
    original.steps.push(step("step_1", StepStatus::Pending));
    original.steps.push(step("step_2", StepStatus::Pending));
    fixture.store.create_plan(&task, &original).unwrap();

    let mut revised = original.clone();
    revised.steps.remove(1);
    fixture.store.revise_plan(&task, &revised).unwrap();

    assert_eq!(fixture.plan_event_kinds(), vec!["plan_created", "plan_revised"]);
    assert_eq!(
        fixture
            .store
            .read_task_plan(&fixture.session_id, &task.id)
            .unwrap()
            .unwrap()
            .steps
            .len(),
        1
    );
}
```

- [ ] **Step 2: Run to verify they fail, then implement**

Reuse the `PendingChatTurn` shape: the plan gate is the same pause — persist the
turn state keyed by a proposal id, hand the user a proposal, resume on the
decision. Do not invent a second pause mechanism.

Gate only on the **first mutating** step, per §5.5: a plan whose steps are all
read-only needs no approval, and asking for one would make the gate noise the
user learns to click through.

- [ ] **Step 3: Run to verify they pass, run the full gate, then ask**

Proposed message: `Show a plan for approval before the first mutating step`

#### What this task actually did

**"Mutating" is not decidable from the action alone, and guessing was the
trap.** The plan said "gate only on the first mutating step", and the obvious
reading — reuse `tool_action_marker`'s `sideEffecting` flag — would have gated
every `run_command`, including the sandbox-safe `ls src` that three existing
tests run inside a plan. That flag answers a different question: *could a crash
here have left a side effect*, which is conservative on purpose. Whether a
command *mutates* depends on the command, and `CommandPolicy::classify` is the
only thing that knows. `action_awaits_plan_review` therefore takes the command
answer as a closure and the arm passes
`ValidationOrchestrator::command_needs_approval`, a new pure wrapper over the
same classifier `propose_command` uses so the two cannot disagree.

Gating on a step's *title* was never on the table: a title is model-authored
text, and deciding whether work is dangerous from what the model called it is
the exact pattern §5.3 exists to refuse.

**`propose_patch` is gated even though it is correctly not side-effecting.**
Proposing writes nothing, and `tool_action_marker` is right to say so. But it
is the front door to an edit, and a plan first seen *alongside* a prepared
patch is a plan seen too late to redirect — which is the whole point of the
gate. That is the one deliberate departure, and it is written down where the
departure is.

**The approval is a log fact, not turn state.** `plan_approved` /
`read_plan_approved` sit beside `read_task_plan` and read `active_events` for
the same reason: an approval is part of the conversation, so a rewind past it
takes it with it. The alternative — a rewind that drops the plan but keeps the
clearance granted for it — would let a rewind quietly widen what the agent may
do. Absence reads as `false`, never "unknown"; the gate is the only consumer,
and anything falling through to `true` would run a mutating step unreviewed.

**A revision may not delete a step that already ran.** Steps can reach a
terminal status before the first mutating action (the model can complete
read-only steps first), so the gate is not always hit on a pristine plan.
`apply_plan_revision` keeps terminal steps ahead of the user's list and ignores
a deletion that names one — §5.5's own reason for ruling out mid-execution
editing, applied to the one moment where editing *is* allowed. `StepStatus`
gained `is_terminal` for it.

**The deferred tool call is dropped, not replayed.** Nothing was dispatched
when the turn paused, so there is no marker to finish and nothing to undo. On
resume the decision goes to the model as a message and it asks again. Replaying
the stored call would run the step the user may have just revised away, which
would make the revision cosmetic.

**Five mutations, five distinct tests.** Each of the gate's decisions was
inverted and failed exactly the test written for it: dropping `!plan_approved`
fails `approving_the_plan_lets_the_patch_through`; making every command
mutating fails `a_plan_that_only_reads_is_never_put_up_for_approval` (and three
pre-existing tests, which is how the `sideEffecting` misreading surfaced);
letting a revision delete a completed step fails
`a_revision_may_not_delete_a_step_that_already_ran`; treating a decline as an
approval fails `a_declined_plan_stops_the_work_it_was_holding_back`; gating a
turn with no plan fails `a_turn_with_no_plan_is_not_gated`.

**Loop break value refactored.** Three kinds of pause did not fit a
`(run, response, Option<command>, Option<patch>, stop_reason)` tuple that eight
`break` sites each had to spell out. They now carry a `TurnProposals` struct,
so a fourth pause does not mean editing eight sites with no opinion about it.

**One debt, recorded against Task 12.** `resume_after_plan_decision` exists and
is tested, but nothing in `desktop-shell` calls it yet: `ChatTurnResult`
carries `plan_proposal` and neither `chat_result_json` nor `appendProposals`
knows about it. In the app a gated turn shows the plan as readable prose and
stops — recoverable (the user can start another turn), but with no approve
control. A shell endpoint with no caller would have been dead code dressed as
wiring, so it waits for the task that renders the panel.

---

### Task 12: Plan panel and completion report

**Files:**
- Modify: `crates/desktop-shell/src/lib.rs`, `crates/desktop-shell/static/app.js`
- Test: `crates/desktop-shell/src/lib.rs` tests

**Interfaces:**
- Consumes: `TaskPlan` over a new `plan` SSE event, alongside the existing
  `phase` event.

- [ ] **Step 1: Write the failing test**

A shell test asserting the `plan` SSE event carries steps with status and
evidence, and that a plan with two `in_progress` steps renders the violation
visibly rather than silently picking one.

- [ ] **Step 2: Implement**

Extend `TurnProgress` with `Plan(TaskPlan)`, map it in `turn_progress_event`,
add the `plan` case to `write_sse_event`, and add a `plan(payload)` handler in
`app.js` beside the existing `phase(payload)` handler at the dispatch site.

**Also close Task 11's debt:** `ChatTurnResult::plan_proposal` reaches nothing.
Add `planProposal` to `chat_result_json`, a `/api/resume-plan-stream` route
calling `ChatOrchestrator::resume_after_plan_decision`, and a
`payload.planProposal` branch in `appendProposals` — the shared site, so a plan
proposed after the user approved a command is not silently dropped the way a
patch once was. The card has to offer reorder, retitle, delete and approve
(§5.5), and a deletion aimed at a step that already ran is refused by the
engine, so the card should not offer it.

The completion report distinguishes four outcomes per step — verified complete,
completed unverified, blocked, skipped — and the summary line never says
"complete" for a plan with a blocked step.

- [ ] **Step 3: Verify in the running app**

Static assets are `include_str!`-embedded: rebuild and restart before checking,
or you will be looking at the previous build. Kill by PID, never by name.

- [ ] **Step 4: Run the full gate, then ask before committing**

`node --check crates/desktop-shell/static/app.js` is in the gate and is the one
that gets missed after an `app.js` edit.

Proposed message: `Show the plan, its steps, and what confirms each one`

#### What this task actually did

**The completion report lives in `plan.rs`, not in the UI.** `PlanReport`,
`StepOutcome` and `PlanReport::summary` are pure and tested without a browser.
Writing the summary in `app.js` would have put the sentence "this plan is
complete" in a different place from the rule that decides whether it is — and
those two drift.

`StepOutcome` has five variants, not the four §5.6 names. The fifth,
`Outstanding`, is not an outcome: a turn can end with a step still open (a
token stop, the stop button, a plan held for review), and reporting that step
beside the finished ones claims it reached somewhere it has not. Four
mutations, four tests: an empty plan counting as complete, a blocked step not
stopping completion, an outstanding step not stopping completion, and
`Unverified` collapsing into `Verified` each fail exactly one.

**The panel is sent whole, every time.** `TurnProgress::Plan(TaskPlan)` carries
the entire plan rather than a delta. A client that missed one delta — a
reconnect, a dropped frame — would render a plan that never existed, and the
plan is small enough that there is nothing to save by being clever. The panel
rebuilds in place on each event for the same reason: there is no accumulated
state a missed event could corrupt.

`violatesSingleInProgress` crosses the wire rather than being re-derived in the
frontend, and the panel prints a warning when it is set. A panel that silently
rendered the first of two in-progress steps would conceal the exact invariant
requirement 2 exists to catch.

**Task 11's debt is closed.** `planProposal` on the turn result, a
`/api/resume-plan-stream` route, and a review card in `appendProposals` — the
shared site, so a plan proposed after a command approval is not dropped the way
a patch once was. The card offers reorder, retitle, delete and approve; steps
that already ran are shown but have no controls, because the engine refuses to
delete one and offering the control would offer something that does not work.
Whether a decision counts as a revision is decided by comparing against what
was proposed, not by whether the user touched a control: a reorder and a
reorder back is not a revision, and recording one would put a `plan_revised`
event in the log describing no change.

**Task 7's debt is closed**, and the fix was not the three-line one. A token
stop now has its own row with no button — continuing means raising
`agent_max_task_tokens` first, and a one-click button beside the number that
stopped the spending is the wrong shape of control. The three call sites became
one `markMessageTurnStop`, which is what let the next finding surface.

**A defect found by testing the reload, pre-existing and not mine.** A turn
that dispatched tools writes one assistant message per tool before its answer,
and `renderMessages` marked *every* one of them — so a tool-budget stop already
rendered a duplicate "Tool budget exhausted" row under each. The per-message
dedupe guard inside the helper could not catch it: each row was the first under
its own message. Whatever a turn appends after its answer now hangs off the
turn's last assistant message, which fixes the token row, the tool row, and the
plan panel's anchor at once. Verified in the running app: two assistant
messages, one stop row.

**A gap closed rather than documented.** The panel was live-turn-only, like
`appendContextDisclosure`'s context files — but the completion report is the
part a user reopens a session *for*, and losing it on reload would have made
the report ceremony. `/api/session` now carries each turn's plan.
`SessionStore::read_session_plans` folds every plan in one pass: calling
`read_task_plan` per task would read the whole log once per task, quadratic in
exactly the sessions that are already the longest. It is tested by agreeing
with `read_task_plan` on every plan — the moment it disagrees it has stopped
being an optimisation and become a second implementation.

Absent, not empty, throughout: a task with no plan carries no `plan` field, so
a trivial turn stays panel-free (§5.1).

**Verified in the running app** at 1280×800 against a rebuilt binary on an
isolated data dir and port (static assets are `include_str!`-embedded), driving
the real `app.js`: the panel, the confirmed/unverified/blocked/skipped tones,
the evidence disclosures, the violation warning, the review card's editable and
settled rows, the token-stop row, and the reload join.

---

### Task 13: Harness coverage and documentation

**Files:**
- Modify: `crates/eval-harness/src/metrics.rs`, `crates/eval-harness/src/record.rs`
- Modify: `docs/USER_GUIDE.md`, `docs/TROUBLESHOOTING.md`
- Modify: `docs/specs/21_task_plan_progress_and_budget/proposal.md` (§7)

- [ ] **Step 1: Add plan metrics to the harness**

Steps planned, steps completed, steps completed unverified, steps blocked.
Follow spec 18 requirement 5: every metric carries a value or an explicit
not-applicable marker, and a run with no plan reports `notApplicable`, never a
zero. Use the `count_or_no_data` helpers that already exist rather than adding
a parallel path.

This is the acceptance criterion restated from spec 23 (§3.6): a plan runs end
to end through to a completion report inside the harness, which is Done and
runs in CI.

- [ ] **Step 2: Write the user guide section**

What a plan is, when one appears, how to adjust it, what "completed unverified"
means and why Damaian says it rather than hiding it, and how to set a ceiling.

Say plainly that the ceiling is **per turn**, not per session or per task in the
colloquial sense (§3.1) — a reader who assumes otherwise will set a ceiling that
does not do what they meant, and finding that out from a bill is the wrong way.

- [ ] **Step 3: Write the troubleshooting section**

Where plan events are in the session log, how to read step evidence, and the
difference between `tool_budget_exhausted` and `token_budget_exhausted` — one
means the work needed more rounds, the other that it needed more money.

- [ ] **Step 4: Fill in proposal §7**

It asks for three things by name: the mechanical rule actually used to decide a
turn is non-trivial and how often it produced an unnecessary plan; the default
ceiling if one was chosen and why; and whether any step type ends up with no
available evidence in practice — a large share of unverified steps means the
evidence model is missing a source, not that the work is unobservable.

Record the phase-derivation rule from Task 6 here too.

- [ ] **Step 5: Update the status line and the specs README**

Set the proposal's `Status:` to what is actually true, and update
`docs/specs/README.md` row 21.

- [ ] **Step 6: Run the full gate, then ask before committing**

Proposed message: `Measure plans in the harness and document what a plan means`

---

## Self-Review

Checked against the proposal on 2026-09-11.

**Spec coverage.** Requirements 1–7 map to tasks 2/3 (structure and
persistence), 5 (single in-progress, asserted in Task 2), 6 (phase), 11
(review), 3/10 (persistence and recovery), 1/4/5 (evidence), and 7/8/9
(ceiling). Every §6 acceptance criterion has a test named in a task, including
the four added during this planning pass.

**One deliberate omission.** `Evidence::Findings` is not implemented — spec 22
does not exist to produce the ids it would hold (§3.6). The enum is
`#[non_exhaustive]` and `status_from_evidence`'s match is exhaustive in-crate,
so adding it later is a compile error at the one place that must make a
decision about it. That is the intended behaviour, not an oversight to fix.

**Ordering.** Task 1 is first because every evidence criterion is vacuous
without it, and it is independently valuable: it also repairs spec 18's
`tool_and_model_error_rate`. Tasks 7–9 (the ceiling) do not depend on tasks 2–6
(the plan) except for naming remaining steps in the stop message, so they can be
taken in either order if the ceiling is wanted sooner.
