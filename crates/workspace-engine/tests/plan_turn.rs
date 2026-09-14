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

use workspace_engine::plan::{Evidence, StepStatus, TaskPlan};
use workspace_engine::{
    CancelToken, ChatTurnResult, Config, MockModelAdapter, ModelAdapter, PlanRevisionStep,
    ToolCall, TurnProgress, TurnSink, WorkspaceEngine,
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

/// [`ask`], keeping every plan the turn reported as it went.
fn ask_watching_plans(
    engine: &WorkspaceEngine,
    repo: &Path,
    prompt: &str,
    adapter: &mut dyn ModelAdapter,
) -> (ChatTurnResult, Vec<TaskPlan>) {
    let cancel = CancelToken::new();
    let mut seen: Vec<TaskPlan> = Vec::new();
    let mut on_token = |_token: &str| {};
    let mut on_progress = |event: TurnProgress| {
        if let TurnProgress::Plan(plan) = event {
            seen.push(plan);
        }
    };
    let mut sink = TurnSink {
        on_token: &mut on_token,
        on_progress: &mut on_progress,
        cancel: &cancel,
    };
    let result = engine
        .chat_orchestrator
        .ask_with_session(repo, prompt, &[], None, adapter, &mut sink)
        .expect("the turn should run");
    (result, seen)
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

/// Continues a turn that stopped for plan review, the way the UI's decision
/// would.
fn decide(
    engine: &WorkspaceEngine,
    proposal_id: &str,
    approved: bool,
    revised: Option<Vec<PlanRevisionStep>>,
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
        .resume_after_plan_decision(proposal_id, approved, revised, "tester", adapter, &mut sink)
        .expect("the resumed turn should run")
}

/// A `propose_patch` call that would create one new file.
fn patch_call(path: &str) -> ToolCall {
    call(
        "propose_patch",
        &format!(
            r#"{{"summary":"Add {path}","files":[{{"path":"{path}","content":"pub fn added() {{}}\n"}}]}}"#
        ),
    )
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

// ---------------------------------------------------------------------------
// The review gate. Proposal §5.5.
// ---------------------------------------------------------------------------

#[test]
fn no_mutating_step_runs_before_the_plan_is_approved() {
    // Requirement 4. The model plans, then reaches for a patch; the turn stops
    // and shows the plan instead. A patch proposal here would mean the user's
    // first sight of the plan came *with* the edit already prepared — too late
    // to redirect, which is the whole thing the gate is for.
    let repo = temp_repo("gate");
    let engine = engine_for(&repo);
    let mut adapter = scripted(vec![
        vec![call(
            "propose_plan",
            r#"{"steps":[{"title":"Read the retry helper"},{"title":"Add a bounded backoff"}]}"#,
        )],
        vec![patch_call("src/retry.rs")],
    ]);

    let result = ask(&engine, &repo, "Add retry handling", &mut adapter);

    let proposal = result.plan_proposal.expect("the plan should be put up");
    assert_eq!(proposal.plan.steps.len(), 2);
    assert_eq!(proposal.deferred_action, "Preparing a patch");
    assert!(
        result.patch_proposal.is_none(),
        "the patch must not be prepared before the plan is reviewed"
    );
    assert_eq!(result.task.status.as_str(), "waiting_for_approval");
}

#[test]
fn a_plan_that_only_reads_is_never_put_up_for_approval() {
    // §5.5's other half: a plan whose steps only look at things needs no
    // approval. `ls src` is sandbox-safe, so the turn runs it, finishes the
    // step and answers without ever interrupting. A gate that fired here
    // would be the noise a user learns to click through, which would cost the
    // gate its meaning on the turn that matters.
    let repo = temp_repo("readonly");
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

    let result = ask(&engine, &repo, "What is in src?", &mut adapter);

    assert!(result.plan_proposal.is_none());
    let plan = plan_of(&engine, &result).expect("the turn proposed a plan");
    assert_eq!(
        plan.steps[0].status,
        StepStatus::Completed,
        "the read-only step should have run to completion uninterrupted"
    );
}

#[test]
fn a_turn_with_no_plan_is_not_gated() {
    // The gate reviews a plan. With no plan there is nothing to review, and
    // manufacturing one to have something to approve would put a panel in
    // front of every one-line question (§5.1).
    let repo = temp_repo("noplan");
    let engine = engine_for(&repo);
    let mut adapter = scripted(vec![vec![patch_call("src/retry.rs")]]);

    let result = ask(&engine, &repo, "Add retry handling", &mut adapter);

    assert!(result.plan_proposal.is_none());
    assert!(
        result.patch_proposal.is_some(),
        "an unplanned patch still reaches its own review, as it did before the gate"
    );
}

#[test]
fn approving_the_plan_lets_the_patch_through() {
    let repo = temp_repo("approve");
    let engine = engine_for(&repo);
    let mut adapter = scripted(vec![
        vec![call(
            "propose_plan",
            r#"{"steps":[{"title":"Read the retry helper"},{"title":"Add a bounded backoff"}]}"#,
        )],
        vec![patch_call("src/retry.rs")],
    ]);
    let paused = ask(&engine, &repo, "Add retry handling", &mut adapter);
    let proposal = paused.plan_proposal.expect("the plan should be put up");

    let mut after = scripted(vec![vec![patch_call("src/retry.rs")]]);
    let result = decide(&engine, &proposal.id, true, None, &mut after);

    assert!(
        result.patch_proposal.is_some(),
        "an approved plan should not be asked about again"
    );
    assert!(result.plan_proposal.is_none());
}

#[test]
fn a_declined_plan_stops_the_work_it_was_holding_back() {
    let repo = temp_repo("decline");
    let engine = engine_for(&repo);
    let mut adapter = scripted(vec![
        vec![call(
            "propose_plan",
            r#"{"steps":[{"title":"Read the retry helper"},{"title":"Add a bounded backoff"}]}"#,
        )],
        vec![patch_call("src/retry.rs")],
    ]);
    let paused = ask(&engine, &repo, "Add retry handling", &mut adapter);
    let proposal = paused.plan_proposal.expect("the plan should be put up");

    let mut after = MockModelAdapter::new("I won't make that change.");
    let result = decide(&engine, &proposal.id, false, None, &mut after);

    assert!(result.patch_proposal.is_none());
    assert!(
        !engine
            .session_store
            .read_plan_approved(&result.session.id, &result.task.id)
            .unwrap(),
        "declining must not record an approval; the next mutating step is still gated"
    );
}

#[test]
fn a_revision_is_what_the_work_goes_on_to_follow() {
    // §5.5: the user may delete and retitle steps, and both the original and
    // the revision stay in the log. What runs afterwards is the revision.
    let repo = temp_repo("revise");
    let engine = engine_for(&repo);
    let mut adapter = scripted(vec![
        vec![call(
            "propose_plan",
            r#"{"steps":[{"title":"Read the retry helper"},{"title":"Add a bounded backoff"},{"title":"Rewrite the scheduler"}]}"#,
        )],
        vec![patch_call("src/retry.rs")],
    ]);
    let paused = ask(&engine, &repo, "Add retry handling", &mut adapter);
    let proposal = paused.plan_proposal.expect("the plan should be put up");
    let ids: Vec<String> = proposal
        .plan
        .steps
        .iter()
        .map(|step| step.id.clone())
        .collect();

    // Drop the third step and retitle the second.
    let revision = vec![
        PlanRevisionStep {
            id: ids[0].clone(),
            title: "Read the retry helper".to_string(),
        },
        PlanRevisionStep {
            id: ids[1].clone(),
            title: "Add a backoff capped at 30s".to_string(),
        },
    ];
    let mut after = scripted(vec![vec![patch_call("src/retry.rs")]]);
    let result = decide(&engine, &proposal.id, true, Some(revision), &mut after);

    let plan = plan_of(&engine, &result).expect("the revised plan should read back");
    assert_eq!(plan.steps.len(), 2, "the deleted step is gone");
    assert_eq!(plan.steps[1].title, "Add a backoff capped at 30s");
    assert!(
        log_of(&repo, &result.session.id).contains("plan_revised"),
        "the revision is its own event, so the log holds both versions"
    );
}

#[test]
fn a_revision_may_not_delete_a_step_that_already_ran() {
    // A completed step carries evidence tied to a state of the repository.
    // Honouring a deletion would leave a plan whose history no longer
    // describes what happened — §5.5's own reason for ruling out mid-execution
    // editing. The step survives the revision that omits it.
    let repo = temp_repo("revise-done");
    let engine = engine_for(&repo);
    let mut adapter = scripted(vec![
        vec![call(
            "propose_plan",
            r#"{"steps":[{"title":"List the source"},{"title":"Add a bounded backoff"}]}"#,
        )],
        vec![call(
            "run_command",
            r#"{"command":"ls src","reason":"List"}"#,
        )],
        vec![call("complete_step", "{}")],
        vec![patch_call("src/retry.rs")],
    ]);
    let paused = ask(&engine, &repo, "Add retry handling", &mut adapter);
    let proposal = paused.plan_proposal.expect("the plan should be put up");
    assert_eq!(proposal.plan.steps[0].status, StepStatus::Completed);
    let second = proposal.plan.steps[1].id.clone();

    // A revision naming only the second step: the user is trying to drop the
    // one that already ran.
    let revision = vec![PlanRevisionStep {
        id: second,
        title: "Add a backoff capped at 30s".to_string(),
    }];
    let mut after = scripted(vec![vec![patch_call("src/retry.rs")]]);
    let result = decide(&engine, &proposal.id, true, Some(revision), &mut after);

    let plan = plan_of(&engine, &result).expect("the revised plan should read back");
    assert_eq!(plan.steps.len(), 2, "the completed step is not deletable");
    assert_eq!(plan.steps[0].title, "List the source");
    assert_eq!(plan.steps[0].status, StepStatus::Completed);
    assert!(
        !plan.steps[0].evidence.is_empty(),
        "and it still carries what confirmed it"
    );
}

fn log_of(repo: &Path, session_id: &str) -> String {
    fs::read_to_string(
        repo.join(".damaian")
            .join("sessions")
            .join(format!("{session_id}.jsonl")),
    )
    .expect("the session log should exist")
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

// ---------------------------------------------------------------------------
// The panel's side channel. Proposal §5.6.
// ---------------------------------------------------------------------------

#[test]
fn the_turn_reports_the_plan_as_it_advances() {
    // The panel is driven by `TurnProgress::Plan`, and without these emissions
    // it would stay empty for the whole turn and then appear complete — the
    // one shape of progress reporting worse than none.
    let repo = temp_repo("progress");
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

    let (result, reported) = ask_watching_plans(&engine, &repo, "What is in src?", &mut adapter);

    // Once when the plan is created, once when the step hands off.
    assert_eq!(
        reported.len(),
        2,
        "expected a report on creation and on the handoff, got {reported:?}"
    );
    assert_eq!(reported[0].steps[0].status, StepStatus::InProgress);
    assert!(
        reported[0].steps[0].evidence.is_empty(),
        "nothing has been observed yet when the plan is first shown"
    );
    assert_eq!(reported[1].steps[0].status, StepStatus::Completed);
    assert_eq!(
        reported[1].steps[1].status,
        StepStatus::InProgress,
        "the handoff is reported once, after both writes — never with no step running"
    );
    // And every report matches what the log would replay.
    assert_eq!(
        reported.last(),
        plan_of(&engine, &result).as_ref(),
        "the panel and the log must not disagree about the same plan"
    );
}

#[test]
fn a_turn_that_arrives_with_a_plan_reports_it_before_doing_anything() {
    // A resumed turn — after a token stop, or after the user approved the plan
    // — already has one in hand. Without this the panel stays empty until the
    // first `complete_step`, which is the longest stretch of the turn.
    let repo = temp_repo("progress-resumed");
    let engine = engine_for(&repo);
    let mut first = scripted(vec![
        vec![call(
            "propose_plan",
            r#"{"steps":[{"title":"List the source"},{"title":"Report"}]}"#,
        )],
        vec![patch_call("src/retry.rs")],
    ]);
    let paused = ask(&engine, &repo, "Add retry handling", &mut first);
    let proposal = paused.plan_proposal.expect("the plan should be put up");

    let cancel = CancelToken::new();
    let mut seen: Vec<TaskPlan> = Vec::new();
    let mut on_token = |_token: &str| {};
    let mut on_progress = |event: TurnProgress| {
        if let TurnProgress::Plan(plan) = event {
            seen.push(plan);
        }
    };
    let mut sink = TurnSink {
        on_token: &mut on_token,
        on_progress: &mut on_progress,
        cancel: &cancel,
    };
    let mut after = scripted(vec![vec![patch_call("src/retry.rs")]]);
    engine
        .chat_orchestrator
        .resume_after_plan_decision(&proposal.id, true, None, "tester", &mut after, &mut sink)
        .expect("the resumed turn should run");

    assert!(
        !seen.is_empty(),
        "a resumed turn must show its plan before it starts working"
    );
    assert_eq!(seen[0].steps.len(), 2);
    assert_eq!(seen[0].steps[0].title, "List the source");
}

#[test]
fn a_turn_without_a_plan_reports_none() {
    // The panel appears only when there is a plan. A turn that answered a
    // question must not emit an empty one for the frontend to render (§5.1).
    let repo = temp_repo("progress-trivial");
    let engine = engine_for(&repo);
    let mut adapter = MockModelAdapter::new("It defines main.");

    let (_, reported) = ask_watching_plans(&engine, &repo, "What does src/a.rs do?", &mut adapter);

    assert!(reported.is_empty(), "got {reported:?}");
}

#[test]
fn a_step_that_read_a_file_is_confirmed_by_the_read() {
    // §7 asks whether any step type ends up with no available evidence. A
    // reading step did: `Evidence::FileRead` existed in the enum and nothing
    // ever built one, so "read the retry helper" reported *completed
    // unverified* even though Damaian had the path and the content hash in
    // hand. The evidence was available and simply not recorded, which is a
    // missing source rather than genuinely unobservable work.
    let repo = temp_repo("read-evidence");
    let engine = engine_for(&repo);
    let mut adapter = scripted(vec![
        vec![call(
            "propose_plan",
            r#"{"steps":[{"title":"Read the source"},{"title":"Report"}]}"#,
        )],
        vec![call("read_file", r#"{"path":"src/a.rs"}"#)],
        vec![call("complete_step", "{}")],
    ]);

    let result = ask(&engine, &repo, "What does src/a.rs do?", &mut adapter);
    let plan = plan_of(&engine, &result).expect("the turn proposed a plan");

    assert_eq!(plan.steps[0].status, StepStatus::Completed);
    assert!(
        !plan.steps[0].is_unverified(),
        "the read is observable, so the step is confirmed rather than unverified"
    );
    match plan.steps[0].evidence.as_slice() {
        [Evidence::FileRead { path, hash }] => {
            assert_eq!(path, "src/a.rs");
            assert!(!hash.is_empty(), "the hash says *which* content was read");
        }
        other => panic!("expected one file-read evidence, got {other:?}"),
    }
}

#[test]
fn a_file_that_could_not_be_read_confirms_nothing() {
    // The failure direction, and the one that matters: a read that was refused
    // or missing must not mint evidence. Recording it would confirm a step
    // with the fact that Damaian *tried* to look at something.
    let repo = temp_repo("read-evidence-failed");
    let engine = engine_for(&repo);
    let mut adapter = scripted(vec![
        vec![call(
            "propose_plan",
            r#"{"steps":[{"title":"Read the source"},{"title":"Report"}]}"#,
        )],
        vec![call("read_file", r#"{"path":"src/no-such-file.rs"}"#)],
        vec![call("complete_step", "{}")],
    ]);

    let result = ask(&engine, &repo, "What does it do?", &mut adapter);
    let plan = plan_of(&engine, &result).expect("the turn proposed a plan");

    assert!(
        plan.steps[0].evidence.is_empty(),
        "a failed read is not evidence of anything, got {:?}",
        plan.steps[0].evidence
    );
    assert!(plan.steps[0].is_unverified());
}
