//! The plan as a turn actually produces it, per
//! `docs/specs/21_task_plan_progress_and_budget/proposal.md` §5.1 and §5.3.
//!
//! `plan.rs` covers the types and the store on their own. These tests go
//! through the chat orchestrator, because the guarantees that matter are about
//! runs: that a trivial turn gets no plan, that exactly one step is in progress
//! at a time, and — the one this spec exists for — that a step's status comes
//! from what the engine observed and not from what the model said about it.

use std::collections::BTreeMap;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use workspace_engine::plan::{Evidence, StepStatus, TaskPlan};
use workspace_engine::{
    CancelToken, ChatTurnResult, CheckBaseline, CheckId, CommandPurpose, CompletionFacts,
    CompletionGateReport, CompletionReasonCode, Config, FailureProvenance, MockModelAdapter,
    ModelAdapter, ModelRun, PLANNED_CHECK_VERSION, PatchApplyOutcome, PlanRevisionCheck,
    PlanRevisionStep, PlannedCheck, TaskStatus, TokenUsage, ToolCall, TurnProgress, TurnSink,
    VerificationEvent, WorkerRecord, WorkspaceEngine, fold_verification,
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

/// [`ask`], continuing an existing session so the turn picks up its carried
/// plan.
fn ask_in_session(
    engine: &WorkspaceEngine,
    repo: &Path,
    session_id: &str,
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
        .ask_with_session(repo, prompt, &[], Some(session_id), adapter, &mut sink)
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
    try_decide(engine, proposal_id, approved, revised, adapter)
        .expect("the resumed turn should run")
}

fn try_decide(
    engine: &WorkspaceEngine,
    proposal_id: &str,
    approved: bool,
    revised: Option<Vec<PlanRevisionStep>>,
    adapter: &mut dyn ModelAdapter,
) -> workspace_engine::Result<ChatTurnResult> {
    let cancel = CancelToken::new();
    let mut on_token = |_token: &str| {};
    let mut on_progress = |_event: TurnProgress| {};
    let mut sink = TurnSink {
        on_token: &mut on_token,
        on_progress: &mut on_progress,
        cancel: &cancel,
        steering: None,
    };
    engine.chat_orchestrator.resume_after_plan_decision(
        proposal_id,
        approved,
        revised,
        "tester",
        adapter,
        &mut sink,
    )
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

/// The completion gate over the plan the turn left behind and no verification
/// runs. The deferred check's evidence is the step's own `CommandExit`, so an
/// empty `CheckRun` state is the whole of what the gate reads for it.
fn gate_report(engine: &WorkspaceEngine, result: &ChatTurnResult) -> CompletionGateReport {
    let plan = plan_of(engine, result).expect("the plan should read");
    let verification = fold_verification(std::iter::empty::<&VerificationEvent>());
    CompletionGateReport::evaluate(CompletionFacts {
        task_id: &result.task.id,
        plan: Some(&plan),
        verification: &verification,
        pending: None,
        current_hashes: &BTreeMap::new(),
    })
}

/// Runs the verification a patch application made due, the way the client
/// does after its apply settles.
fn run_verification_due(
    engine: &WorkspaceEngine,
    repo: &Path,
    session_id: &str,
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
        .verify_due(repo, session_id, adapter, &mut sink)
        .expect("the due verification should run")
}

fn resume_checks(
    engine: &WorkspaceEngine,
    batch_id: &str,
    decisions: &[(String, bool)],
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
        .resume_after_check_decision(batch_id, decisions, "tester", adapter, &mut sink)
        .expect("the check batch should resume")
}

/// Runs a due verification, approving every check the ordinary check card
/// offers. Some fixtures exercise commands the policy still asks about; the
/// card is not the behaviour under test.
fn run_verification_and_approve_checks(
    engine: &WorkspaceEngine,
    repo: &Path,
    session_id: &str,
    adapter: &mut dyn ModelAdapter,
) -> ChatTurnResult {
    let mut result = run_verification_due(engine, repo, session_id, adapter);
    while let Some(batch) = result.check_batch.clone() {
        let decisions = batch
            .checks
            .iter()
            .map(|check| (check.id.clone(), true))
            .collect::<Vec<_>>();
        result = resume_checks(engine, &batch.id, &decisions, adapter);
    }
    result
}

/// Continues the newest incomplete task through the engine's own continuation
/// route, carrying its plan and completion report.
fn continue_task(
    engine: &WorkspaceEngine,
    repo: &Path,
    session_id: &str,
    task_id: &str,
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
        .continue_incomplete_task(repo, session_id, task_id, None, adapter, &mut sink)
        .expect("the continuation should run")
}

const CHECK_ON_SECOND_STEP: &str = r#"{"steps":[
    {"title":"Make the change"},
    {"title":"Run the check","check":{"command":"ls","workingDirectory":"","purpose":"validates the final tree","required":true}}
]}"#;

/// Runs the deferred check on its plan step, lets the turn end so the engine
/// attaches the check's evidence, then proposes a patch in a later turn while
/// that same step is still open. The patch turn ends on the patch proposal, so
/// the step stays `InProgress` until another turn closes it.
fn run_check_then_propose_a_patch(
    engine: &WorkspaceEngine,
    repo: &Path,
    plan_json: &str,
    patch_path: &str,
) -> (ChatTurnResult, String) {
    let mut adapter = scripted(vec![
        vec![call("propose_plan", plan_json)],
        // The mutation is what puts the plan up for review; the check alone
        // would not.
        vec![patch_call(patch_path)],
    ]);
    let paused = ask(engine, repo, "Add retry handling", &mut adapter);
    let proposal = paused.plan_proposal.expect("the plan should be put up");

    // Run the check and answer: a plain answer is a true turn end, so the
    // command's exit is attached to the still-open step.
    let mut after = scripted(vec![
        vec![call("complete_step", "{}")],
        vec![call(
            "run_command",
            r#"{"command":"ls","reason":"Run the required check"}"#,
        )],
    ]);
    let checked = decide(engine, &proposal.id, true, None, &mut after);

    // A later turn proposes the patch. The check step is still open, so an
    // apply lands its `PatchApplied` after the check's exit on the same step.
    let mut patch_adapter = scripted(vec![vec![patch_call(patch_path)]]);
    let proposed = ask_in_session(
        engine,
        repo,
        &checked.session.id,
        "Continue with the change",
        &mut patch_adapter,
    );
    let patch = proposed
        .patch_proposal
        .as_ref()
        .expect("the patch is proposed while the check step is open")
        .patch_id
        .clone();
    (proposed, patch)
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

/// A repository whose `./check` fails the first time and passes after: the
/// shape of "the check failed, the cause was fixed, the check was run again".
fn engine_with_a_check_that_passes_on_its_second_run(repo: &Path) -> WorkspaceEngine {
    fs::write(
        repo.join("check"),
        "#!/bin/sh\nif [ -f .fixed ]; then exit 0; fi\ntouch .fixed\nexec ls no-such-directory\n",
    )
    .unwrap();
    fs::set_permissions(repo.join("check"), fs::Permissions::from_mode(0o755)).unwrap();
    WorkspaceEngine::new(Config {
        command_allowlist: vec!["./check".to_string(), "ls src".to_string()],
        data_dir: repo.join(".damaian"),
        enable_index_watcher: false,
        ..Config::default()
    })
}

fn tool_results(engine: &WorkspaceEngine, result: &ChatTurnResult) -> Vec<String> {
    engine
        .session_store
        .read_messages(&result.session.id)
        .expect("messages should read")
        .into_iter()
        .map(|message| message.content)
        .collect()
}

/// Found in a session on 2026-10-29: after a step blocked, nothing the model
/// did could clear it, and four `complete_step` calls got the same refusal.
/// Running the failed command again, successfully, now clears the block.
#[test]
fn a_blocked_step_recovers_when_the_same_command_later_passes() {
    let repo = temp_repo("blocked-recovers");
    let engine = engine_with_a_check_that_passes_on_its_second_run(&repo);
    let mut adapter = scripted(vec![
        vec![call(
            "propose_plan",
            r#"{"steps":[{"title":"A"},{"title":"B"}]}"#,
        )],
        vec![call(
            "run_command",
            r#"{"command":"./check","reason":"Check it"}"#,
        )],
        vec![call("complete_step", "{}")],
        vec![call(
            "run_command",
            r#"{"command":"./check","reason":"Check it again after the fix"}"#,
        )],
        vec![call("complete_step", "{}")],
    ]);

    let result = ask(&engine, &repo, "Do it", &mut adapter);
    let plan = plan_of(&engine, &result).expect("the turn proposed a plan");

    assert_eq!(plan.steps[0].status, StepStatus::Completed);
    assert_eq!(plan.steps[1].status, StepStatus::InProgress);
    assert!(!plan.violates_single_in_progress());
    // Both runs stay on the step: the failure is history, not erased.
    let exits: Vec<Option<i32>> = plan.steps[0]
        .evidence
        .iter()
        .filter_map(|item| match item {
            Evidence::CommandExit { exit_code, .. } => Some(*exit_code),
            _ => None,
        })
        .collect();
    assert_eq!(exits.len(), 2, "{:?}", plan.steps[0].evidence);
    assert_ne!(exits[0], Some(0));
    assert_eq!(exits[1], Some(0));
    let messages = tool_results(&engine, &result);
    assert!(
        messages
            .iter()
            .any(|message| message.contains("is no longer blocked")),
        "{messages:?}"
    );
}

#[test]
fn a_different_command_passing_leaves_the_step_blocked() {
    let repo = temp_repo("blocked-other-command");
    let engine = engine_with_a_check_that_passes_on_its_second_run(&repo);
    let mut adapter = scripted(vec![
        vec![call(
            "propose_plan",
            r#"{"steps":[{"title":"A"},{"title":"B"}]}"#,
        )],
        vec![call(
            "run_command",
            r#"{"command":"./check","reason":"Check it"}"#,
        )],
        vec![call("complete_step", "{}")],
        vec![call(
            "run_command",
            r#"{"command":"ls src","reason":"Something else that works"}"#,
        )],
        vec![call("complete_step", "{}")],
    ]);

    let result = ask(&engine, &repo, "Do it", &mut adapter);
    let plan = plan_of(&engine, &result).expect("the turn proposed a plan");

    assert_eq!(plan.steps[0].status, StepStatus::Blocked);
    assert_eq!(plan.steps[1].status, StepStatus::Pending);
    // What was observed since is kept on the step for the next re-check.
    assert_eq!(
        plan.steps[0]
            .evidence
            .iter()
            .filter(|item| matches!(item, Evidence::CommandExit { .. }))
            .count(),
        2,
        "{:?}",
        plan.steps[0].evidence
    );
    let messages = tool_results(&engine, &result);
    assert!(
        messages
            .iter()
            .any(|message| message.contains("is still blocked")
                && message.contains("same command")),
        "{messages:?}"
    );
}

#[test]
fn a_check_that_failed_then_passed_within_the_step_completes_it() {
    let repo = temp_repo("fail-then-pass");
    let engine = engine_with_a_check_that_passes_on_its_second_run(&repo);
    let mut adapter = scripted(vec![
        vec![call(
            "propose_plan",
            r#"{"steps":[{"title":"A"},{"title":"B"}]}"#,
        )],
        vec![call(
            "run_command",
            r#"{"command":"./check","reason":"Check it"}"#,
        )],
        vec![call(
            "run_command",
            r#"{"command":"./check","reason":"Check it again"}"#,
        )],
        vec![call("complete_step", "{}")],
    ]);

    let result = ask(&engine, &repo, "Do it", &mut adapter);
    let plan = plan_of(&engine, &result).expect("the turn proposed a plan");

    assert_eq!(plan.steps[0].status, StepStatus::Completed);
    assert_eq!(plan.steps[1].status, StepStatus::InProgress);
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
            check: PlanRevisionCheck::Preserve,
        },
        PlanRevisionStep {
            id: ids[1].clone(),
            title: "Add a backoff capped at 30s".to_string(),
            check: PlanRevisionCheck::Preserve,
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
fn a_revision_carries_the_users_check_edits_into_the_settled_plan() {
    // Spec 105 §2, §7: the review card lets the user change, remove or add a
    // proposed check, and the settled plan records exactly what they approved.
    let repo = temp_repo("revise-check");
    let engine = engine_for(&repo);
    let mut adapter = scripted(vec![
        vec![call(
            "propose_plan",
            r#"{"steps":[
                {"title":"Implement the change","check":{"command":"cargo test","workingDirectory":"","purpose":"validates","required":true}},
                {"title":"Run the suite","check":{"command":"cargo nextest run","workingDirectory":"","purpose":"final","required":true}},
                {"title":"Write the report"}
            ]}"#,
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
    assert_eq!(
        proposal.plan.steps[0]
            .check
            .as_ref()
            .map(|c| c.command.as_str()),
        Some("cargo test")
    );

    // Change the first check (new command, directory and purpose, and toggle
    // `required` off), remove the second, and add one to the third.
    let revision = vec![
        PlanRevisionStep {
            id: ids[0].clone(),
            title: "Implement the change".to_string(),
            check: PlanRevisionCheck::Set(PlannedCheck {
                version: PLANNED_CHECK_VERSION,
                command: "cargo test -p engine".to_string(),
                working_directory: "crates/engine".to_string(),
                purpose: "changed".to_string(),
                required: false,
            }),
        },
        PlanRevisionStep {
            id: ids[1].clone(),
            title: "Run the suite".to_string(),
            check: PlanRevisionCheck::Remove,
        },
        PlanRevisionStep {
            id: ids[2].clone(),
            title: "Write the report".to_string(),
            check: PlanRevisionCheck::Set(PlannedCheck {
                version: PLANNED_CHECK_VERSION,
                command: "cargo doc".to_string(),
                working_directory: String::new(),
                purpose: "added".to_string(),
                required: false,
            }),
        },
    ];
    let mut after = scripted(vec![vec![patch_call("src/retry.rs")]]);
    let result = decide(&engine, &proposal.id, true, Some(revision), &mut after);

    let plan = plan_of(&engine, &result).expect("the settled plan should read back");
    assert_eq!(plan.steps.len(), 3);
    assert_eq!(
        plan.steps[0].check,
        Some(PlannedCheck {
            version: PLANNED_CHECK_VERSION,
            command: "cargo test -p engine".to_string(),
            working_directory: "crates/engine".to_string(),
            purpose: "changed".to_string(),
            required: false,
        }),
        "the changed check records the new purpose and the toggled required flag"
    );
    assert_eq!(plan.steps[1].check, None, "the removed check is gone");
    assert_eq!(
        plan.steps[2].check,
        Some(PlannedCheck {
            version: PLANNED_CHECK_VERSION,
            command: "cargo doc".to_string(),
            working_directory: String::new(),
            purpose: "added".to_string(),
            required: false,
        }),
        "the added check is what the user approved"
    );
}

#[test]
fn approving_a_plan_with_a_required_check_runs_no_command_and_the_check_still_needs_approval() {
    // Spec 105 requirement 3: approving a plan that carries a required check is
    // not command approval. The approval settles the plan (and its check) but
    // runs nothing; when the worker reaches the check, the command still follows
    // command policy and pauses for its own one-shot approval.
    let repo = temp_repo("approve-check");
    let engine = engine_for(&repo);
    let mut adapter = scripted(vec![
        vec![call(
            "propose_plan",
            r#"{"steps":[
                {"title":"Implement the change"},
                {"title":"Validate","check":{"command":"cargo test","workingDirectory":"","purpose":"validates the final tree","required":true}}
            ]}"#,
        )],
        vec![patch_call("src/retry.rs")],
    ]);
    let paused = ask(&engine, &repo, "Add retry handling", &mut adapter);
    let proposal = paused.plan_proposal.expect("the plan should be put up");

    // Approving the plan runs nothing: the resumed turn's first call is the
    // check command, which still pauses on command policy's own card.
    let mut after = scripted(vec![vec![call(
        "run_command",
        r#"{"command":"cargo test","reason":"Run the required check"}"#,
    )]]);
    let result = decide(&engine, &proposal.id, true, None, &mut after);

    assert!(
        result.command_proposal.is_some(),
        "the required check still waits for command approval; the plan approval did not run it"
    );
    assert_eq!(result.task.status, TaskStatus::WaitingForApproval);
    let settled = plan_of(&engine, &result).expect("the settled plan should read back");
    assert_eq!(
        settled.steps[1]
            .check
            .as_ref()
            .map(|check| check.command.as_str()),
        Some("cargo test"),
        "the approved plan still carries the required check"
    );
}

/// Spec 105's check identity is directory + command. A required check in
/// `crates/foo` is not satisfied by the same command run at the repository
/// root, even though the exact command text matches. This drives the command
/// through the real `run_command` path so the directory the gate sees is the
/// one the command actually ran in.
#[test]
fn a_required_check_in_a_subdirectory_is_missing_when_the_command_ran_at_the_root() {
    let repo = temp_repo("check-directory-root");
    fs::create_dir_all(repo.join("crates/foo")).expect("the check directory should exist");
    let engine = engine_for(&repo);
    let mut adapter = scripted(vec![
        vec![call(
            "propose_plan",
            r#"{"steps":[
                {"title":"Make the change"},
                {"title":"Run the check","check":{"command":"ls","workingDirectory":"crates/foo","purpose":"validates the final tree","required":true}}
            ]}"#,
        )],
        // A mutation is what raises the plan review; `ls` alone never would.
        vec![patch_call("src/b.rs")],
    ]);
    let paused = ask(&engine, &repo, "Add retry handling", &mut adapter);
    let proposal = paused.plan_proposal.expect("the plan should be put up");
    let mut after = scripted(vec![
        vec![call("complete_step", "{}")],
        vec![call(
            "run_command",
            r#"{"command":"ls","reason":"Run the required check"}"#,
        )],
        vec![call("complete_step", "{}")],
    ]);
    let result = decide(&engine, &proposal.id, true, None, &mut after);

    let plan = plan_of(&engine, &result).expect("the settled plan should read back");
    assert_eq!(plan.steps[1].status, StepStatus::Completed);
    assert_eq!(
        plan.steps[1]
            .evidence
            .iter()
            .find_map(|evidence| match evidence {
                Evidence::CommandExit {
                    working_directory, ..
                } => Some(working_directory.as_str()),
                _ => None,
            }),
        Some(""),
        "the run at the root records the repository root, not `crates/foo`"
    );
    assert_eq!(
        gate_report(&engine, &result).reason_codes(),
        vec![CompletionReasonCode::RequiredCheckMissing],
        "the same command at the repository root is a different check"
    );
}

/// The other half of the same identity: the command run in the required
/// directory satisfies the check. This guards against fixing the mismatch by
/// never matching at all.
#[test]
fn a_required_check_in_a_subdirectory_passes_when_the_command_ran_there() {
    let repo = temp_repo("check-directory-subdir");
    fs::create_dir_all(repo.join("crates/foo")).expect("the check directory should exist");
    let engine = engine_for(&repo);
    let mut adapter = scripted(vec![
        vec![call(
            "propose_plan",
            r#"{"steps":[
                {"title":"Make the change"},
                {"title":"Run the check","check":{"command":"ls","workingDirectory":"crates/foo","purpose":"validates the final tree","required":true}}
            ]}"#,
        )],
        // A mutation is what raises the plan review; `ls` alone never would.
        vec![patch_call("src/b.rs")],
    ]);
    let paused = ask(&engine, &repo, "Add retry handling", &mut adapter);
    let proposal = paused.plan_proposal.expect("the plan should be put up");
    let mut after = scripted(vec![
        vec![call("complete_step", "{}")],
        vec![call(
            "run_command",
            r#"{"command":"ls","reason":"Run the required check","working_directory":"crates/foo"}"#,
        )],
        vec![call("complete_step", "{}")],
    ]);
    let result = decide(&engine, &proposal.id, true, None, &mut after);

    let plan = plan_of(&engine, &result).expect("the settled plan should read back");
    assert_eq!(
        plan.steps[1]
            .evidence
            .iter()
            .find_map(|evidence| match evidence {
                Evidence::CommandExit {
                    working_directory, ..
                } => Some(working_directory.as_str()),
                _ => None,
            }),
        Some("crates/foo"),
        "the run records the directory `run_command` actually used"
    );
    let report = gate_report(&engine, &result);
    assert!(report.eligible, "{report:?}");
    assert!(report.reasons.is_empty(), "{report:?}");
}

/// A model naturally writes `.` for the repository root. The planned check's
/// directory is stored in the `CheckId` form (`""`), so the same command run at
/// the root satisfies it instead of leaving a check that can never match.
#[test]
fn a_plan_check_with_a_dot_directory_matches_the_command_run_at_the_root() {
    let repo = temp_repo("check-directory-dot");
    let engine = engine_for(&repo);
    let mut adapter = scripted(vec![
        vec![call(
            "propose_plan",
            r#"{"steps":[
                {"title":"Make the change"},
                {"title":"Run the check","check":{"command":"ls","workingDirectory":".","purpose":"validates the final tree","required":true}}
            ]}"#,
        )],
        vec![patch_call("src/b.rs")],
    ]);
    let paused = ask(&engine, &repo, "Add retry handling", &mut adapter);
    let proposal = paused.plan_proposal.expect("the plan should be put up");
    assert_eq!(
        proposal.plan.steps[1]
            .check
            .as_ref()
            .map(|check| check.working_directory.as_str()),
        Some(""),
        "`.` is the repository root in `CheckId` form"
    );

    let mut after = scripted(vec![
        vec![call("complete_step", "{}")],
        vec![call(
            "run_command",
            r#"{"command":"ls","reason":"Run the required check"}"#,
        )],
        vec![call("complete_step", "{}")],
    ]);
    let result = decide(&engine, &proposal.id, true, None, &mut after);

    let report = gate_report(&engine, &result);
    assert!(report.eligible, "{report:?}");
}

/// `./crates/foo/` is the same directory as `crates/foo` to `CheckId`, so it
/// must defer and match the command run there.
#[test]
fn a_plan_check_with_a_relative_subdirectory_matches_the_command_run_there() {
    let repo = temp_repo("check-directory-relative");
    fs::create_dir_all(repo.join("crates/foo")).expect("the check directory should exist");
    let engine = engine_for(&repo);
    let mut adapter = scripted(vec![
        vec![call(
            "propose_plan",
            r#"{"steps":[
                {"title":"Make the change"},
                {"title":"Run the check","check":{"command":"ls","workingDirectory":"./crates/foo/","purpose":"validates the final tree","required":true}}
            ]}"#,
        )],
        vec![patch_call("src/b.rs")],
    ]);
    let paused = ask(&engine, &repo, "Add retry handling", &mut adapter);
    let proposal = paused.plan_proposal.expect("the plan should be put up");
    assert_eq!(
        proposal.plan.steps[1]
            .check
            .as_ref()
            .map(|check| check.working_directory.as_str()),
        Some("crates/foo"),
        "a leading `./` and trailing `/` name the same directory"
    );

    let mut after = scripted(vec![
        vec![call("complete_step", "{}")],
        vec![call(
            "run_command",
            r#"{"command":"ls","reason":"Run the required check","working_directory":"crates/foo"}"#,
        )],
        vec![call("complete_step", "{}")],
    ]);
    let result = decide(&engine, &proposal.id, true, None, &mut after);

    let report = gate_report(&engine, &result);
    assert!(report.eligible, "{report:?}");
}

/// A directory that cannot be repository-relative is refused, not guessed at.
/// The check is dropped the same way a check with no command is, and the step's
/// prose remains.
#[test]
fn a_plan_check_with_a_directory_outside_the_repository_is_dropped() {
    for (index, directory) in ["../outside", "/etc"].iter().enumerate() {
        let repo = temp_repo(&format!("check-drop-{index}"));
        let engine = engine_for(&repo);
        let proposed = serde_json::json!({
            "steps": [
                {"title":"Make the change"},
                {
                    "title":"Run the check",
                    "check": {
                        "command":"ls",
                        "workingDirectory": directory,
                        "purpose":"validates the final tree",
                        "required":true
                    }
                }
            ]
        });
        let mut adapter = scripted(vec![
            vec![call("propose_plan", &proposed.to_string())],
            vec![patch_call("src/b.rs")],
        ]);
        let paused = ask(&engine, &repo, "Add retry handling", &mut adapter);
        let proposal = paused.plan_proposal.expect("the plan should be put up");

        assert!(
            proposal.plan.steps[1].check.is_none(),
            "{directory}: a check outside the repository is dropped, not reinterpreted"
        );
        assert_eq!(proposal.plan.steps[1].title, "Run the check");
    }
}

/// The engine's own revision path refuses a directory it cannot place, and
/// leaves the review answerable. A valid revision is stored in `CheckId` form.
#[test]
fn a_plan_revision_with_a_directory_outside_the_repository_is_rejected() {
    let repo = temp_repo("revision-check-directory");
    let engine = engine_for(&repo);
    let mut adapter = scripted(vec![
        vec![call(
            "propose_plan",
            r#"{"steps":[{"title":"Make the change"},{"title":"Run the check"}]}"#,
        )],
        vec![patch_call("src/b.rs")],
    ]);
    let paused = ask(&engine, &repo, "Add retry handling", &mut adapter);
    let proposal = paused.plan_proposal.expect("the plan should be put up");
    let first_id = proposal.plan.steps[0].id.clone();
    let step_id = proposal.plan.steps[1].id.clone();

    let revision = |directory: &str| {
        vec![
            PlanRevisionStep {
                id: first_id.clone(),
                title: "Make the change".to_string(),
                check: PlanRevisionCheck::Preserve,
            },
            PlanRevisionStep {
                id: step_id.clone(),
                title: "Run the check".to_string(),
                check: PlanRevisionCheck::Set(PlannedCheck {
                    version: PLANNED_CHECK_VERSION,
                    command: "ls".to_string(),
                    working_directory: directory.to_string(),
                    purpose: "validates the final tree".to_string(),
                    required: true,
                }),
            },
        ]
    };

    let error = try_decide(
        &engine,
        &proposal.id,
        true,
        Some(revision("../outside")),
        &mut MockModelAdapter::new("Done."),
    )
    .expect_err("a directory outside the repository is refused");
    assert!(
        error.to_string().contains("outside this repository"),
        "{error}"
    );

    let result = decide(
        &engine,
        &proposal.id,
        true,
        Some(revision("./crates/foo/")),
        &mut scripted(vec![]),
    );
    let plan = plan_of(&engine, &result).expect("the settled plan should read back");
    assert_eq!(
        plan.steps[1]
            .check
            .as_ref()
            .map(|check| check.working_directory.as_str()),
        Some("crates/foo"),
        "the valid revision is stored normalized after the refusal was answered again"
    );
}

/// R4 regression from the 2026-11-04 review of specs 101–105: a deferred
/// required check's evidence was current merely because its step was terminal
/// and no later step had begun. A patch applied after the check, still in the
/// same step, left the evidence stale in fact but current to the gate, so the
/// task could close on a tree the check never saw.
#[test]
fn a_patch_applied_after_the_deferred_check_makes_its_evidence_stale() {
    let repo = temp_repo("check-stale-same-step");
    let engine = engine_for(&repo);
    let (checked, patch) =
        run_check_then_propose_a_patch(&engine, &repo, CHECK_ON_SECOND_STEP, "src/added.rs");
    let open_plan = plan_of(&engine, &checked).expect("the open plan should read back");
    assert!(
        open_plan.steps[1]
            .evidence
            .iter()
            .any(|evidence| matches!(evidence, Evidence::CommandExit { .. })),
        "the check ran before the patch: steps={:?}",
        open_plan
            .steps
            .iter()
            .map(|step| (&step.id, step.status, &step.evidence))
            .collect::<Vec<_>>()
    );

    engine
        .edit_orchestrator
        .apply_stored_patch(&repo, &patch, None, None, "tester", false)
        .expect("the approved patch applies");

    // Clear the patch's verification due through its ordinary route. The
    // verification turn carries the plan; the stale check stops it continuing.
    let verified = run_verification_due(
        &engine,
        &repo,
        &checked.session.id,
        &mut MockModelAdapter::new("Done."),
    );
    let mut adapter = scripted(vec![vec![call("complete_step", "{}")]]);
    let result = continue_task(
        &engine,
        &repo,
        &checked.session.id,
        &verified.task.id,
        &mut adapter,
    );

    assert_eq!(
        result.task.status,
        TaskStatus::Incomplete,
        "the check ran before the patch, so closing the step cannot complete the task"
    );
    let report = engine
        .session_store
        .read_completion_report(&checked.session.id, &result.task.id)
        .unwrap()
        .expect("the continued task settles through the gate");
    assert_eq!(
        report.reason_codes(),
        vec![CompletionReasonCode::RequiredCheckStale],
        "{report:?}"
    );
}

/// The other side of the same rule: rerunning the exact check after the patch
/// makes its evidence current again, so the same step can close the task.
#[test]
fn rerunning_the_deferred_check_after_the_patch_makes_it_current() {
    let repo = temp_repo("check-rerun-same-step");
    let engine = engine_for(&repo);
    let (checked, patch) =
        run_check_then_propose_a_patch(&engine, &repo, CHECK_ON_SECOND_STEP, "src/added.rs");

    engine
        .edit_orchestrator
        .apply_stored_patch(&repo, &patch, None, None, "tester", false)
        .expect("the approved patch applies");

    let verified = run_verification_due(
        &engine,
        &repo,
        &checked.session.id,
        &mut MockModelAdapter::new("Done."),
    );
    let mut adapter = scripted(vec![
        vec![call(
            "run_command",
            r#"{"command":"ls","reason":"Run the required check again"}"#,
        )],
        vec![call("complete_step", "{}")],
    ]);
    let result = continue_task(
        &engine,
        &repo,
        &checked.session.id,
        &verified.task.id,
        &mut adapter,
    );

    assert_eq!(
        result.task.status,
        TaskStatus::Complete,
        "the rerun validates the patched tree: {:?}",
        result.task
    );
    let report = engine
        .session_store
        .read_completion_report(&checked.session.id, &result.task.id)
        .unwrap()
        .expect("the completed task carries its report");
    assert!(report.eligible, "{report:?}");
}

/// A patch applied in a later step also invalidates the earlier check's
/// evidence. That case already read as stale before R4 because the later step
/// had begun; the test keeps the rule from regressing when staleness starts
/// tracking mutations rather than step position alone.
#[test]
fn a_patch_applied_in_a_later_step_makes_the_check_step_stale() {
    let repo = temp_repo("check-stale-later-step");
    let engine = engine_for(&repo);
    let mut adapter = scripted(vec![
        vec![call(
            "propose_plan",
            r#"{"steps":[
                {"title":"Run the check","check":{"command":"ls","workingDirectory":"","purpose":"validates the final tree","required":true}},
                {"title":"Make the change"}
            ]}"#,
        )],
        vec![patch_call("src/added.rs")],
    ]);
    let paused = ask(&engine, &repo, "Add retry handling", &mut adapter);
    let proposal = paused.plan_proposal.expect("the plan should be put up");
    let mut after = scripted(vec![
        vec![call(
            "run_command",
            r#"{"command":"ls","reason":"Run the required check"}"#,
        )],
        vec![call("complete_step", "{}")],
        vec![patch_call("src/added.rs")],
    ]);
    let result = decide(&engine, &proposal.id, true, None, &mut after);
    let patch = result
        .patch_proposal
        .as_ref()
        .expect("the patch is proposed in the later step")
        .patch_id
        .clone();

    engine
        .edit_orchestrator
        .apply_stored_patch(&repo, &patch, None, None, "tester", false)
        .expect("the approved patch applies");

    let verified = run_verification_due(
        &engine,
        &repo,
        &result.session.id,
        &mut MockModelAdapter::new("Done."),
    );
    let mut adapter = scripted(vec![vec![call("complete_step", "{}")]]);
    let result = continue_task(
        &engine,
        &repo,
        &result.session.id,
        &verified.task.id,
        &mut adapter,
    );

    assert_eq!(result.task.status, TaskStatus::Incomplete);
    let report = engine
        .session_store
        .read_completion_report(&result.session.id, &result.task.id)
        .unwrap()
        .expect("the continued task settles through the gate");
    assert_eq!(
        report.reason_codes(),
        vec![CompletionReasonCode::RequiredCheckStale],
        "{report:?}"
    );
}

/// A patch that is only proposed, or proposed and declined, leaves no
/// `PatchApplied` in the plan, so the check that ran before it is still
/// current. Staleness is a fact about mutations, not about a pending card.
#[test]
fn a_patch_that_is_declined_or_never_applied_does_not_make_the_check_stale() {
    for declined in [false, true] {
        let repo = temp_repo(&format!("check-not-applied-{declined}"));
        let engine = engine_for(&repo);
        let (checked, patch) =
            run_check_then_propose_a_patch(&engine, &repo, CHECK_ON_SECOND_STEP, "src/added.rs");
        if declined {
            engine
                .edit_orchestrator
                .reject_stored_patch(&patch, "tester")
                .expect("the patch is declined");
        }

        let mut adapter = scripted(vec![vec![call("complete_step", "{}")]]);
        let result = ask_in_session(
            &engine,
            &repo,
            &checked.session.id,
            "Finish the check step",
            &mut adapter,
        );

        assert_eq!(
            result.task.status,
            TaskStatus::Complete,
            "declined={declined}: an unapplied patch does not invalidate the check"
        );
        let report = engine
            .session_store
            .read_completion_report(&checked.session.id, &result.task.id)
            .unwrap()
            .expect("the completed task carries its report");
        assert!(report.eligible, "declined={declined}: {report:?}");
    }
}

/// R13 regression from the 2026-11-04 review of specs 101–105: a turn that
/// proposed a patch saved no paused turn, so the failing command it ran earlier
/// in the same turn was dropped instead of reaching the open step. The commonest
/// repair shape — run the check, it fails, propose the fix — then could not
/// block the step, and after the patch it closed `Completed` without proof.
fn engine_with_a_command_that_fails_until_the_patch_lands(repo: &Path) -> WorkspaceEngine {
    fs::write(
        repo.join("check"),
        "#!/bin/sh\nif [ -f src/added.rs ]; then exit 0; fi\nexec ls no-such-directory\n",
    )
    .unwrap();
    fs::set_permissions(repo.join("check"), fs::Permissions::from_mode(0o755)).unwrap();
    WorkspaceEngine::new(Config {
        command_allowlist: vec!["./check".to_string()],
        data_dir: repo.join(".damaian"),
        enable_index_watcher: false,
        ..Config::default()
    })
}

/// Runs a failing command and proposes a patch in the same resumed turn, after
/// its plan is approved. Returns the turn and the patch it left for review.
fn failing_command_then_a_patch_in_one_turn(
    engine: &WorkspaceEngine,
    repo: &Path,
) -> (ChatTurnResult, String) {
    let mut first = scripted(vec![
        vec![call(
            "propose_plan",
            r#"{"steps":[{"title":"Make the change"},{"title":"Run the check"}]}"#,
        )],
        // The mutation is what raises the plan review.
        vec![patch_call("src/added.rs")],
    ]);
    let paused = ask(engine, repo, "Fix the check", &mut first);
    let review = paused.plan_proposal.expect("the plan should be put up");

    // Close the first step, then fail the check on the second and propose the
    // fix — all in the one resumed turn.
    let mut after = scripted(vec![
        vec![call("complete_step", "{}")],
        vec![call(
            "run_command",
            r#"{"command":"./check","reason":"Check the fix"}"#,
        )],
        vec![patch_call("src/added.rs")],
    ]);
    let proposed = decide(engine, &review.id, true, None, &mut after);
    let patch = proposed
        .patch_proposal
        .as_ref()
        .expect("the patch is proposed while the step is open")
        .patch_id
        .clone();
    (proposed, patch)
}

#[test]
fn a_failing_command_before_a_patch_in_one_turn_reaches_the_open_step() {
    let repo = temp_repo("patch-keeps-failed-exit");
    let engine = engine_with_a_command_that_fails_until_the_patch_lands(&repo);
    let (proposed, _patch) = failing_command_then_a_patch_in_one_turn(&engine, &repo);

    let plan = plan_of(&engine, &proposed).expect("the plan should read back");
    assert_eq!(
        plan.steps[1].status,
        StepStatus::InProgress,
        "attaching is not closing: the step waits for complete_step"
    );
    let exit = plan.steps[1]
        .evidence
        .iter()
        .find_map(|evidence| match evidence {
            Evidence::CommandExit { exit_code, .. } => Some(*exit_code),
            _ => None,
        })
        .expect("the failing exit reaches the open step before the patch proposal stops the turn");
    assert_ne!(exit, Some(0), "the command failed: {:?}", plan.steps[1]);
    assert!(
        audit_events(&repo, "plan_left_open").is_empty(),
        "a turn that only pauses for the patch is not a turn end"
    );
}

#[test]
fn closing_a_step_after_a_patch_without_rerunning_the_command_keeps_it_blocked() {
    let repo = temp_repo("patch-without-rerun-blocked");
    let engine = engine_with_a_command_that_fails_until_the_patch_lands(&repo);
    let (proposed, patch) = failing_command_then_a_patch_in_one_turn(&engine, &repo);

    engine
        .edit_orchestrator
        .apply_stored_patch(&repo, &patch, None, None, "tester", false)
        .expect("the approved patch applies");
    let verified = run_verification_due(
        &engine,
        &repo,
        &proposed.session.id,
        &mut MockModelAdapter::new("Done."),
    );

    let mut adapter = scripted(vec![vec![call("complete_step", "{}")]]);
    let result = continue_task(
        &engine,
        &repo,
        &verified.session.id,
        &verified.task.id,
        &mut adapter,
    );
    let plan = plan_of(&engine, &result).expect("the plan should read back");
    assert_eq!(
        plan.steps[1].status,
        StepStatus::Blocked,
        "the failed command has not succeeded since: {:?}",
        plan.steps[1]
    );
    assert_eq!(
        result.task.status,
        TaskStatus::Incomplete,
        "a blocked step is not finished work"
    );
    let report = engine
        .session_store
        .read_completion_report(&result.session.id, &result.task.id)
        .unwrap()
        .expect("the continued task settles through the gate");
    assert!(
        report
            .reason_codes()
            .contains(&CompletionReasonCode::BlockedPlanSteps),
        "{report:?}"
    );
}

#[test]
fn rerunning_the_failed_command_after_the_patch_completes_the_step() {
    let repo = temp_repo("patch-rerun-clears");
    let engine = engine_with_a_command_that_fails_until_the_patch_lands(&repo);
    let (proposed, patch) = failing_command_then_a_patch_in_one_turn(&engine, &repo);

    engine
        .edit_orchestrator
        .apply_stored_patch(&repo, &patch, None, None, "tester", false)
        .expect("the approved patch applies");
    let verified = run_verification_due(
        &engine,
        &repo,
        &proposed.session.id,
        &mut MockModelAdapter::new("Done."),
    );

    let mut adapter = scripted(vec![
        vec![call(
            "run_command",
            r#"{"command":"./check","reason":"Run the same check again"}"#,
        )],
        vec![call("complete_step", "{}")],
    ]);
    let result = continue_task(
        &engine,
        &repo,
        &verified.session.id,
        &verified.task.id,
        &mut adapter,
    );
    let plan = plan_of(&engine, &result).expect("the plan should read back");
    assert_eq!(
        plan.steps[1].status,
        StepStatus::Completed,
        "the same command passed after the patch: {:?}",
        plan.steps[1]
    );
    assert_eq!(
        result.task.status,
        TaskStatus::Complete,
        "{:?}",
        result.task
    );
}

/// R4 plus R13: a passing deferred required check and the patch it validates in
/// the same turn used to lose the check's exit at the patch pause, so the gate
/// reported it missing. With the exit kept, the applied patch makes it stale —
/// the safe direction, and the one that tells the worker to rerun it.
#[test]
fn a_required_check_before_a_patch_in_one_turn_is_stale_not_missing() {
    let repo = temp_repo("check-then-patch-one-turn");
    let engine = engine_for(&repo);
    let mut first = scripted(vec![
        vec![call(
            "propose_plan",
            r#"{"steps":[
                {"title":"Make the change"},
                {"title":"Run the check","check":{"command":"ls","workingDirectory":"","purpose":"validates the final tree","required":true}}
            ]}"#,
        )],
        vec![patch_call("src/added.rs")],
    ]);
    let paused = ask(&engine, &repo, "Add retry handling", &mut first);
    let review = paused.plan_proposal.expect("the plan should be put up");

    let mut after = scripted(vec![
        vec![call("complete_step", "{}")],
        vec![call(
            "run_command",
            r#"{"command":"ls","reason":"Run the required check"}"#,
        )],
        vec![patch_call("src/added.rs")],
    ]);
    let proposed = decide(&engine, &review.id, true, None, &mut after);
    let patch = proposed
        .patch_proposal
        .as_ref()
        .expect("the patch is proposed while the check step is open")
        .patch_id
        .clone();

    let open_plan = plan_of(&engine, &proposed).expect("the open plan should read back");
    assert_eq!(open_plan.steps[1].status, StepStatus::InProgress);
    assert!(
        open_plan.steps[1]
            .evidence
            .iter()
            .any(|evidence| matches!(evidence, Evidence::CommandExit { .. })),
        "the passing check reaches the open step before the patch proposal: {:?}",
        open_plan.steps[1]
    );

    engine
        .edit_orchestrator
        .apply_stored_patch(&repo, &patch, None, None, "tester", false)
        .expect("the approved patch applies");
    let verified = run_verification_due(
        &engine,
        &repo,
        &proposed.session.id,
        &mut MockModelAdapter::new("Done."),
    );

    let mut adapter = scripted(vec![vec![call("complete_step", "{}")]]);
    let result = continue_task(
        &engine,
        &repo,
        &verified.session.id,
        &verified.task.id,
        &mut adapter,
    );
    assert_eq!(result.task.status, TaskStatus::Incomplete);
    let report = engine
        .session_store
        .read_completion_report(&result.session.id, &result.task.id)
        .unwrap()
        .expect("the continued task settles through the gate");
    assert_eq!(
        report.reason_codes(),
        vec![CompletionReasonCode::RequiredCheckStale],
        "{report:?}"
    );
}

/// The other half of the fix: a turn that *does* save a paused turn carries its
/// evidence in the pending file and attaches it only when it settles, so the new
/// patch-pause attach must not also fire for it and duplicate the evidence.
#[test]
fn a_command_pause_does_not_duplicate_its_evidence_on_resume() {
    let repo = temp_repo("pause-evidence-once");
    let engine = engine_for(&repo);
    let mut first = scripted(vec![
        vec![plan_call(&["Read the source", "Write the report"])],
        vec![call("read_file", r#"{"path":"src/a.rs"}"#)],
        vec![patch_call("src/retry.rs")],
    ]);
    let paused = ask(&engine, &repo, "Add retry handling", &mut first);
    let review = paused.plan_proposal.expect("the plan should be put up");

    let mut command_round = scripted(vec![vec![call(
        "run_command",
        r#"{"command":"sed -i '' 's/1/2/' src/a.rs","reason":"Bump the constant"}"#,
    )]]);
    let paused = approve_plan(&engine, &review.id, &mut command_round);
    let command = paused
        .command_proposal
        .expect("the command is put up for approval");

    let mut finish = scripted(vec![vec![call("complete_step", "{}")]]);
    let result = approve_command(&engine, &command.id, &mut finish);
    let plan = plan_of(&engine, &result).expect("the plan should read back");

    assert_eq!(
        plan.steps[0]
            .evidence
            .iter()
            .filter(|evidence| matches!(evidence, Evidence::FileRead { .. }))
            .count(),
        1,
        "the read carried across the pause is attached once: {:?}",
        plan.steps[0].evidence
    );
    assert_eq!(
        plan.steps[0]
            .evidence
            .iter()
            .filter(|evidence| matches!(evidence, Evidence::CommandExit { .. }))
            .count(),
        1,
        "the approved command's exit is attached once: {:?}",
        plan.steps[0].evidence
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
        check: PlanRevisionCheck::Preserve,
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
            tool_call_markup_recovered: false,
            upstream_provider: None,
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

/// Spec 94: a plan put up for review can be handed to a worker. The prompt
/// carries the request and the plan as the card has it, and reading it leaves
/// the pause alone; handing over ends the turn with nothing changed here.
#[test]
fn a_plan_under_review_is_handed_to_a_worker() {
    let repo = temp_repo("hand-over");
    let engine = engine_for(&repo);
    let mut adapter = scripted(vec![
        vec![call(
            "propose_plan",
            r#"{"steps":[{"title":"Read the retry helper"},{"title":"Add a bounded backoff"}]}"#,
        )],
        vec![patch_call("src/retry.rs")],
    ]);
    let paused = ask(
        &engine,
        &repo,
        "Add retry handling to the upload client",
        &mut adapter,
    );
    let proposal = paused
        .plan_proposal
        .clone()
        .expect("the plan should be put up");
    let plan = plan_of(&engine, &paused).expect("a plan");

    let handoff = engine
        .chat_orchestrator
        .plan_worker_prompt(&proposal.id, None)
        .unwrap();
    assert_eq!(
        handoff.task_id, paused.task.id,
        "the spawn request must carry the paused plan task, not the plan proposal id"
    );
    let prompt = handoff.prompt;
    assert!(
        prompt.starts_with("Add retry handling to the upload client"),
        "{prompt}"
    );
    assert!(
        prompt.contains("1. Read the retry helper\n2. Add a bounded backoff"),
        "{prompt}"
    );
    // Handing over is not an approval: the worker's own review is the one
    // that counts, so the prompt must not claim the user approved the plan.
    assert!(prompt.contains("has not approved it yet"), "{prompt}");
    assert!(!prompt.contains("user reviewed this plan"), "{prompt}");
    let revised = engine
        .chat_orchestrator
        .plan_worker_prompt(
            &proposal.id,
            Some(vec![PlanRevisionStep {
                id: plan.steps[1].id.clone(),
                title: "Add a capped backoff".to_string(),
                check: PlanRevisionCheck::Preserve,
            }]),
        )
        .unwrap()
        .prompt;
    assert!(revised.contains("1. Add a capped backoff\n"), "{revised}");
    assert!(!revised.contains("Read the retry helper"), "{revised}");

    engine
        .chat_orchestrator
        .hand_plan_to_worker(&proposal.id, "session_worker", "tester")
        .unwrap();

    let task = engine
        .session_store
        .read_tasks(&paused.session.id)
        .unwrap()
        .into_iter()
        .find(|task| task.id == paused.task.id)
        .unwrap();
    assert_eq!(task.status, TaskStatus::Complete);
    // Spec 101: handing the plan off still settles through the shared gate,
    // so the completion has a durable report rather than a bypass.
    let report = engine
        .session_store
        .read_completion_report(&paused.session.id, &paused.task.id)
        .unwrap()
        .expect("the hand-off settlement carries its report");
    assert!(report.eligible, "{report:?}");
    let plan = plan_of(&engine, &paused).unwrap();
    assert!(
        plan.steps
            .iter()
            .all(|step| step.status == StepStatus::Skipped),
        "{plan:?}"
    );
    assert!(!repo.join("src/retry.rs").exists(), "nothing changed here");
    let messages = engine
        .session_store
        .read_messages(&paused.session.id)
        .unwrap();
    let note = messages
        .iter()
        .rev()
        .find(|message| message.role == "assistant")
        .expect("a hand-over note");
    assert!(
        note.content
            .contains("asks you to approve the plan there before its first change"),
        "{}",
        note.content
    );
    assert!(
        engine
            .chat_orchestrator
            .hand_plan_to_worker(&proposal.id, "session_worker", "tester")
            .is_err(),
        "the pause is answered once"
    );
}

// --- R14: required-check baselines and uncertain provenance (spec 102) ------
//
// A required check is baselined before the first patch the task writes, on the
// untouched tree. A required check that first becomes required only after the
// task has changed files has no task-start baseline; its failure is
// `uncertain`, is reported, and asks the user whether to repair instead of
// silently treating the failure as pre-existing or repairing it automatically.
// Per-change checks keep their existing pre-patch baseline (spec 102 D2 piece
// 1).

const REQUIRED_FOO_PLAN: &str = r#"{"steps":[
    {"title":"Change the shared value"},
    {"title":"Validate","check":{"command":"cargo test","workingDirectory":"crates/foo","purpose":"validates the shared value","required":true}},
    {"title":"Change the foo crate"}
]}"#;

const PLAIN_FOO_PLAN: &str = r#"{"steps":[
    {"title":"Change the shared value"},
    {"title":"Validate the shared value"},
    {"title":"Change the foo crate"}
]}"#;

/// A fixture with two project roots. The fake shell makes `cargo test` in
/// `crates/foo` depend on the repository-root `shared.txt`, while the root's
/// own `cargo test` always passes. `write-broken` changes the shared value
/// without a patch, which is how a check can become required after the task has
/// already changed files.
fn required_check_fixture(name: &str) -> (PathBuf, PathBuf) {
    let repo = temp_repo(name);
    fs::create_dir_all(repo.join("crates/foo/src")).expect("the second root should exist");
    fs::write(
        repo.join("Cargo.toml"),
        "[package]\nname = \"root\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .expect("the root manifest");
    fs::write(
        repo.join("crates/foo/Cargo.toml"),
        "[package]\nname = \"foo\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .expect("the member manifest");
    fs::write(
        repo.join("crates/foo/src/lib.rs"),
        "pub fn value() -> u8 { 1 }\n",
    )
    .expect("the member source");
    fs::write(repo.join("shared.txt"), "ok\n").expect("the shared value");
    let shell = repo.join("check-shell.sh");
    let script = format!(
        "#!/bin/sh\n\
         command=\"$2\"\n\
         case \"$command\" in\n\
           *write-broken*) printf 'broken\\n' > '{}/shared.txt' ;;\n\
           *\"cargo test\"*)\n\
             case \"$PWD\" in\n\
               */crates/foo) grep -q '^ok$' \"$PWD/../../shared.txt\" || exit 1 ;;\n\
             esac\n\
             ;;\n\
           *) exit 0 ;;\n\
         esac\n\
         exit 0\n",
        repo.display()
    );
    fs::write(&shell, script).expect("the fake shell");
    let mut permissions = fs::metadata(&shell)
        .expect("the fake shell metadata")
        .permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&shell, permissions).expect("the fake shell is executable");
    (repo, shell)
}

fn check_engine(repo: &Path, shell: &Path, allow: &[&str]) -> WorkspaceEngine {
    WorkspaceEngine::new(Config {
        data_dir: repo.join(".damaian"),
        enable_index_watcher: false,
        shell: shell.to_string_lossy().to_string(),
        command_allowlist: allow.iter().map(|entry| (*entry).to_string()).collect(),
        ..Config::default()
    })
}

fn patch_call_with(path: &str, content: &str) -> ToolCall {
    call(
        "propose_patch",
        &serde_json::json!({
            "summary": format!("Change {path}"),
            "files": [{"path": path, "content": content}],
        })
        .to_string(),
    )
}

/// Applies a stored patch through the ordinary pre-apply baseline gate,
/// answering every offered baseline with one decision.
fn apply_patch_with_baselines(
    engine: &WorkspaceEngine,
    repo: &Path,
    patch_id: &str,
    approve_baselines: bool,
) {
    let mut outcome = engine
        .edit_orchestrator
        .apply_stored_patch_with_baseline(repo, patch_id, None, None, "tester", false)
        .expect("the patch should apply");
    while let PatchApplyOutcome::BaselineRequired(batch) = outcome {
        let decisions = batch
            .checks
            .iter()
            .map(|check| (check.proposal_id.clone(), approve_baselines))
            .collect::<Vec<_>>();
        engine
            .edit_orchestrator
            .resume_stored_patch_baseline(repo, &batch.id, &decisions, &[], "tester")
            .expect("the baseline decisions should record");
        outcome = engine
            .edit_orchestrator
            .apply_stored_patch_with_baseline(repo, patch_id, None, None, "tester", false)
            .expect("the patch should apply after the baselines settle");
    }
    assert!(
        matches!(outcome, PatchApplyOutcome::Applied(_)),
        "the patch should be applied: {outcome:?}"
    );
}

fn mark_worker_session(engine: &WorkspaceEngine, session_id: &str, root: &Path) {
    engine
        .session_store
        .record_worker_spawned(
            session_id,
            &WorkerRecord {
                parent_session_id: None,
                parent_task_id: None,
                inherited_protected_paths: Vec::new(),
                source_repository: root.to_path_buf(),
                worktree_path: root.to_path_buf(),
                branch: "damaian/fixture".to_string(),
                base_commit: "HEAD".to_string(),
                included_uncommitted: false,
                git_commands: Vec::new(),
            },
        )
        .expect("the worker record is written");
}

fn try_approve_command(
    engine: &WorkspaceEngine,
    proposal_id: &str,
    adapter: &mut dyn ModelAdapter,
) -> workspace_engine::Result<ChatTurnResult> {
    let cancel = CancelToken::new();
    let mut on_token = |_token: &str| {};
    let mut on_progress = |_event: TurnProgress| {};
    let mut sink = TurnSink {
        on_token: &mut on_token,
        on_progress: &mut on_progress,
        cancel: &cancel,
        steering: None,
    };
    engine.chat_orchestrator.resume_after_command_decision(
        proposal_id,
        true,
        "tester",
        adapter,
        &mut sink,
    )
}

fn answer_question(
    engine: &WorkspaceEngine,
    question_id: &str,
    answer: &str,
    selected_option: Option<usize>,
    adapter: &mut dyn ModelAdapter,
) -> ChatTurnResult {
    try_answer_question(engine, question_id, answer, selected_option, adapter)
        .expect("the answer should resume the turn")
}

fn try_answer_question(
    engine: &WorkspaceEngine,
    question_id: &str,
    answer: &str,
    selected_option: Option<usize>,
    adapter: &mut dyn ModelAdapter,
) -> workspace_engine::Result<ChatTurnResult> {
    let cancel = CancelToken::new();
    let mut on_token = |_token: &str| {};
    let mut on_progress = |_event: TurnProgress| {};
    let mut sink = TurnSink {
        on_token: &mut on_token,
        on_progress: &mut on_progress,
        cancel: &cancel,
        steering: None,
    };
    engine.chat_orchestrator.resume_after_answer(
        question_id,
        answer,
        selected_option,
        adapter,
        &mut sink,
    )
}

fn request_text(adapter: &MockModelAdapter) -> String {
    adapter
        .requests
        .iter()
        .flat_map(|request| request.messages.iter())
        .map(|message| message.content.clone())
        .collect::<Vec<_>>()
        .join("\n")
}

/// (a) The required check is baselined before patch 1, on the untouched tree.
/// A regression first observed after a later patch is therefore `introduced`
/// and starts an ordinary repair turn.
#[test]
fn a_required_check_is_baselined_at_task_start_and_a_later_regression_is_repaired() {
    let (repo, shell) = required_check_fixture("r14-task-start");
    let engine = check_engine(&repo, &shell, &["cargo test"]);
    let patch1 = patch_call_with("shared.txt", "broken\n");
    let mut first = scripted(vec![
        vec![call("propose_plan", REQUIRED_FOO_PLAN)],
        vec![patch1.clone()],
    ]);
    let paused = ask(&engine, &repo, "Change the shared value", &mut first);
    let review = paused.plan_proposal.expect("the plan is put up");

    let mut resumed = scripted(vec![vec![patch1]]);
    let proposed = decide(&engine, &review.id, true, None, &mut resumed);
    let patch1_id = proposed
        .patch_proposal
        .as_ref()
        .expect("patch one is proposed")
        .patch_id
        .clone();
    apply_patch_with_baselines(&engine, &repo, &patch1_id, true);

    let foo = CheckId::new("crates/foo", "cargo test");
    let baselines = engine
        .session_store
        .task_verification_baselines(&proposed.session.id, &proposed.task.id)
        .expect("the baselines should read");
    assert!(
        matches!(baselines.get(&foo), Some(CheckBaseline::Passed)),
        "the required check is baselined before patch 1 on the untouched tree: {baselines:?}"
    );

    // No verification runs between the two patches, so patch 1's regression is
    // first observed after patch 2. Complete the first two steps in a later
    // turn and propose the second patch.
    let patch2 = patch_call_with("crates/foo/src/lib.rs", "pub fn value() -> u8 { 2 }\n");
    let mut after_first = scripted(vec![
        vec![call("complete_step", "{}"), call("complete_step", "{}")],
        vec![patch2],
    ]);
    let continued = ask_in_session(
        &engine,
        &repo,
        &proposed.session.id,
        "Continue with the foo crate",
        &mut after_first,
    );
    let patch2_id = continued
        .patch_proposal
        .as_ref()
        .expect("patch two is proposed in the later turn")
        .patch_id
        .clone();
    apply_patch_with_baselines(&engine, &repo, &patch2_id, true);

    let mut repair_adapter = MockModelAdapter::new("Done.");
    let verified = run_verification_and_approve_checks(
        &engine,
        &repo,
        &continued.session.id,
        &mut repair_adapter,
    );
    let report = verified
        .verification_report
        .expect("the second verification has a report");
    let introduced = report
        .provenance
        .iter()
        .find(|partition| partition.provenance == FailureProvenance::NewRegression)
        .unwrap_or_else(|| {
            panic!("the later regression is introduced by the task-start baseline: {report:?}")
        });
    assert!(
        introduced.checks.iter().any(|check| check.check == foo),
        "the required check is the introduced failure: {introduced:?}"
    );
    assert!(
        request_text(&repair_adapter).contains("Repair only the failures this patch introduced"),
        "the introduced failure starts an ordinary repair turn"
    );
}

/// Applies the uncertain scenario: a command changes a file before the plan
/// exists, the plan then declares a required check, and the next patch lands.
/// The check has no task-start baseline, so its later failure is `uncertain` and
/// the worker asks whether to repair. Returns the engine, repository, the
/// verification task, the question id and the report.
fn uncertain_repair_fixture(
    name: &str,
) -> (
    WorkspaceEngine,
    PathBuf,
    ChatTurnResult,
    String,
    workspace_engine::VerificationReport,
) {
    let (repo, shell) = required_check_fixture(name);
    let engine = check_engine(&repo, &shell, &["cargo test"]);
    let patch1 = patch_call_with("shared.txt", "broken\n");
    let mut adapter = scripted(vec![
        vec![call("propose_plan", PLAIN_FOO_PLAN)],
        vec![patch1.clone()],
    ]);
    let paused = ask(&engine, &repo, "Change the shared value", &mut adapter);
    let review = paused.plan_proposal.expect("the plan is put up");

    let mut resumed = scripted(vec![vec![patch1]]);
    let proposed = decide(&engine, &review.id, true, None, &mut resumed);
    let patch1_id = proposed
        .patch_proposal
        .as_ref()
        .expect("patch one is proposed")
        .patch_id
        .clone();
    apply_patch_with_baselines(&engine, &repo, &patch1_id, true);

    // The check becomes required only after patch 1 landed: this is the
    // durable plan-revision step, not a check the task started with.
    let mut plan = engine
        .session_store
        .read_task_plan(&proposed.session.id, &proposed.task.id)
        .expect("the plan should read")
        .expect("patch one's task carries the plan");
    plan.steps[1].check = Some(PlannedCheck {
        version: PLANNED_CHECK_VERSION,
        command: "cargo test".to_string(),
        working_directory: "crates/foo".to_string(),
        purpose: "validates the shared value".to_string(),
        required: true,
    });
    engine
        .session_store
        .revise_plan(&proposed.task, &plan)
        .expect("the plan revision records");

    let patch2 = patch_call_with("crates/foo/src/lib.rs", "pub fn value() -> u8 { 2 }\n");
    let mut after_first = scripted(vec![
        vec![call("complete_step", "{}"), call("complete_step", "{}")],
        vec![patch2],
    ]);
    let continued = ask_in_session(
        &engine,
        &repo,
        &proposed.session.id,
        "Continue with the foo crate",
        &mut after_first,
    );
    let patch2_id = continued
        .patch_proposal
        .as_ref()
        .expect("patch two is proposed in the later turn")
        .patch_id
        .clone();
    apply_patch_with_baselines(&engine, &repo, &patch2_id, true);
    mark_worker_session(&engine, &continued.session.id, &repo);

    let mut verification_adapter = MockModelAdapter::new("Done.");
    let verified = run_verification_and_approve_checks(
        &engine,
        &repo,
        &continued.session.id,
        &mut verification_adapter,
    );
    assert!(
        verification_adapter.requests.is_empty(),
        "an uncertain failure starts no automatic repair turn"
    );
    let question_id = verified
        .question_proposal
        .as_ref()
        .expect("the worker asks whether to repair the uncertain failure")
        .question_id
        .clone();
    let report = verified
        .verification_report
        .clone()
        .expect("the uncertain verification has a report");
    (engine, repo, verified, question_id, report)
}

/// (b) A required check first declared after the task already changed files has
/// no task-start baseline. Its failure is `uncertain`, named in the report,
/// starts no repair turn, and raises a question.
#[test]
fn a_required_check_first_required_after_changes_is_uncertain_and_asks_before_repair() {
    let (_engine, repo, verified, _question_id, report) = uncertain_repair_fixture("r14-uncertain");
    assert!(
        report
            .provenance
            .iter()
            .any(|partition| partition.provenance.as_str() == "uncertain"),
        "the required check's failure is uncertain, not pre-existing: {report:?}"
    );
    assert!(
        report.repair_targets.is_empty(),
        "an uncertain failure is never an automatic repair target: {report:?}"
    );
    assert_eq!(
        verified.task.status,
        TaskStatus::WaitingForApproval,
        "the worker waits for the user's repair decision"
    );
    assert!(
        !fs::read_to_string(repo.join("crates/foo/src/lib.rs"))
            .unwrap()
            .contains("value() -> u8 { 3 }"),
        "nothing is repaired before the user answers"
    );
}

/// (c) Answering the question with repair runs an ordinary repair turn; its
/// patch still needs the ordinary patch approval. A question answer is never a
/// patch or command approval, in either direction.
#[test]
fn answering_the_uncertain_repair_question_with_repair_runs_one_repair_turn_that_still_needs_approval()
 {
    let (engine, repo, _verified, question_id, report) =
        uncertain_repair_fixture("r14-answer-repair");
    let foo = CheckId::new("crates/foo", "cargo test");
    let uncertain = report
        .provenance
        .iter()
        .find(|partition| partition.provenance.as_str() == "uncertain")
        .expect("the report names the uncertain failure");
    assert!(
        uncertain.checks.iter().any(|check| check.check == foo),
        "{uncertain:?}"
    );

    // A command approval route cannot answer a question.
    let refusal = try_approve_command(&engine, &question_id, &mut MockModelAdapter::new("Done."))
        .expect_err("a question is not a command approval");
    assert!(
        refusal.to_string().contains("question"),
        "the refusal names the pause it actually is: {refusal}"
    );

    let repair_patch = patch_call_with("crates/foo/src/lib.rs", "pub fn value() -> u8 { 3 }\n");
    let mut repair_adapter = scripted(vec![vec![repair_patch]]);
    let result = answer_question(
        &engine,
        &question_id,
        "Repair it",
        Some(0),
        &mut repair_adapter,
    );
    let patch = result
        .patch_proposal
        .as_ref()
        .expect("the repair turn proposes a patch");
    assert_eq!(
        result.task.status,
        TaskStatus::WaitingForApproval,
        "the repair patch still waits for its own approval"
    );
    assert!(
        request_text(&repair_adapter).contains("Repair only the failures this patch introduced"),
        "answering repair runs an ordinary repair turn"
    );
    assert!(
        !fs::read_to_string(repo.join("crates/foo/src/lib.rs"))
            .unwrap()
            .contains("value() -> u8 { 3 }"),
        "the repair patch is not applied by its proposal"
    );

    // The other direction: a command approval cannot be answered as a
    // question. The command pause is created through the ordinary turn path.
    let mut command_adapter = scripted(vec![vec![call(
        "run_command",
        r#"{"command":"touch needs-approval.txt","reason":"Write a marker"}"#,
    )]]);
    let command_pause = ask_in_session(
        &engine,
        &repo,
        &result.session.id,
        "Write a marker",
        &mut command_adapter,
    );
    let command_id = command_pause
        .command_proposal
        .as_ref()
        .expect("the command waits for approval")
        .id
        .clone();
    let refusal = try_answer_question(
        &engine,
        &command_id,
        "Repair it",
        Some(0),
        &mut MockModelAdapter::new("Done."),
    )
    .expect_err("a command approval is not a question answer");
    assert!(
        refusal.to_string().contains("approval"),
        "the refusal names the pause it actually is: {refusal}"
    );
    assert!(
        patch.patch_id.starts_with("patch_"),
        "the repair turn proposed a real patch waiting for its own approval"
    );
}

/// (c) Answering the question with no leaves the failure reported and does not
/// repair it.
#[test]
fn answering_the_uncertain_repair_question_with_no_leaves_it_reported_and_unrepaired() {
    let (engine, repo, _verified, question_id, _report) = uncertain_repair_fixture("r14-answer-no");
    let mut adapter = MockModelAdapter::new("Done.");
    let result = answer_question(
        &engine,
        &question_id,
        "Leave it unrepaired",
        Some(1),
        &mut adapter,
    );
    assert!(
        result.patch_proposal.is_none(),
        "no repair patch is proposed when the user declines"
    );
    let report = result
        .verification_report
        .expect("the declined question still reports the failure");
    assert!(
        report
            .provenance
            .iter()
            .any(|partition| partition.provenance.as_str() == "uncertain"),
        "the failure stays reported as uncertain: {report:?}"
    );
    assert!(
        fs::read_to_string(repo.join("crates/foo/src/lib.rs"))
            .unwrap()
            .contains("value() -> u8 { 2 }"),
        "the tree is unchanged when the user declines"
    );
}

/// (d) A per-change check keeps its existing pre-patch baseline. It fails
/// before the later patch, so the later failure is `pre_existing` and is never
/// promoted to `uncertain` or repaired.
#[test]
fn a_per_change_check_keeps_its_pre_patch_baseline_and_pre_existing_classification() {
    let (repo, shell) = required_check_fixture("r14-per-change");
    let engine = check_engine(&repo, &shell, &["cargo test", "write-broken"]);
    let patch1 = patch_call_with("shared.txt", "broken\n");
    let mut first = scripted(vec![
        vec![call("propose_plan", PLAIN_FOO_PLAN)],
        vec![patch1.clone()],
    ]);
    let paused = ask(&engine, &repo, "Change the shared value", &mut first);
    let review = paused.plan_proposal.expect("the plan is put up");

    let mut resumed = scripted(vec![vec![patch1]]);
    let proposed = decide(&engine, &review.id, true, None, &mut resumed);
    let patch1_id = proposed
        .patch_proposal
        .as_ref()
        .expect("patch one is proposed")
        .patch_id
        .clone();
    apply_patch_with_baselines(&engine, &repo, &patch1_id, true);

    let patch2 = patch_call_with("crates/foo/src/lib.rs", "pub fn value() -> u8 { 2 }\n");
    let mut after_first = scripted(vec![
        vec![call("complete_step", "{}"), call("complete_step", "{}")],
        vec![patch2],
    ]);
    let continued = ask_in_session(
        &engine,
        &repo,
        &proposed.session.id,
        "Continue with the foo crate",
        &mut after_first,
    );
    let patch2_id = continued
        .patch_proposal
        .as_ref()
        .expect("patch two is proposed in the later turn")
        .patch_id
        .clone();
    apply_patch_with_baselines(&engine, &repo, &patch2_id, true);

    let foo = CheckId::new("crates/foo", "cargo test");
    let baselines = engine
        .session_store
        .task_verification_baselines(&continued.session.id, &continued.task.id)
        .expect("the baselines should read");
    assert!(
        matches!(baselines.get(&foo), Some(CheckBaseline::Failed { .. })),
        "the per-change check keeps its pre-patch baseline, taken with patch 1 applied: {baselines:?}"
    );

    mark_worker_session(&engine, &continued.session.id, &repo);
    let mut verification_adapter = MockModelAdapter::new("Done.");
    let verified = run_verification_and_approve_checks(
        &engine,
        &repo,
        &continued.session.id,
        &mut verification_adapter,
    );
    let report = verified
        .verification_report
        .expect("the verification has a report");
    let pre_existing = report
        .provenance
        .iter()
        .find(|partition| partition.provenance == FailureProvenance::PreExisting)
        .unwrap_or_else(|| panic!("the per-change failure is pre-existing: {report:?}"));
    assert!(
        pre_existing.checks.iter().any(|check| check.check == foo),
        "{pre_existing:?}"
    );
    assert!(report.repair_targets.is_empty(), "{report:?}");
    assert!(
        verified.question_proposal.is_none(),
        "a pre-existing failure is not an uncertain repair question"
    );
}

/// (e) A required check that needs approval is offered on the existing baseline
/// card before the first patch. Declining it records `Declined`, not
/// `Uncertain`.
#[test]
fn a_declined_required_baseline_is_recorded_declined_not_uncertain() {
    let (repo, shell) = required_check_fixture("r14-declined-required");
    // Every command needs approval, so the required check reaches the ordinary
    // baseline card instead of running as a safe detected check.
    let engine = WorkspaceEngine::new(Config {
        data_dir: repo.join(".damaian"),
        enable_index_watcher: false,
        shell: shell.to_string_lossy().to_string(),
        require_approval_for_all_commands: true,
        ..Config::default()
    });
    let patch = patch_call_with("shared.txt", "broken\n");
    let mut first = scripted(vec![
        vec![call("propose_plan", REQUIRED_FOO_PLAN)],
        vec![patch.clone()],
    ]);
    let paused = ask(&engine, &repo, "Change the shared value", &mut first);
    let review = paused.plan_proposal.expect("the plan is put up");

    let mut resumed = scripted(vec![vec![patch]]);
    let proposed = decide(&engine, &review.id, true, None, &mut resumed);
    let patch_id = proposed
        .patch_proposal
        .as_ref()
        .expect("the patch is proposed")
        .patch_id
        .clone();

    let outcome = engine
        .edit_orchestrator
        .apply_stored_patch_with_baseline(&repo, &patch_id, None, None, "tester", false)
        .expect("the patch should reach the baseline gate");
    let PatchApplyOutcome::BaselineRequired(batch) = outcome else {
        panic!("a required check that needs approval pauses before the first patch: {outcome:?}");
    };
    let foo = CheckId::new("crates/foo", "cargo test");
    assert!(
        batch.checks.iter().any(|check| check.check == foo),
        "the required check is on the existing baseline card: {batch:?}"
    );
    let decisions = batch
        .checks
        .iter()
        .map(|check| (check.proposal_id.clone(), false))
        .collect::<Vec<_>>();
    engine
        .edit_orchestrator
        .resume_stored_patch_baseline(&repo, &batch.id, &decisions, &[], "tester")
        .expect("declining every baseline records the decision");

    let baselines = engine
        .session_store
        .task_verification_baselines(&proposed.session.id, &proposed.task.id)
        .expect("the baselines should read");
    assert!(
        matches!(baselines.get(&foo), Some(CheckBaseline::Declined)),
        "a declined required baseline is Declined, not uncertain: {baselines:?}"
    );
}
