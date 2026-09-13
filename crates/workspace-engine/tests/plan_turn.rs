//! The plan as a turn actually produces it, per
//! `docs/specs/21_task_plan_progress_and_budget/proposal.md` §5.1 and §5.3.
//!
//! `plan.rs` covers the types and the store on their own. These tests go
//! through the chat orchestrator, because the guarantees that matter are about
//! runs: that a trivial turn gets no plan, that exactly one step is in progress
//! at a time, and — the one this spec exists for — that a step's status comes
//! from what the engine observed and not from what the model said about it.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use workspace_engine::plan::{StepStatus, TaskPlan};
use workspace_engine::{
    CancelToken, ChatTurnResult, Config, MockModelAdapter, ModelAdapter, ToolCall, TurnProgress,
    TurnSink, WorkspaceEngine,
};

static COUNTER: AtomicU64 = AtomicU64::new(1);

fn temp_repo(name: &str) -> PathBuf {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock should work")
        .as_nanos();
    let repo = std::env::temp_dir().join(format!(
        "damaian-plan-turn-{name}-{now}-{}",
        COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir_all(repo.join("src")).expect("repository should be created");
    fs::write(repo.join("src/a.rs"), "fn main() {}\n").expect("a file to read");
    repo
}

fn engine_for(repo: &Path) -> WorkspaceEngine {
    WorkspaceEngine::new(Config {
        data_dir: repo.join(".damaian"),
        ..Config::default()
    })
}

fn ask(
    engine: &WorkspaceEngine,
    repo: &Path,
    prompt: &str,
    adapter: &mut dyn ModelAdapter,
) -> ChatTurnResult {
    let cancel = CancelToken::new();
    let mut on_token = |_token: &str| {};
    let mut on_progress = |_event: TurnProgress| {};
    let mut sink = TurnSink {
        on_token: &mut on_token,
        on_progress: &mut on_progress,
        cancel: &cancel,
    };
    engine
        .chat_orchestrator
        .ask_with_session(repo, prompt, &[], None, adapter, &mut sink)
        .expect("the turn should run")
}

fn call(name: &str, arguments_json: &str) -> ToolCall {
    ToolCall {
        id: format!("call_{name}"),
        name: name.to_string(),
        arguments_json: arguments_json.to_string(),
    }
}

/// Drives a turn through a scripted sequence of tool-call rounds, ending with
/// a plain answer so the loop terminates rather than running to the round
/// limit.
fn scripted(rounds: Vec<Vec<ToolCall>>) -> MockModelAdapter {
    let mut responses: Vec<String> = rounds.iter().map(|_| String::new()).collect();
    let mut calls = rounds;
    responses.push("Done.".to_string());
    calls.push(Vec::new());
    MockModelAdapter::new_sequence_with_tool_calls(responses, calls)
}

fn plan_of(engine: &WorkspaceEngine, result: &ChatTurnResult) -> Option<TaskPlan> {
    engine
        .session_store
        .read_task_plan(&result.session.id, &result.task.id)
        .expect("the plan should read")
}

#[test]
fn a_trivial_turn_gets_no_plan() {
    // §5.1: a one-step plan is ceremony. Spec 08's turn states already cover a
    // single question, and a plan panel appearing for one would train the user
    // to ignore it.
    let repo = temp_repo("trivial");
    let engine = engine_for(&repo);
    let mut adapter = MockModelAdapter::new("It defines main.");

    let result = ask(&engine, &repo, "What does src/a.rs do?", &mut adapter);

    assert!(plan_of(&engine, &result).is_none());
}

#[test]
fn a_turn_that_proposes_a_plan_gets_one_with_its_first_step_running() {
    let repo = temp_repo("proposed");
    let engine = engine_for(&repo);
    let mut adapter = scripted(vec![vec![call(
        "propose_plan",
        r#"{"steps":[{"title":"Read the retry helper"},{"title":"Add a bounded backoff"}]}"#,
    )]]);

    let result = ask(&engine, &repo, "Add retry handling", &mut adapter);
    let plan = plan_of(&engine, &result).expect("the turn proposed a plan");

    assert_eq!(plan.steps.len(), 2);
    assert_eq!(plan.steps[0].title, "Read the retry helper");
    assert_eq!(plan.steps[0].status, StepStatus::InProgress);
    assert!(plan.steps[0].started_at_ms.is_some());
    // Requirement 2: the second step waits its turn.
    assert_eq!(plan.steps[1].status, StepStatus::Pending);
    assert!(!plan.violates_single_in_progress());
}

#[test]
fn a_step_whose_command_failed_is_blocked_however_the_model_describes_it() {
    // The requirement this spec exists for. The model completes the step and
    // says so in prose; the command it ran exited non-zero. Evidence wins.
    let repo = temp_repo("blocked");
    let engine = engine_for(&repo);
    let mut adapter = scripted(vec![
        vec![call(
            "propose_plan",
            r#"{"steps":[{"title":"Check the folder"},{"title":"Report"}]}"#,
        )],
        vec![call(
            "run_command",
            r#"{"command":"ls no-such-directory","reason":"List it"}"#,
        )],
        vec![call("complete_step", "{}")],
    ]);

    let result = ask(&engine, &repo, "Check the folder", &mut adapter);
    let plan = plan_of(&engine, &result).expect("the turn proposed a plan");

    assert_eq!(
        plan.steps[0].status,
        StepStatus::Blocked,
        "a non-zero exit blocks the step no matter what the model claims"
    );
    assert_eq!(plan.steps[0].evidence.len(), 1, "the exit is recorded");
    assert!(
        !plan.steps[0].is_unverified(),
        "blocked is not completed-unverified; that would launder a failure into a soft pass"
    );
}

#[test]
fn a_step_whose_command_passed_completes_and_carries_its_evidence() {
    let repo = temp_repo("passed");
    let engine = engine_for(&repo);
    let mut adapter = scripted(vec![
        vec![call(
            "propose_plan",
            r#"{"steps":[{"title":"List the source"},{"title":"Report"}]}"#,
        )],
        vec![call(
            "run_command",
            r#"{"command":"ls src","reason":"List"}"#,
        )],
        vec![call("complete_step", "{}")],
    ]);

    let result = ask(&engine, &repo, "List the source", &mut adapter);
    let plan = plan_of(&engine, &result).expect("the turn proposed a plan");

    assert_eq!(plan.steps[0].status, StepStatus::Completed);
    assert!(!plan.steps[0].is_unverified());
    assert!(plan.steps[0].completed_at_ms.is_some());
    // And the next step took over, still only one at a time.
    assert_eq!(plan.steps[1].status, StepStatus::InProgress);
    assert!(!plan.violates_single_in_progress());
}

#[test]
fn a_step_with_no_observable_work_completes_unverified() {
    // §5.3's last row. "Understand the existing retry logic" has nothing
    // observable behind it, and inventing evidence would be worse than saying
    // so plainly.
    let repo = temp_repo("unverified");
    let engine = engine_for(&repo);
    let mut adapter = scripted(vec![
        vec![call(
            "propose_plan",
            r#"{"steps":[{"title":"Understand the retry logic"},{"title":"Report"}]}"#,
        )],
        vec![call("complete_step", "{}")],
    ]);

    let result = ask(&engine, &repo, "Understand the retry logic", &mut adapter);
    let plan = plan_of(&engine, &result).expect("the turn proposed a plan");

    assert_eq!(plan.steps[0].status, StepStatus::Completed);
    assert!(
        plan.steps[0].is_unverified(),
        "nothing observable confirms this step, and the report must say so"
    );
}

#[test]
fn a_second_plan_in_one_turn_does_not_replace_the_first() {
    // A plan is proposed once per turn. Silently replacing it would discard
    // steps that already carry evidence tied to a state of the repository —
    // the same reason §5.5 rules out editing a plan mid-execution.
    let repo = temp_repo("second");
    let engine = engine_for(&repo);
    let mut adapter = scripted(vec![
        vec![call(
            "propose_plan",
            r#"{"steps":[{"title":"First"},{"title":"Second"}]}"#,
        )],
        vec![call(
            "propose_plan",
            r#"{"steps":[{"title":"Replacement"}]}"#,
        )],
    ]);

    let result = ask(&engine, &repo, "Do the thing", &mut adapter);
    let plan = plan_of(&engine, &result).expect("the turn proposed a plan");

    assert_eq!(plan.steps.len(), 2);
    assert_eq!(plan.steps[0].title, "First");
}

#[test]
fn no_point_in_the_log_ever_has_two_steps_in_progress() {
    // Requirement 2 is about runs, and asserting it only on the final plan
    // would miss a transient double — which is exactly the shape the bug would
    // take, since the handoff writes the closing step and the opening one as
    // two separate appends. Replay the log prefix by prefix instead.
    let repo = temp_repo("invariant");
    let engine = engine_for(&repo);
    let mut adapter = scripted(vec![
        vec![call(
            "propose_plan",
            r#"{"steps":[{"title":"One"},{"title":"Two"},{"title":"Three"}]}"#,
        )],
        vec![call(
            "run_command",
            r#"{"command":"ls src","reason":"List"}"#,
        )],
        vec![call("complete_step", "{}")],
        vec![call("run_command", r#"{"command":"ls .","reason":"List"}"#)],
        vec![call("complete_step", "{}")],
    ]);

    let result = ask(&engine, &repo, "Do three things", &mut adapter);
    let log = fs::read_to_string(
        repo.join(".damaian")
            .join("sessions")
            .join(format!("{}.jsonl", result.session.id)),
    )
    .expect("the session log should exist");

    let lines: Vec<&str> = log.lines().collect();
    let mut checked = 0;
    for end in 1..=lines.len() {
        let prefix = lines[..end].join("\n");
        let Some(plan) = replay(&prefix, &result.task.id) else {
            continue;
        };
        checked += 1;
        assert!(
            !plan.violates_single_in_progress(),
            "two steps in progress after {end} events: {:?}",
            plan.steps
                .iter()
                .map(|step| (&step.id, step.status))
                .collect::<Vec<_>>()
        );
    }
    assert!(checked > 3, "the replay should have seen the plan evolve");
}

/// Folds a log prefix the way `SessionStore::read_task_plan` does, so a
/// half-written handoff is visible rather than smoothed over.
fn replay(log: &str, task_id: &str) -> Option<TaskPlan> {
    let mut plan: Option<TaskPlan> = None;
    for line in log.lines() {
        let Ok(event) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        let kind = event.get("eventType").and_then(|value| value.as_str());
        let payload = event.get("payload")?.clone();
        match kind {
            Some("plan_created") | Some("plan_revised") => {
                if let Ok(created) = serde_json::from_value::<TaskPlan>(payload)
                    && created.task_id == task_id
                {
                    plan = Some(created);
                }
            }
            Some("plan_step_updated") => {
                let current = plan.as_mut()?;
                if payload.get("taskId").and_then(|value| value.as_str()) != Some(task_id) {
                    continue;
                }
                if let Some(updated) = payload.get("step").cloned().and_then(|value| {
                    serde_json::from_value::<workspace_engine::plan::PlanStep>(value).ok()
                }) && let Some(existing) =
                    current.steps.iter_mut().find(|step| step.id == updated.id)
                {
                    *existing = updated;
                }
            }
            _ => {}
        }
    }
    plan
}
