//! The plan as a turn actually produces it, per
//! `docs/specs/21_task_plan_progress_and_budget/proposal.md` §5.1 and §5.3.
//!
//! `plan.rs` covers the types and the store on their own. These tests go
//! through the chat orchestrator, because the guarantees that matter are about
//! runs: that a trivial turn gets no plan, that exactly one step is in progress
//! at a time, and — the one this spec exists for — that a step's status comes
//! from what the engine observed and not from what the model said about it.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use workspace_engine::plan::{Evidence, StepStatus, TaskPlan};
use workspace_engine::{
    CancelToken, ChatTurnResult, CommandPurpose, Config, MockModelAdapter, ModelAdapter, ModelRun,
    PlanRevisionStep, TaskStatus, TokenUsage, ToolCall, TurnProgress, TurnSink, WorkspaceEngine,
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
        // Throwaway repository: a watcher would only cost FSEvents registration.
        enable_index_watcher: false,
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
        steering: None,
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
        steering: None,
    };
    let result = engine
        .chat_orchestrator
        .ask_with_session(repo, prompt, &[], None, adapter, &mut sink)
        .expect("the turn should run");
    (result, seen)
}

/// [`ask`], keeping every plan the turn reported as *finished*.
fn ask_watching_finished(
    engine: &WorkspaceEngine,
    repo: &Path,
    prompt: &str,
    adapter: &mut dyn ModelAdapter,
) -> (ChatTurnResult, Vec<TaskPlan>) {
    let cancel = CancelToken::new();
    let mut finished: Vec<TaskPlan> = Vec::new();
    let mut on_token = |_token: &str| {};
    let mut on_progress = |event: TurnProgress| {
        if let TurnProgress::PlanFinished(plan) = event {
            finished.push(plan);
        }
    };
    let mut sink = TurnSink {
        on_token: &mut on_token,
        on_progress: &mut on_progress,
        cancel: &cancel,
        steering: None,
    };
    let result = engine
        .chat_orchestrator
        .ask_with_session(repo, prompt, &[], None, adapter, &mut sink)
        .expect("the turn should run");
    (result, finished)
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
        steering: None,
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
    // A failed `ls` is exploration, which records no finding (spec 63), so
    // the failing check here is a repository script, run without approval.
    fs::write(repo.join("check"), "#!/bin/sh\nexec ls no-such-directory\n").unwrap();
    fs::set_permissions(repo.join("check"), fs::Permissions::from_mode(0o755)).unwrap();
    let engine = WorkspaceEngine::new(Config {
        command_allowlist: vec!["./check".to_string()],
        data_dir: repo.join(".damaian"),
        enable_index_watcher: false,
        ..Config::default()
    });
    let mut adapter = scripted(vec![
        vec![call(
            "propose_plan",
            r#"{"steps":[{"title":"Check the folder"},{"title":"Report"}]}"#,
        )],
        vec![call(
            "run_command",
            r#"{"command":"./check","reason":"Check it"}"#,
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
    // The exit is recorded, and since spec 22 Task 8, so are the findings
    // the failed command produced (spec 22 `context.md` §14).
    assert!(
        matches!(
            plan.steps[0].evidence.as_slice(),
            [Evidence::CommandExit { .. }, Evidence::Findings { .. }]
        ),
        "{:?}",
        plan.steps[0].evidence
    );
    assert!(
        !plan.steps[0].is_unverified(),
        "blocked is not completed-unverified; that would launder a failure into a soft pass"
    );
}

/// Found in a session on 2026-10-26: after a step blocked, a second
/// `complete_step` found no step in progress, left its status at the default
/// `Completed`, and started the next pending step. A blocked step must not
/// hand off, however many times the model asks.
#[test]
fn a_second_complete_step_after_a_block_is_refused_and_starts_nothing() {
    let repo = temp_repo("blocked-twice");
    fs::write(repo.join("check"), "#!/bin/sh\nexec ls no-such-directory\n").unwrap();
    fs::set_permissions(repo.join("check"), fs::Permissions::from_mode(0o755)).unwrap();
    let engine = WorkspaceEngine::new(Config {
        command_allowlist: vec!["./check".to_string()],
        data_dir: repo.join(".damaian"),
        enable_index_watcher: false,
        ..Config::default()
    });
    let mut adapter = scripted(vec![
        vec![call(
            "propose_plan",
            r#"{"steps":[{"title":"A"},{"title":"B"},{"title":"C"}]}"#,
        )],
        vec![call("complete_step", "{}")],
        vec![call(
            "run_command",
            r#"{"command":"./check","reason":"Check it"}"#,
        )],
        vec![call("complete_step", "{}")],
        // The call that used to fabricate a completion and start C.
        vec![call("complete_step", "{}")],
    ]);

    let result = ask(&engine, &repo, "Do it", &mut adapter);
    let plan = plan_of(&engine, &result).expect("the turn proposed a plan");

    assert_eq!(plan.steps[0].status, StepStatus::Completed);
    assert_eq!(plan.steps[1].status, StepStatus::Blocked);
    assert_eq!(
        plan.steps[2].status,
        StepStatus::Pending,
        "a blocked step must not hand off, even after another complete_step"
    );
    assert!(!plan.violates_single_in_progress());

    let messages = engine
        .session_store
        .read_messages(&result.session.id)
        .expect("messages should read");
    assert!(
        messages
            .iter()
            .any(|message| message.content.contains("No step is in progress")),
        "the refusal should say why: {:?}",
        messages
            .iter()
            .map(|message| message.content.as_str())
            .collect::<Vec<_>>()
    );
    assert!(
        !messages
            .iter()
            .any(|message| message.content == "Finished: "),
        "no empty completion may be recorded"
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
fn a_search_that_matched_nothing_does_not_block_its_step() {
    // Spec 63 §6. `grep` exits 1 when nothing matches. That is the answer to
    // the question the step asked, not a failure of the step, so the exit is
    // kept as evidence and the plan moves on.
    let repo = temp_repo("search-no-match");
    // `grep` asks for approval by default, which would raise the plan review.
    // Allowlisting the exact command keeps the turn to one call, and what the
    // policy decided is not under test here.
    let engine = WorkspaceEngine::new(Config {
        command_allowlist: vec!["grep -rn nothing-matches src".to_string()],
        data_dir: repo.join(".damaian"),
        enable_index_watcher: false,
        ..Config::default()
    });
    let mut adapter = scripted(vec![
        vec![call(
            "propose_plan",
            r#"{"steps":[{"title":"Look for callers"},{"title":"Report"}]}"#,
        )],
        vec![call(
            "run_command",
            r#"{"command":"grep -rn nothing-matches src","reason":"Search"}"#,
        )],
        vec![call("complete_step", "{}")],
    ]);

    let result = ask(&engine, &repo, "Look for callers", &mut adapter);
    let plan = plan_of(&engine, &result).expect("the turn proposed a plan");

    assert_eq!(plan.steps[0].status, StepStatus::Completed);
    // The exit stays evidence, with its purpose, and no finding beside it.
    assert!(
        matches!(
            plan.steps[0].evidence.as_slice(),
            [Evidence::CommandExit {
                exit_code: Some(1),
                purpose: CommandPurpose::Exploration,
                ..
            }]
        ),
        "{:?}",
        plan.steps[0].evidence
    );
    assert_eq!(plan.steps[1].status, StepStatus::InProgress);
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
fn the_turn_reports_its_plan_as_finished_when_it_ends() {
    // The final plan event is what lets the panel stop calling an open step
    // "running" (spec 66 §5.4). It carries the plan whole, so evidence
    // attached at the end is in it.
    let repo = temp_repo("plan-finished");
    let engine = engine_for(&repo);
    let mut adapter = scripted(vec![
        vec![plan_call(&["Read the source", "Report"])],
        vec![call("read_file", r#"{"path":"src/a.rs"}"#)],
        vec![call("complete_step", "{}")],
        vec![call("complete_step", "{}")],
    ]);

    let (_, finished) = ask_watching_finished(&engine, &repo, "Read it", &mut adapter);

    assert_eq!(finished.len(), 1, "one final event per true end");
    assert_eq!(finished[0].steps[0].status, StepStatus::Completed);
    assert_eq!(finished[0].steps[1].status, StepStatus::Completed);
}

#[test]
fn a_pause_reports_no_finished_plan() {
    let repo = temp_repo("plan-not-finished-on-pause");
    let engine = engine_for(&repo);
    let mut adapter = scripted(vec![
        vec![plan_call(&["Read the source", "Report"])],
        vec![patch_call("src/retry.rs")],
    ]);

    let (paused, finished) = ask_watching_finished(&engine, &repo, "Edit it", &mut adapter);

    assert!(paused.plan_proposal.is_some(), "the turn paused");
    assert!(
        finished.is_empty(),
        "a pause is not a turn end: {finished:?}"
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
        steering: None,
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

// ---------------------------------------------------------------------------
// A plan left open when the turn ends (spec 66).
// ---------------------------------------------------------------------------

/// Every audit event of one type, parsed, in order.
fn audit_events(repo: &Path, event_type: &str) -> Vec<serde_json::Value> {
    fs::read_to_string(repo.join(".damaian/audit/events.jsonl"))
        .unwrap_or_default()
        .lines()
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
        .filter(|event| event.get("eventType").and_then(|value| value.as_str()) == Some(event_type))
        .collect()
}

/// One `propose_plan` call with the given step titles.
fn plan_call(titles: &[&str]) -> ToolCall {
    let steps = titles
        .iter()
        .map(|title| format!(r#"{{"title":"{title}"}}"#))
        .collect::<Vec<_>>()
        .join(",");
    call("propose_plan", &format!(r#"{{"steps":[{steps}]}}"#))
}

/// A turn whose model answers with the given text and makes no calls.
fn answer_only(answer: &str) -> MockModelAdapter {
    MockModelAdapter::new(answer)
}

/// Resumes a turn paused on a plan review, approving it.
fn approve_plan(
    engine: &WorkspaceEngine,
    proposal_id: &str,
    adapter: &mut dyn ModelAdapter,
) -> ChatTurnResult {
    decide(engine, proposal_id, true, None, adapter)
}

/// Resumes a turn paused on a command approval.
fn approve_command(
    engine: &WorkspaceEngine,
    proposal_id: &str,
    adapter: &mut dyn ModelAdapter,
) -> ChatTurnResult {
    let cancel = CancelToken::new();
    let mut on_token = |_token: &str| {};
    let mut on_progress = |_event: TurnProgress| {};
    let mut sink = TurnSink {
        on_token: &mut on_token,
        on_progress: &mut on_progress,
        cancel: &cancel,
        steering: None,
    };
    engine
        .chat_orchestrator
        .resume_after_command_decision(proposal_id, true, "tester", adapter, &mut sink)
        .expect("the command resume should run")
}

/// The pending turn's saved JSON, so a test can see what a pause carried.
fn pending_json(repo: &Path, proposal_id: &str) -> serde_json::Value {
    let text = fs::read_to_string(
        repo.join(".damaian/chat/pending")
            .join(format!("{proposal_id}.json")),
    )
    .expect("the pending turn should be on disk");
    serde_json::from_str(&text).expect("the pending turn should be JSON")
}

#[test]
fn an_answer_with_open_steps_gets_one_reminder_and_closes_after_it() {
    let repo = temp_repo("reminder-closes");
    let engine = engine_for(&repo);
    let mut adapter = MockModelAdapter::new_sequence_with_tool_calls(
        vec![
            String::new(),
            String::new(),
            "The report is written.".to_string(),
            String::new(),
            "Done.".to_string(),
        ],
        vec![
            vec![plan_call(&["Collect statuses", "Write the report"])],
            vec![call("read_file", r#"{"path":"src/a.rs"}"#)],
            Vec::new(),
            vec![call("complete_step", "{}"), call("complete_step", "{}")],
            Vec::new(),
        ],
    );

    let result = ask(&engine, &repo, "Write the report", &mut adapter);
    let plan = plan_of(&engine, &result).expect("the turn proposed a plan");

    let reminders = audit_events(&repo, "plan_reminder_sent");
    assert_eq!(reminders.len(), 1, "exactly one reminder");
    assert_eq!(reminders[0]["openSteps"].as_str(), Some("2"));

    // The request after the answer carries the reminder as its last message — a
    // user message, so it sits where the model reads it.
    let reminder_request = adapter
        .requests
        .iter()
        .find(|request| {
            request
                .messages
                .last()
                .is_some_and(|message| message.content.contains("Your plan still has open steps"))
        })
        .expect("one request carried the reminder");
    assert_eq!(
        reminder_request.messages.last().unwrap().role,
        "user",
        "the reminder is a user message, not a system one"
    );

    assert_eq!(result.response, "The report is written.");
    let messages = engine
        .session_store
        .read_messages(&result.session.id)
        .expect("messages should read");
    assert!(
        messages
            .iter()
            .any(|message| message.role == "assistant"
                && message.content == "The report is written.")
    );
    assert!(
        messages
            .iter()
            .any(|message| message.kind.as_deref() == Some("plan_reminder")),
        "the reminder is tagged in the session log"
    );
    let last_assistant = messages
        .iter()
        .rev()
        .find(|message| message.role == "assistant")
        .expect("an assistant message");
    assert_eq!(last_assistant.content, "Done.");

    assert_eq!(plan.steps[0].status, StepStatus::Completed);
    assert_eq!(plan.steps[1].status, StepStatus::Completed);
    assert!(
        plan.steps[0]
            .evidence
            .iter()
            .any(|item| matches!(item, Evidence::FileRead { path, .. } if path == "src/a.rs")),
        "the read before the reminder still reached the closed step: {:?}",
        plan.steps[0].evidence
    );
}

#[test]
fn a_second_answer_ends_the_turn_with_the_steps_still_open() {
    let repo = temp_repo("reminder-open");
    let engine = engine_for(&repo);
    let mut adapter = MockModelAdapter::new_sequence_with_tool_calls(
        vec![
            String::new(),
            String::new(),
            "The report is written.".to_string(),
            "Still working.".to_string(),
        ],
        vec![
            vec![plan_call(&["Collect statuses", "Write the report"])],
            vec![call("read_file", r#"{"path":"src/a.rs"}"#)],
            Vec::new(),
            Vec::new(),
        ],
    );

    let result = ask(&engine, &repo, "Write the report", &mut adapter);
    let plan = plan_of(&engine, &result).expect("the turn proposed a plan");

    assert_eq!(audit_events(&repo, "plan_reminder_sent").len(), 1);
    assert_eq!(plan.steps[0].status, StepStatus::InProgress);
    assert_eq!(plan.steps[1].status, StepStatus::Pending);
    assert_eq!(result.response, "The report is written.");
    let messages = engine
        .session_store
        .read_messages(&result.session.id)
        .expect("messages should read");
    let last_assistant = messages
        .iter()
        .rev()
        .find(|message| message.role == "assistant")
        .expect("an assistant message");
    assert_eq!(last_assistant.content, "Still working.");

    let left_open = audit_events(&repo, "plan_left_open");
    assert_eq!(left_open.len(), 1);
    assert_eq!(left_open[0]["openSteps"].as_str(), Some("2"));
    assert_eq!(left_open[0]["reminderSent"].as_str(), Some("true"));
    assert_eq!(left_open[0]["stopReason"].as_str(), Some("answered"));
    assert!(
        plan.steps[0]
            .evidence
            .iter()
            .any(|item| matches!(item, Evidence::FileRead { .. })),
        "the open step keeps what it ran: {:?}",
        plan.steps[0].evidence
    );
}

#[test]
fn a_closed_plan_gets_no_reminder() {
    let repo = temp_repo("reminder-closed-plan");
    let engine = engine_for(&repo);
    let mut adapter = scripted(vec![
        vec![plan_call(&["Read the source", "Report"])],
        vec![call("read_file", r#"{"path":"src/a.rs"}"#)],
        vec![call("complete_step", "{}")],
        vec![call("complete_step", "{}")],
    ]);

    let result = ask(&engine, &repo, "Read the source", &mut adapter);
    let plan = plan_of(&engine, &result).expect("the turn proposed a plan");

    assert!(audit_events(&repo, "plan_reminder_sent").is_empty());
    assert_eq!(plan.steps[0].status, StepStatus::Completed);
    assert_eq!(plan.steps[1].status, StepStatus::Completed);
    assert_eq!(adapter.requests.len(), 5, "no extra call was spent");
}

#[test]
fn a_plan_with_only_a_blocked_step_gets_no_reminder() {
    let repo = temp_repo("reminder-blocked");
    fs::write(repo.join("check"), "#!/bin/sh\nexec ls no-such-directory\n").unwrap();
    fs::set_permissions(repo.join("check"), fs::Permissions::from_mode(0o755)).unwrap();
    let engine = WorkspaceEngine::new(Config {
        command_allowlist: vec!["./check".to_string()],
        data_dir: repo.join(".damaian"),
        enable_index_watcher: false,
        ..Config::default()
    });
    let mut adapter = scripted(vec![
        vec![plan_call(&["Write the report", "Check the folder"])],
        vec![call("complete_step", "{}")],
        vec![call(
            "run_command",
            r#"{"command":"./check","reason":"Check it"}"#,
        )],
        vec![call("complete_step", "{}")],
    ]);

    let result = ask(&engine, &repo, "Check the folder", &mut adapter);
    let plan = plan_of(&engine, &result).expect("the plan reads");

    // Completed and blocked are both terminal, so nothing is open. The model
    // was already told the step blocked; a reminder would ask it to close work
    // that failed.
    assert_eq!(plan.steps[0].status, StepStatus::Completed);
    assert_eq!(plan.steps[1].status, StepStatus::Blocked);
    assert!(audit_events(&repo, "plan_reminder_sent").is_empty());
}

#[test]
fn a_planless_turn_gets_no_reminder() {
    let repo = temp_repo("reminder-planless");
    let engine = engine_for(&repo);
    let mut adapter = answer_only("It defines main.");

    let result = ask(&engine, &repo, "What does src/a.rs do?", &mut adapter);

    assert_eq!(result.response, "It defines main.");
    assert!(audit_events(&repo, "plan_reminder_sent").is_empty());
    assert!(audit_events(&repo, "plan_left_open").is_empty());
}

#[test]
fn a_declined_plan_is_marked_skipped_and_never_reminded() {
    let repo = temp_repo("reminder-declined");
    let engine = engine_for(&repo);
    let mut first = scripted(vec![
        vec![plan_call(&["Read the source", "Write the report"])],
        vec![patch_call("src/retry.rs")],
    ]);
    let paused = ask(&engine, &repo, "Add retry handling", &mut first);
    let proposal = paused.plan_proposal.expect("the plan is put up for review");

    let mut after = answer_only("I did not change anything.");
    let result = decide(&engine, &proposal.id, false, None, &mut after);
    let plan = plan_of(&engine, &result).expect("the plan still reads");

    assert_eq!(plan.steps[0].status, StepStatus::Skipped);
    assert_eq!(plan.steps[1].status, StepStatus::Skipped);
    assert!(audit_events(&repo, "plan_reminder_sent").is_empty());
    let summary = plan.report().summary(true);
    assert!(summary.contains("skipped"), "{summary}");
    assert!(!summary.contains("outstanding"), "{summary}");
    assert_eq!(result.response, "I did not change anything.");
}

#[test]
fn a_plan_review_pause_carries_the_evidence_and_the_reminder_flag() {
    let repo = temp_repo("pause-evidence");
    let engine = engine_for(&repo);
    let mut first = scripted(vec![
        vec![plan_call(&["Read the source", "Write the report"])],
        vec![call("read_file", r#"{"path":"src/a.rs"}"#)],
        vec![patch_call("src/retry.rs")],
    ]);
    let paused = ask(&engine, &repo, "Add retry handling", &mut first);
    let proposal = paused.plan_proposal.expect("the plan is put up for review");

    let pending = pending_json(&repo, &proposal.id);
    assert_eq!(pending["plan_reminder_sent"], serde_json::json!(false));
    let evidence = pending["step_evidence"]
        .as_array()
        .expect("the pause carries evidence");
    assert!(
        evidence
            .iter()
            .any(|item| item["kind"].as_str() == Some("fileRead")),
        "the read is in the pending turn: {evidence:?}"
    );

    // Approved, the resumed turn closes the step; the read carried across the
    // pause is still on it.
    let mut after = scripted(vec![vec![call("complete_step", "{}")]]);
    let result = approve_plan(&engine, &proposal.id, &mut after);
    let plan = plan_of(&engine, &result).expect("the plan reads");
    assert_eq!(plan.steps[0].status, StepStatus::Completed);
    assert!(
        plan.steps[0]
            .evidence
            .iter()
            .any(|item| matches!(item, Evidence::FileRead { path, .. } if path == "src/a.rs")),
        "the read survived the pause: {:?}",
        plan.steps[0].evidence
    );
}

#[test]
fn a_pending_turn_written_before_step_evidence_still_loads() {
    // Both fields are `#[serde(default)]`: a turn paused by an older build has
    // neither, and must still resume (spec 66 §5.2, §5.3).
    let repo = temp_repo("old-pending");
    let engine = engine_for(&repo);
    let mut first = scripted(vec![
        vec![plan_call(&["Read the source", "Write the report"])],
        vec![call("read_file", r#"{"path":"src/a.rs"}"#)],
        vec![patch_call("src/retry.rs")],
    ]);
    let paused = ask(&engine, &repo, "Report", &mut first);
    let review = paused.plan_proposal.expect("the plan is put up for review");

    let mut command_round = scripted(vec![vec![call(
        "run_command",
        r#"{"command":"sed -i '' 's/1/2/' src/a.rs","reason":"Bump"}"#,
    )]]);
    let paused = approve_plan(&engine, &review.id, &mut command_round);
    let command = paused.command_proposal.expect("the command pauses");

    let path = repo
        .join(".damaian/chat/pending")
        .join(format!("{}.json", command.id));
    let mut value: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&path).expect("the pending file")).unwrap();
    let object = value.as_object_mut().expect("a pending turn object");
    object.remove("step_evidence");
    object.remove("plan_reminder_sent");
    fs::write(&path, serde_json::to_string(&value).unwrap()).unwrap();

    let restarted = engine_for(&repo);
    let mut finish = answer_only("The report is written.");
    let result = approve_command(&restarted, &command.id, &mut finish);
    assert_eq!(result.response, "The report is written.");
}

#[test]
fn an_approved_command_reaches_the_open_step_as_evidence() {
    let repo = temp_repo("command-evidence");
    let engine = engine_for(&repo);
    let mut first = scripted(vec![
        vec![plan_call(&["Read the source", "Write the report"])],
        vec![call("read_file", r#"{"path":"src/a.rs"}"#)],
        vec![patch_call("src/retry.rs")],
    ]);
    let paused = ask(&engine, &repo, "Add retry handling", &mut first);
    let review = paused.plan_proposal.expect("the plan is put up for review");

    // Approve the plan, then ask for a command that needs approval.
    let mut command_round = scripted(vec![vec![call(
        "run_command",
        r#"{"command":"sed -i '' 's/1/2/' src/a.rs","reason":"Bump the constant"}"#,
    )]]);
    let paused = approve_plan(&engine, &review.id, &mut command_round);
    let command = paused
        .command_proposal
        .expect("the command is put up for approval");
    let pending = pending_json(&repo, &command.id);
    assert!(
        pending["step_evidence"]
            .as_array()
            .is_some_and(|items| items
                .iter()
                .any(|item| item["kind"].as_str() == Some("fileRead"))),
        "the pause carries the read from before it"
    );

    let mut finish = scripted(vec![vec![call("complete_step", "{}")]]);
    let result = approve_command(&engine, &command.id, &mut finish);
    let plan = plan_of(&engine, &result).expect("the plan reads");

    let closed = &plan.steps[0];
    assert_eq!(closed.status, StepStatus::Completed);
    let command_exit = closed
        .evidence
        .iter()
        .find_map(|item| match item {
            Evidence::CommandExit {
                marker_id,
                exit_code,
                ..
            } => Some((marker_id.clone(), *exit_code)),
            _ => None,
        })
        .expect("the approved command's exit is evidence");
    assert_eq!(command_exit.1, Some(0));
    assert!(
        closed
            .evidence
            .iter()
            .any(|item| matches!(item, Evidence::FileRead { .. })),
        "the read from before the pause is still there"
    );

    let executed = audit_events(&repo, "stored_command_executed");
    assert_eq!(executed.len(), 1);
    assert_eq!(
        executed[0]["commandId"].as_str(),
        Some(command_exit.0.as_str()),
        "the breadcrumb is the execution id on the executed command record"
    );
}

#[test]
fn a_pause_does_not_attach_or_audit_at_the_open_step() {
    let repo = temp_repo("pause-no-audit");
    let engine = engine_for(&repo);
    let mut first = scripted(vec![
        vec![plan_call(&["Read the source", "Write the report"])],
        vec![call("read_file", r#"{"path":"src/a.rs"}"#)],
        vec![patch_call("src/retry.rs")],
    ]);
    let _paused = ask(&engine, &repo, "Add retry handling", &mut first);

    assert!(
        audit_events(&repo, "plan_left_open").is_empty(),
        "a pause is not a turn end"
    );
    // And nothing was attached: the evidence is in the pending turn instead,
    // which the pause test above covers.
    assert!(audit_events(&repo, "plan_reminder_sent").is_empty());
}

#[test]
fn a_turn_that_answers_with_a_step_open_keeps_it_open_with_its_evidence() {
    let repo = temp_repo("end-open");
    let engine = engine_for(&repo);
    let mut adapter = MockModelAdapter::new_sequence_with_tool_calls(
        vec![
            String::new(),
            String::new(),
            "The report is written.".to_string(),
            "Still not done.".to_string(),
        ],
        vec![
            vec![plan_call(&["Collect statuses", "Write the report"])],
            vec![call("read_file", r#"{"path":"src/a.rs"}"#)],
            Vec::new(),
            Vec::new(),
        ],
    );

    let result = ask(&engine, &repo, "Report", &mut adapter);
    let plan = plan_of(&engine, &result).expect("the plan reads");

    assert_eq!(plan.steps[0].status, StepStatus::InProgress);
    assert!(
        plan.steps[0]
            .evidence
            .iter()
            .any(|item| matches!(item, Evidence::FileRead { .. }))
    );
    let left_open = audit_events(&repo, "plan_left_open");
    assert_eq!(left_open.len(), 1);
    assert_eq!(left_open[0]["reminderSent"].as_str(), Some("true"));
}

#[test]
fn the_reminder_is_not_sent_twice_across_a_command_approval() {
    let repo = temp_repo("reminder-across-command");
    let engine = engine_for(&repo);
    // A mutating command cannot run before the plan is approved, so the plan
    // review comes first: plan, read, patch.
    let mut first = scripted(vec![
        vec![plan_call(&["Read the source", "Write the report"])],
        vec![call("read_file", r#"{"path":"src/a.rs"}"#)],
        vec![patch_call("src/retry.rs")],
    ]);
    let paused = ask(&engine, &repo, "Report", &mut first);
    let review = paused.plan_proposal.expect("the plan is put up for review");

    // Approving resumes the turn: the model answers (the reminder fires), then
    // asks for the command, which pauses for approval.
    let mut after = MockModelAdapter::new_sequence_with_tool_calls(
        vec!["The report is written.".to_string(), String::new()],
        vec![
            Vec::new(),
            vec![call(
                "run_command",
                r#"{"command":"sed -i '' 's/1/2/' src/a.rs","reason":"Bump the constant"}"#,
            )],
        ],
    );
    let paused = approve_plan(&engine, &review.id, &mut after);
    let command = paused.command_proposal.expect("the command pauses");
    let pending = pending_json(&repo, &command.id);
    assert_eq!(
        pending["plan_reminder_sent"],
        serde_json::json!(true),
        "the reminder already happened before the pause"
    );
    assert_eq!(
        audit_events(&repo, "plan_reminder_sent").len(),
        1,
        "one reminder before the pause"
    );

    // A second engine over the same data directory is what a restart looks
    // like: the flag and the evidence are in the pending file, not in memory.
    let restarted = engine_for(&repo);
    let mut finish = answer_only("The report is written.");
    let _ = approve_command(&restarted, &command.id, &mut finish);
    assert_eq!(
        audit_events(&repo, "plan_reminder_sent").len(),
        1,
        "a resume must not remind again"
    );
}

#[test]
fn the_reminder_is_not_sent_twice_across_a_plan_review() {
    let repo = temp_repo("reminder-across-review");
    let engine = engine_for(&repo);
    let mut first = MockModelAdapter::new_sequence_with_tool_calls(
        vec![
            String::new(),
            "The report is written.".to_string(),
            String::new(),
        ],
        vec![
            vec![plan_call(&["Read the source", "Write the report"])],
            Vec::new(),
            // The plan was never approved, so the first mutating call raises
            // the review gate even though the reminder has already gone.
            vec![patch_call("src/retry.rs")],
        ],
    );

    let paused = ask(&engine, &repo, "Report", &mut first);
    let review = paused.plan_proposal.expect("the plan is put up for review");
    assert_eq!(audit_events(&repo, "plan_reminder_sent").len(), 1);
    let pending = pending_json(&repo, &review.id);
    assert_eq!(pending["plan_reminder_sent"], serde_json::json!(true));

    let mut after = answer_only("The report is written.");
    let _ = approve_plan(&engine, &review.id, &mut after);
    assert_eq!(
        audit_events(&repo, "plan_reminder_sent").len(),
        1,
        "approving the plan must not remind again"
    );
}

#[test]
fn a_stopped_turn_keeps_its_step_open_with_its_evidence() {
    let repo = temp_repo("cancel-open");
    let engine = engine_for(&repo);
    let cancel = CancelToken::new();
    let mut adapter = MockModelAdapter::new_sequence_with_tool_calls(
        vec![String::new(), String::new(), "Working.".to_string()],
        vec![
            vec![plan_call(&["Read the source", "Report"])],
            vec![call("read_file", r#"{"path":"src/a.rs"}"#)],
            Vec::new(),
        ],
    );
    let mut on_token = |token: &str| {
        if token.is_empty() {
            return;
        }
        cancel.cancel();
    };
    let mut on_progress = |_event: TurnProgress| {};
    let mut sink = TurnSink {
        on_token: &mut on_token,
        on_progress: &mut on_progress,
        cancel: &cancel,
        steering: None,
    };
    let result = engine
        .chat_orchestrator
        .ask_with_session(&repo, "Report", &[], None, &mut adapter, &mut sink)
        .expect("the turn should stop cleanly");

    let plan = plan_of(&engine, &result).expect("the plan reads");
    assert_eq!(plan.steps[0].status, StepStatus::InProgress);
    assert!(
        plan.steps[0]
            .evidence
            .iter()
            .any(|item| matches!(item, Evidence::FileRead { .. })),
        "a stopped turn keeps what it ran: {:?}",
        plan.steps[0].evidence
    );
    let left_open = audit_events(&repo, "plan_left_open");
    assert_eq!(left_open.len(), 1);
    assert_eq!(left_open[0]["stopReason"].as_str(), Some("cancelled"));
}

/// Proposes a plan, reads a file, then fails the transport on the third call —
/// the shape a provider failure takes after a turn has already done work.
struct PlansReadsThenFails {
    calls: usize,
}

impl ModelAdapter for PlansReadsThenFails {
    fn stream_response(
        &mut self,
        request: &workspace_engine::ModelRequest,
        _cancel: &CancelToken,
        _on_token: &mut dyn FnMut(&str),
        _on_wait: &mut dyn FnMut(u64),
    ) -> workspace_engine::Result<ModelRun> {
        self.calls += 1;
        let tool_calls = match self.calls {
            1 => vec![plan_call(&["Read the source", "Write the report"])],
            2 => vec![call("read_file", r#"{"path":"src/a.rs"}"#)],
            _ => {
                return Err(workspace_engine::ClientError::Io(
                    "transport died".to_string(),
                ));
            }
        };
        Ok(ModelRun {
            run_id: format!("run_{}", self.calls),
            provider: "mock".to_string(),
            model: request.model.clone(),
            started_at_ms: 0,
            completed_at_ms: 0,
            content: String::new(),
            incomplete: false,
            retry_count: 0,
            refusal_retries: 0,
            tool_calls,
            truncated: false,
            reasoning_content: None,
            usage: TokenUsage::estimated(10, 5),
            reported_cost: None,
            usage_reporting_unsupported: false,
        })
    }
}

#[test]
fn a_provider_failure_keeps_its_step_open() {
    let repo = temp_repo("provider-failure");
    let engine = engine_for(&repo);
    let cancel = CancelToken::new();
    let mut on_token = |_token: &str| {};
    let mut on_progress = |_event: TurnProgress| {};
    let mut sink = TurnSink {
        on_token: &mut on_token,
        on_progress: &mut on_progress,
        cancel: &cancel,
        steering: None,
    };
    let mut adapter = PlansReadsThenFails { calls: 0 };

    engine
        .chat_orchestrator
        .ask_with_session(&repo, "Report", &[], None, &mut adapter, &mut sink)
        .expect_err("the transport failed");

    let session = engine
        .session_store
        .list_sessions(None)
        .expect("sessions")
        .into_iter()
        .next()
        .expect("a session");
    let task = engine
        .session_store
        .read_tasks(&session.id)
        .expect("tasks")
        .into_iter()
        .next()
        .expect("a task");
    assert_eq!(task.status, TaskStatus::Failed);
    let plan = engine
        .session_store
        .read_task_plan(&session.id, &task.id)
        .expect("the plan reads")
        .expect("the turn planned");
    assert_eq!(plan.steps[0].status, StepStatus::InProgress);
    assert!(
        plan.steps[0]
            .evidence
            .iter()
            .any(|item| matches!(item, Evidence::FileRead { .. })),
        "the failed turn kept what it ran: {:?}",
        plan.steps[0].evidence
    );
    let left_open = audit_events(&repo, "plan_left_open");
    assert_eq!(left_open.len(), 1);
    assert_eq!(left_open[0]["stopReason"].as_str(), Some("failed"));
}
