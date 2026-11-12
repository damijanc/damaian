//! Review fix R8: a declined command that matches a required deferred check is
//! the named waiver the completion gate already understands for a `CheckRun`.
//!
//! A plan-declared required check deferred to its step runs as an ordinary
//! `run_command`. Before this fix, declining that command recorded nothing the
//! gate read as a waiver, so the task could never complete. These tests drive
//! the decline through the orchestrator and the normal command-approval route.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use workspace_engine::{
    CancelToken, ChatTurnResult, CheckId, CompletionReasonCode, Config, MockModelAdapter,
    ModelAdapter, TaskStatus, ToolCall, TurnProgress, TurnSink, WorkspaceEngine, fold_verification,
};

static COUNTER: AtomicU64 = AtomicU64::new(1);

fn temp_repo(name: &str) -> PathBuf {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock should work")
        .as_nanos();
    let repo = std::env::temp_dir().join(format!(
        "damaian-deferred-waiver-{name}-{now}-{}",
        COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir_all(repo.join("src")).expect("repository should be created");
    fs::write(repo.join("src/a.rs"), "fn main() {}\n").expect("a file to read");
    repo
}

fn engine_with_shell(repo: &Path, shell: &str) -> WorkspaceEngine {
    WorkspaceEngine::new(Config {
        data_dir: repo.join(".damaian"),
        enable_index_watcher: false,
        shell: shell.to_string(),
        ..Config::default()
    })
}

fn engine_for(repo: &Path) -> WorkspaceEngine {
    engine_with_shell(repo, "/usr/bin/true")
}

fn call(name: &str, arguments_json: &str) -> ToolCall {
    ToolCall {
        id: format!("call_{name}"),
        name: name.to_string(),
        arguments_json: arguments_json.to_string(),
    }
}

fn scripted(rounds: Vec<Vec<ToolCall>>) -> MockModelAdapter {
    let mut responses: Vec<String> = rounds.iter().map(|_| String::new()).collect();
    let mut calls = rounds;
    responses.push("Done.".to_string());
    calls.push(Vec::new());
    MockModelAdapter::new_sequence_with_tool_calls(responses, calls)
}

fn ask(
    engine: &WorkspaceEngine,
    repo: &Path,
    prompt: &str,
    adapter: &mut dyn ModelAdapter,
) -> ChatTurnResult {
    with_sink(|sink| {
        engine
            .chat_orchestrator
            .ask_with_session(repo, prompt, &[], None, adapter, sink)
            .expect("the turn should run")
    })
}

fn ask_in_session(
    engine: &WorkspaceEngine,
    repo: &Path,
    session_id: &str,
    prompt: &str,
    adapter: &mut dyn ModelAdapter,
) -> ChatTurnResult {
    with_sink(|sink| {
        engine
            .chat_orchestrator
            .ask_with_session(repo, prompt, &[], Some(session_id), adapter, sink)
            .expect("the turn should run")
    })
}

fn with_sink<T>(run: impl FnOnce(&mut TurnSink<'_>) -> T) -> T {
    let cancel = CancelToken::new();
    let mut on_token = |_token: &str| {};
    let mut on_progress = |_event: TurnProgress| {};
    let mut sink = TurnSink {
        on_token: &mut on_token,
        on_progress: &mut on_progress,
        cancel: &cancel,
        steering: None,
    };
    run(&mut sink)
}

fn decide_plan(
    engine: &WorkspaceEngine,
    proposal_id: &str,
    adapter: &mut dyn ModelAdapter,
) -> ChatTurnResult {
    with_sink(|sink| {
        engine
            .chat_orchestrator
            .resume_after_plan_decision(proposal_id, true, None, "tester", adapter, sink)
            .expect("the plan decision should resume the turn")
    })
}

fn decide_command(
    engine: &WorkspaceEngine,
    proposal_id: &str,
    approved: bool,
    adapter: &mut dyn ModelAdapter,
) -> ChatTurnResult {
    with_sink(|sink| {
        engine
            .chat_orchestrator
            .resume_after_command_decision(proposal_id, approved, "tester", adapter, sink)
            .expect("the command decision should resume the turn")
    })
}

fn run_verification_due(
    engine: &WorkspaceEngine,
    repo: &Path,
    session_id: &str,
    adapter: &mut dyn ModelAdapter,
) -> ChatTurnResult {
    with_sink(|sink| {
        engine
            .chat_orchestrator
            .verify_due(repo, session_id, adapter, sink)
            .expect("the due verification should run")
    })
}

fn resume_checks(
    engine: &WorkspaceEngine,
    batch_id: &str,
    decisions: &[(String, bool)],
    adapter: &mut dyn ModelAdapter,
) -> ChatTurnResult {
    with_sink(|sink| {
        engine
            .chat_orchestrator
            .resume_after_check_decision(batch_id, decisions, "tester", adapter, sink)
            .expect("the check batch should resume")
    })
}

/// Approves a command and answers any task-start baseline card it raises. The
/// baseline checks run before the command (spec 102 R18), so a command that is
/// itself a required check now has to be baselined first.
fn approve_command_through_baselines(
    engine: &WorkspaceEngine,
    proposal_id: &str,
    adapter: &mut dyn ModelAdapter,
) -> ChatTurnResult {
    let mut result = decide_command(engine, proposal_id, true, adapter);
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

fn patch_call(path: &str) -> ToolCall {
    call(
        "propose_patch",
        &format!(
            r#"{{"summary":"Add {path}","files":[{{"path":"{path}","content":"pub fn added() {{}}\n"}}]}}"#
        ),
    )
}

const CHECK_ONLY_PLAN: &str = r#"{"steps":[
    {"title":"Make the change"},
    {"title":"Run the check","check":{"command":"cargo test","workingDirectory":"","purpose":"validates the final tree","required":true}}
]}"#;

const SUBDIRECTORY_CHECK_PLAN: &str = r#"{"steps":[
    {"title":"Make the change"},
    {"title":"Run the check","check":{"command":"cargo test","workingDirectory":"crates/foo","purpose":"validates the final tree","required":true}}
]}"#;

const RUN_CARGO_TEST: &str = r#"{"command":"cargo test","reason":"Run the required check"}"#;

const RUN_CARGO_CHECK: &str = r#"{"command":"cargo check","reason":"A different check"}"#;

/// Proposes the plan, lets a patch raise the plan review, approves it, then
/// asks the model to run `command_json` so the ordinary command-approval route
/// pauses. Returns the paused command proposal id.
fn plan_approved_and_command_paused(
    engine: &WorkspaceEngine,
    repo: &Path,
    plan_json: &str,
    command_json: &str,
) -> String {
    let mut first = scripted(vec![
        vec![call("propose_plan", plan_json)],
        // A mutation is what raises the plan review; it is deferred and never
        // runs, because the approval answers by asking for the check instead.
        vec![patch_call("src/added.rs")],
    ]);
    let paused = ask(engine, repo, "Run the required check", &mut first);
    let review = paused.plan_proposal.expect("the plan should be put up");
    // The check lives on the final step, so the earlier step closes first.
    let mut after_plan = scripted(vec![
        vec![call("complete_step", "{}")],
        vec![call("run_command", command_json)],
    ]);
    let paused = decide_plan(engine, &review.id, &mut after_plan);
    paused
        .command_proposal
        .expect("the check still needs command approval")
        .id
}

fn report_for(
    engine: &WorkspaceEngine,
    result: &ChatTurnResult,
) -> workspace_engine::CompletionGateReport {
    engine
        .session_store
        .read_completion_report(&result.session.id, &result.task.id)
        .expect("the report should read")
        .expect("the settlement carries the gate report")
}

#[test]
fn declining_the_deferred_required_check_is_a_named_waiver_and_completes() {
    let repo = temp_repo("declined-check");
    let engine = engine_for(&repo);
    let command_id =
        plan_approved_and_command_paused(&engine, &repo, CHECK_ONLY_PLAN, RUN_CARGO_TEST);

    // The user declines exactly the required check's command, then the model
    // closes its step and answers.
    let mut finish = scripted(vec![vec![call("complete_step", "{}")]]);
    let result = decide_command(&engine, &command_id, false, &mut finish);

    assert_eq!(
        result.task.status,
        TaskStatus::Complete,
        "a declined required check is a waiver, not a permanent blocker: {result:?}"
    );
    let report = report_for(&engine, &result);
    assert!(report.eligible, "{report:?}");
    assert!(report.reasons.is_empty(), "{report:?}");
    assert_eq!(report.waivers.len(), 1, "{report:?}");
    assert_eq!(
        report.waivers[0].check,
        CheckId::new("", "cargo test"),
        "the waiver names the check the user declined"
    );
}

#[test]
fn declining_an_unrelated_command_is_not_a_waiver() {
    let repo = temp_repo("declined-unrelated");
    let engine = engine_for(&repo);
    let command_id =
        plan_approved_and_command_paused(&engine, &repo, CHECK_ONLY_PLAN, RUN_CARGO_CHECK);

    let mut finish = scripted(vec![vec![call("complete_step", "{}")]]);
    let result = decide_command(&engine, &command_id, false, &mut finish);

    assert_eq!(
        result.task.status,
        TaskStatus::Incomplete,
        "declining a different command says nothing about the required check: {result:?}"
    );
    let report = report_for(&engine, &result);
    assert!(report.waivers.is_empty(), "{report:?}");
    // Since spec 106 Task 6 `complete_step` refuses to close the step before
    // its required check is answered, and this decline does not answer it: the
    // step stays open rather than closing on a missing check.
    assert_eq!(
        report.reason_codes(),
        vec![CompletionReasonCode::OpenPlanSteps],
        "{report:?}"
    );
}

#[test]
fn declining_the_same_command_in_another_directory_is_not_a_waiver() {
    let repo = temp_repo("declined-wrong-directory");
    fs::create_dir_all(repo.join("crates/foo")).expect("the check directory should exist");
    let engine = engine_for(&repo);
    let command_id =
        plan_approved_and_command_paused(&engine, &repo, SUBDIRECTORY_CHECK_PLAN, RUN_CARGO_TEST);

    let mut finish = scripted(vec![vec![call("complete_step", "{}")]]);
    let result = decide_command(&engine, &command_id, false, &mut finish);

    assert_eq!(
        result.task.status,
        TaskStatus::Incomplete,
        "the same command at the root does not waive a check in crates/foo: {result:?}"
    );
    let report = report_for(&engine, &result);
    assert!(report.waivers.is_empty(), "{report:?}");
    // Since spec 106 Task 6 `complete_step` refuses to close the step before
    // its required check is answered, and this decline does not answer it: the
    // step stays open rather than closing on a missing check.
    assert_eq!(
        report.reason_codes(),
        vec![CompletionReasonCode::OpenPlanSteps],
        "{report:?}"
    );
}

#[test]
fn a_later_approved_run_decides_instead_of_the_decline_when_it_passes() {
    let repo = temp_repo("decline-then-pass");
    let engine = engine_for(&repo);
    let first_command =
        plan_approved_and_command_paused(&engine, &repo, CHECK_ONLY_PLAN, RUN_CARGO_TEST);

    // Decline the first request, then ask again and approve the rerun.
    let mut rerun = scripted(vec![vec![call("run_command", RUN_CARGO_TEST)]]);
    let paused = decide_command(&engine, &first_command, false, &mut rerun);
    let second_command = paused
        .command_proposal
        .expect("the rerun still needs command approval");
    let mut finish = scripted(vec![vec![call("complete_step", "{}")]]);
    let result = approve_command_through_baselines(&engine, &second_command.id, &mut finish);

    assert_eq!(
        result.task.status,
        TaskStatus::Complete,
        "the later passing run is the current decision, not the old decline: {result:?}"
    );
    let report = report_for(&engine, &result);
    assert!(report.eligible, "{report:?}");
    assert!(
        report.waivers.is_empty(),
        "the check actually ran and passed, so it is not waived: {report:?}"
    );
}

#[test]
fn a_later_approved_run_decides_instead_of_the_decline_when_it_fails() {
    let repo = temp_repo("decline-then-fail");
    let engine = engine_with_shell(&repo, "/usr/bin/false");
    let first_command =
        plan_approved_and_command_paused(&engine, &repo, CHECK_ONLY_PLAN, RUN_CARGO_TEST);

    let mut rerun = scripted(vec![vec![call("run_command", RUN_CARGO_TEST)]]);
    let paused = decide_command(&engine, &first_command, false, &mut rerun);
    let second_command = paused
        .command_proposal
        .expect("the rerun still needs command approval");
    let mut finish = scripted(vec![vec![call("complete_step", "{}")]]);
    let result = approve_command_through_baselines(&engine, &second_command.id, &mut finish);

    assert_eq!(
        result.task.status,
        TaskStatus::Incomplete,
        "the later failing run decides against the task: {result:?}"
    );
    let report = report_for(&engine, &result);
    assert!(report.waivers.is_empty(), "{report:?}");
    assert!(
        report
            .reason_codes()
            .contains(&CompletionReasonCode::RequiredCheckFailed),
        "the failure is named, not hidden behind the old waiver: {report:?}"
    );
}

#[test]
fn a_declined_check_waiver_names_the_check_and_is_never_a_pass() {
    let repo = temp_repo("waiver-is-not-a-pass");
    let engine = engine_for(&repo);
    let command_id =
        plan_approved_and_command_paused(&engine, &repo, CHECK_ONLY_PLAN, RUN_CARGO_TEST);

    let mut finish = scripted(vec![vec![call("complete_step", "{}")]]);
    let result = decide_command(&engine, &command_id, false, &mut finish);

    let report = report_for(&engine, &result);
    assert_eq!(report.waivers.len(), 1, "{report:?}");
    assert!(
        report.waivers[0]
            .explanation
            .contains("cargo test (repository root)"),
        "the waiver shows the check's label: {report:?}"
    );
    assert!(
        report.waivers[0].explanation.contains("declined by you"),
        "the waiver says the user declined it: {report:?}"
    );
    assert!(
        report.reasons.is_empty(),
        "a waiver is a disposition, not a reason: {report:?}"
    );

    let events = engine
        .session_store
        .read_verification_events(&result.session.id, &[])
        .expect("the verification log should read");
    let state = fold_verification(events.iter());
    assert!(
        !state.checks.contains_key(&CheckId::new("", "cargo test")),
        "the declined command never becomes a passing CheckRun"
    );
}

// ---------------------------------------------------------------------------
// R16 floor: the model ran the floor check itself and the user declined it.
// ---------------------------------------------------------------------------

const FLOOR_PLAN: &str = r#"{"steps":[
    {"title":"Prepare the change"},
    {"title":"Change the member"}
]}"#;

fn workspace_fixture(name: &str) -> PathBuf {
    let repo = temp_repo(name);
    fs::write(
        repo.join("Cargo.toml"),
        "[workspace]\nmembers = [\"crates/member\"]\n",
    )
    .expect("the workspace manifest");
    fs::create_dir_all(repo.join("crates/member/src")).expect("the member root");
    fs::write(
        repo.join("crates/member/Cargo.toml"),
        "[package]\nname = \"member\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .expect("the member manifest");
    fs::write(
        repo.join("crates/member/src/lib.rs"),
        "pub fn value() -> u8 { 1 }\n",
    )
    .expect("the member source");
    repo
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
/// declining every offered baseline so no check command runs in the fixture.
fn apply_patch_declining_baselines(engine: &WorkspaceEngine, repo: &Path, patch_id: &str) {
    engine
        .edit_orchestrator
        .apply_stored_patch(repo, patch_id, None, None, "tester", false)
        .expect("the patch should apply");
}

#[test]
fn declining_a_model_run_floor_check_is_a_named_waiver_and_completes() {
    let repo = workspace_fixture("floor-declined-command");
    let engine = engine_for(&repo);
    let member_check = CheckId::new("crates/member", "cargo test");
    let root_check = CheckId::new("", "cargo test");

    let mut first = scripted(vec![
        vec![call("propose_plan", FLOOR_PLAN)],
        vec![patch_call_with(
            "crates/member/src/lib.rs",
            "pub fn value() -> u8 { 2 }\n",
        )],
    ]);
    let paused = ask(&engine, &repo, "Change the member", &mut first);
    let review = paused.plan_proposal.expect("the plan should be put up");
    // Close the preparation step first, so the patch lands in the mutation batch
    // the member check will record. Closing that step later does not advance
    // the generation, so the check stays current.
    let mut after_plan = scripted(vec![
        vec![call("complete_step", "{}")],
        vec![patch_call_with(
            "crates/member/src/lib.rs",
            "pub fn value() -> u8 { 2 }\n",
        )],
    ]);
    let proposed = decide_plan(&engine, &review.id, &mut after_plan);
    let patch_id = proposed
        .patch_proposal
        .as_ref()
        .expect("the patch is proposed")
        .patch_id
        .clone();
    apply_patch_declining_baselines(&engine, &repo, &patch_id);

    // The patch's own due verification is per-change: it selects the member
    // root only, so the enclosing workspace root stays a completion-floor
    // obligation with no CheckRun of its own.
    let mut due = MockModelAdapter::new("Done.");
    let _ = run_verification_and_approve_checks(&engine, &repo, &proposed.session.id, &mut due);
    let events = engine
        .session_store
        .read_verification_events(&proposed.session.id, &[])
        .expect("the verification log should read");
    let state = fold_verification(events.iter());
    assert!(
        state.checks.contains_key(&member_check),
        "the member check ran per change: {state:?}"
    );
    assert!(
        !state.checks.contains_key(&root_check),
        "the workspace root is a completion floor, not a per-change check: {state:?}"
    );

    // The model runs the exact floor check at the root, the user declines it,
    // and the step closes.
    let mut check_round = scripted(vec![vec![call("run_command", RUN_CARGO_TEST)]]);
    let paused = ask_in_session(
        &engine,
        &repo,
        &proposed.session.id,
        "Finish the task",
        &mut check_round,
    );
    let command = paused
        .command_proposal
        .expect("the floor command needs command approval");
    let mut finish = scripted(vec![vec![call("complete_step", "{}")]]);
    let result = decide_command(&engine, &command.id, false, &mut finish);

    assert_eq!(
        result.task.status,
        TaskStatus::Complete,
        "declining the model-run floor check waives its completion obligation: {result:?}"
    );
    let report = report_for(&engine, &result);
    assert!(report.eligible, "{report:?}");
    assert!(report.reasons.is_empty(), "{report:?}");
    assert_eq!(report.waivers.len(), 1, "{report:?}");
    assert_eq!(
        report.waivers[0].check, root_check,
        "the waiver names the declined floor check"
    );
}

/// The decline must be recorded as plan evidence, not only in a transient
/// buffer: a later read of the plan still carries it.
#[test]
fn the_decline_is_durable_plan_evidence() {
    let repo = temp_repo("decline-durable");
    let engine = engine_for(&repo);
    let command_id =
        plan_approved_and_command_paused(&engine, &repo, CHECK_ONLY_PLAN, RUN_CARGO_TEST);

    let mut finish = scripted(vec![vec![call("complete_step", "{}")]]);
    let result = decide_command(&engine, &command_id, false, &mut finish);
    let plan = engine
        .session_store
        .read_task_plan(&result.session.id, &result.task.id)
        .expect("the plan should read")
        .expect("the task has a plan");
    let declined = plan
        .steps
        .iter()
        .flat_map(|step| &step.evidence)
        .any(|evidence| {
            serde_json::to_value(evidence)
                .ok()
                .and_then(|value| {
                    value
                        .get("kind")
                        .and_then(|kind| kind.as_str())
                        .map(String::from)
                })
                .as_deref()
                == Some("commandDeclined")
        });
    assert!(
        declined,
        "the user's decline is durable plan evidence: {:?}",
        plan.steps
    );
}
