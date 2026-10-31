//! A typed turn picks up the session's open plan, per
//! `docs/specs/83_typed_turns_pick_up_the_open_plan.md`.
//!
//! A task is one turn, so a plan lives on the task that proposed it. Before
//! this spec only a turn that a stop handed over (token or round ceiling, a
//! crash resume) carried the plan; a plain "continue" typed after a turn that
//! left steps open started with none, and the verification and spec 77's
//! continuation that came after it saw no open steps of the task's own.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use workspace_engine::plan::{PlanStep, StepStatus, TaskPlan};
use workspace_engine::{
    CancelToken, ChatTurnResult, Config, MockModelAdapter, ModelAdapter, SessionMode, ToolCall,
    TurnProgress, TurnSink, WorkspaceEngine,
};

static COUNTER: AtomicU64 = AtomicU64::new(1);

fn temp_repo(name: &str) -> PathBuf {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock should work")
        .as_nanos();
    let repo = std::env::temp_dir().join(format!(
        "damaian-plan-carry-{name}-{now}-{}",
        COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir_all(repo.join("src")).expect("repository should be created");
    fs::write(repo.join("src/a.rs"), "fn main() {}\n").expect("a file to read");
    repo
}

fn engine_for(repo: &Path) -> WorkspaceEngine {
    WorkspaceEngine::new(Config {
        data_dir: repo.join(".damaian"),
        enable_index_watcher: false,
        ..Config::default()
    })
}

fn sink_parts() -> (CancelToken, impl FnMut(&str), impl FnMut(TurnProgress)) {
    (
        CancelToken::new(),
        |_token: &str| {},
        |_event: TurnProgress| {},
    )
}

/// One typed turn, in `session` when given, in a new session otherwise.
fn ask(
    engine: &WorkspaceEngine,
    repo: &Path,
    session: Option<&str>,
    prompt: &str,
    adapter: &mut dyn ModelAdapter,
) -> ChatTurnResult {
    let (cancel, mut on_token, mut on_progress) = sink_parts();
    let mut sink = TurnSink {
        on_token: &mut on_token,
        on_progress: &mut on_progress,
        cancel: &cancel,
        steering: None,
    };
    engine
        .chat_orchestrator
        .ask_with_session(repo, prompt, &[], session, adapter, &mut sink)
        .expect("the turn should run")
}

fn approve_plan(
    engine: &WorkspaceEngine,
    proposal_id: &str,
    adapter: &mut dyn ModelAdapter,
) -> ChatTurnResult {
    let (cancel, mut on_token, mut on_progress) = sink_parts();
    let mut sink = TurnSink {
        on_token: &mut on_token,
        on_progress: &mut on_progress,
        cancel: &cancel,
        steering: None,
    };
    engine
        .chat_orchestrator
        .resume_after_plan_decision(proposal_id, true, None, "tester", adapter, &mut sink)
        .expect("the approved turn should run")
}

fn call(name: &str, arguments_json: &str) -> ToolCall {
    ToolCall {
        id: format!("call_{name}"),
        name: name.to_string(),
        arguments_json: arguments_json.to_string(),
    }
}

fn plan_call(titles: &[&str]) -> ToolCall {
    let steps = titles
        .iter()
        .map(|title| format!(r#"{{"title":"{title}"}}"#))
        .collect::<Vec<_>>()
        .join(",");
    call("propose_plan", &format!(r#"{{"steps":[{steps}]}}"#))
}

fn patch_call(path: &str) -> ToolCall {
    call(
        "propose_patch",
        &format!(
            r#"{{"summary":"Add {path}","files":[{{"path":"{path}","content":"pub fn added() {{}}\n"}}]}}"#
        ),
    )
}

/// Tool-call rounds followed by a plain answer.
fn scripted(rounds: Vec<Vec<ToolCall>>) -> MockModelAdapter {
    let mut responses: Vec<String> = rounds.iter().map(|_| String::new()).collect();
    let mut calls = rounds;
    responses.push("Done.".to_string());
    calls.push(Vec::new());
    MockModelAdapter::new_sequence_with_tool_calls(responses, calls)
}

/// A turn that proposes a plan and then answers twice without working on it,
/// so spec 66's reminder is spent and the turn ends with every step open —
/// what the session this spec came from looked like after its first turn.
fn leave_a_plan_open(
    engine: &WorkspaceEngine,
    repo: &Path,
    session: Option<&str>,
    titles: &[&str],
) -> ChatTurnResult {
    let mut adapter = MockModelAdapter::new_sequence_with_tool_calls(
        vec![
            String::new(),
            "Here is the plan.".to_string(),
            "Say continue when ready.".to_string(),
        ],
        vec![vec![plan_call(titles)], Vec::new(), Vec::new()],
    );
    ask(
        engine,
        repo,
        session,
        "Rework the retry handling",
        &mut adapter,
    )
}

/// Two plain answers: a turn that does nothing with whatever plan it carries.
fn answer_twice(answer: &str) -> MockModelAdapter {
    MockModelAdapter::new_sequence(vec![answer.to_string(), answer.to_string()])
}

fn plan_of(engine: &WorkspaceEngine, result: &ChatTurnResult) -> Option<TaskPlan> {
    engine
        .session_store
        .read_task_plan(&result.session.id, &result.task.id)
        .expect("the plan should read")
}

fn titles(plan: &TaskPlan) -> Vec<&str> {
    plan.steps.iter().map(|step| step.title.as_str()).collect()
}

fn first_user_message(adapter: &MockModelAdapter) -> String {
    adapter
        .requests
        .first()
        .expect("the model was called")
        .messages
        .iter()
        .rev()
        .find(|message| message.role == "user")
        .map(|message| message.content.clone())
        .unwrap_or_default()
}

#[test]
fn a_typed_turn_picks_up_the_open_plan_and_works_on_it() {
    let repo = temp_repo("picks-up");
    let engine = engine_for(&repo);
    let first = leave_a_plan_open(&engine, &repo, None, &["Read", "Change", "Document"]);
    assert_eq!(plan_of(&engine, &first).unwrap().open_steps().len(), 3);

    let mut adapter = scripted(vec![vec![call("complete_step", "{}")]]);
    let second = ask(
        &engine,
        &repo,
        Some(&first.session.id),
        "continue",
        &mut adapter,
    );

    let plan = plan_of(&engine, &second).expect("the typed turn carries the open plan");
    assert_eq!(titles(&plan), vec!["Read", "Change", "Document"]);
    assert_eq!(plan.steps[0].status, StepStatus::Completed);
    assert_eq!(plan.steps[1].status, StepStatus::InProgress);
    // The model is told the plan is an earlier turn's, and what it may do.
    let prompt = first_user_message(&adapter);
    assert!(
        prompt.contains("The plan this task is carrying forward"),
        "{prompt}"
    );
    let tools = adapter.requests[0]
        .tools
        .as_ref()
        .expect("the turn offers tools");
    assert!(tools.iter().any(|tool| {
        tool.name == "propose_plan"
            && tool
                .description
                .contains("carried forward from an earlier turn")
    }));
    // The original stays readable under the turn that made it.
    assert_eq!(
        plan_of(&engine, &first).unwrap().steps[0].status,
        StepStatus::InProgress
    );
}

#[test]
fn the_latest_plan_is_found_past_a_turn_that_carried_none() {
    // A turn in Ask mode does not pick the plan up; the next turn in Code mode
    // still finds it, because the rule looks for the latest plan, not the
    // previous task's.
    let repo = temp_repo("past-ask");
    let engine = engine_for(&repo);
    let first = leave_a_plan_open(&engine, &repo, None, &["Read", "Change"]);
    let session = first.session.id.clone();

    engine
        .session_store
        .set_session_mode(&session, SessionMode::Ask, "tester")
        .unwrap();
    let asked = ask(
        &engine,
        &repo,
        Some(&session),
        "What does src/a.rs do?",
        &mut MockModelAdapter::new("It defines main."),
    );
    assert!(
        plan_of(&engine, &asked).is_none(),
        "an Ask turn does not pick the plan up"
    );

    engine
        .session_store
        .set_session_mode(&session, SessionMode::Code, "tester")
        .unwrap();
    let mut adapter = scripted(vec![vec![call("complete_step", "{}")]]);
    let resumed = ask(&engine, &repo, Some(&session), "continue", &mut adapter);
    let plan = plan_of(&engine, &resumed).expect("the Code turn picks it up again");
    assert_eq!(titles(&plan), vec!["Read", "Change"]);
}

#[test]
fn a_finished_plan_is_not_picked_up() {
    let repo = temp_repo("finished");
    let engine = engine_for(&repo);
    let mut adapter = scripted(vec![
        vec![plan_call(&["Read", "Report"])],
        vec![call("complete_step", "{}")],
        vec![call("complete_step", "{}")],
    ]);
    let first = ask(&engine, &repo, None, "Look and report", &mut adapter);
    assert!(plan_of(&engine, &first).unwrap().open_steps().is_empty());

    let second = ask(
        &engine,
        &repo,
        Some(&first.session.id),
        "Something else",
        &mut MockModelAdapter::new("Sure."),
    );
    assert!(plan_of(&engine, &second).is_none());
}

#[test]
fn a_plan_whose_only_open_steps_are_verification_steps_is_not_picked_up() {
    // `verify_` steps belong to the verification loop, which carries its own
    // plan; a typed turn has nothing to do on them.
    let repo = temp_repo("verify-only");
    let engine = engine_for(&repo);
    let first = ask(
        &engine,
        &repo,
        None,
        "Hello",
        &mut MockModelAdapter::new("Hi."),
    );
    let mut plan = TaskPlan::new(&first.task.id, 0);
    for (id, status) in [
        ("step_1", StepStatus::Completed),
        ("verify_cargo_test", StepStatus::Pending),
    ] {
        plan.steps.push(PlanStep {
            id: id.to_string(),
            title: id.to_string(),
            detail: None,
            status,
            depends_on: Vec::new(),
            started_at_ms: None,
            completed_at_ms: None,
            evidence: Vec::new(),
        });
    }
    engine
        .session_store
        .create_plan(&first.task, &plan)
        .unwrap();

    let second = ask(
        &engine,
        &repo,
        Some(&first.session.id),
        "continue",
        &mut MockModelAdapter::new("Sure."),
    );
    assert!(plan_of(&engine, &second).is_none());
}

#[test]
fn a_plan_a_turn_carried_and_left_untouched_is_not_carried_again() {
    // The stop rule: an unrelated question after an open plan costs one turn
    // with the plan on it, not the rest of the session.
    let repo = temp_repo("untouched");
    let engine = engine_for(&repo);
    let first = leave_a_plan_open(&engine, &repo, None, &["Read", "Change"]);
    let session = first.session.id.clone();

    let unrelated = ask(
        &engine,
        &repo,
        Some(&session),
        "Unrelated: what is the time zone?",
        &mut answer_twice("UTC."),
    );
    let carried = plan_of(&engine, &unrelated).expect("one turn still carries it");
    assert_eq!(carried.open_steps().len(), 2);

    let next = ask(
        &engine,
        &repo,
        Some(&session),
        "And the date?",
        &mut answer_twice("Today."),
    );
    assert!(
        plan_of(&engine, &next).is_none(),
        "a plan nobody worked on in the turn that carried it stops there"
    );
}

#[test]
fn a_carried_plan_can_be_replaced_before_it_moves() {
    let repo = temp_repo("replaced");
    let engine = engine_for(&repo);
    let first = leave_a_plan_open(&engine, &repo, None, &["Read", "Change"]);

    let mut adapter = scripted(vec![vec![plan_call(&["Inspect logs", "Fix the crash"])]]);
    let second = ask(
        &engine,
        &repo,
        Some(&first.session.id),
        "Forget that; the app crashes on start",
        &mut adapter,
    );

    let plan = plan_of(&engine, &second).expect("the new plan");
    assert_eq!(titles(&plan), vec!["Inspect logs", "Fix the crash"]);
    assert_eq!(plan.steps[0].status, StepStatus::InProgress);
    assert_eq!(
        titles(&plan_of(&engine, &first).unwrap()),
        vec!["Read", "Change"],
        "the old plan stays under the turn that made it"
    );
}

#[test]
fn a_carried_plan_that_moved_cannot_be_replaced() {
    // Once a step of the carried plan closed this turn, its evidence is tied to
    // that plan, and the one-plan-per-turn refusal holds as before.
    let repo = temp_repo("moved");
    let engine = engine_for(&repo);
    let first = leave_a_plan_open(&engine, &repo, None, &["Read", "Change"]);

    let mut adapter = scripted(vec![
        vec![call("complete_step", "{}")],
        vec![plan_call(&["Something else"])],
    ]);
    let second = ask(
        &engine,
        &repo,
        Some(&first.session.id),
        "continue",
        &mut adapter,
    );

    let plan = plan_of(&engine, &second).unwrap();
    assert_eq!(titles(&plan), vec!["Read", "Change"]);
}

#[test]
fn an_approved_plan_stays_approved_when_a_typed_turn_picks_it_up() {
    let repo = temp_repo("approved");
    let engine = engine_for(&repo);
    let mut first = scripted(vec![
        vec![plan_call(&["Add x", "Add y"])],
        vec![patch_call("src/x.rs")],
    ]);
    let paused = ask(&engine, &repo, None, "Add x and y", &mut first);
    let review = paused.plan_proposal.expect("the plan is put up for review");
    let approved = approve_plan(
        &engine,
        &review.id,
        &mut scripted(vec![vec![patch_call("src/x.rs")]]),
    );
    assert!(approved.patch_proposal.is_some());

    let second = ask(
        &engine,
        &repo,
        Some(&approved.session.id),
        "continue",
        &mut scripted(vec![vec![patch_call("src/y.rs")]]),
    );
    assert!(
        second.plan_proposal.is_none(),
        "the user approved this plan once; picking it up does not ask again"
    );
    assert!(second.patch_proposal.is_some());
}

#[test]
fn a_replaced_plan_does_not_inherit_the_approval() {
    let repo = temp_repo("replaced-approval");
    let engine = engine_for(&repo);
    let mut first = scripted(vec![
        vec![plan_call(&["Add x", "Add y"])],
        vec![patch_call("src/x.rs")],
    ]);
    let paused = ask(&engine, &repo, None, "Add x and y", &mut first);
    let review = paused.plan_proposal.expect("the plan is put up for review");
    let approved = approve_plan(
        &engine,
        &review.id,
        &mut scripted(vec![vec![patch_call("src/x.rs")]]),
    );

    let second = ask(
        &engine,
        &repo,
        Some(&approved.session.id),
        "Do something different",
        &mut scripted(vec![
            vec![plan_call(&["Add z", "Test z"])],
            vec![patch_call("src/z.rs")],
        ]),
    );
    assert!(
        second.plan_proposal.is_some(),
        "a new plan is reviewed like any other"
    );
}

#[test]
fn verification_after_a_typed_turn_continues_the_open_plan() {
    // The session this spec came from: a plan left open, a typed "continue"
    // that proposed a patch, the patch applied and verified. Spec 77's
    // continuation must see the plan's own open steps.
    let repo = temp_repo("verify-continues");
    fs::write(repo.join("Cargo.toml"), "[workspace]\nmembers = []\n").unwrap();
    let engine = WorkspaceEngine::new(Config {
        data_dir: repo.join(".damaian"),
        enable_index_watcher: false,
        shell: "/usr/bin/true".to_string(),
        command_allowlist: vec!["cargo test".to_string()],
        ..Config::default()
    });
    let first = leave_a_plan_open(&engine, &repo, None, &["Add the helper", "Document it"]);

    let typed = ask(
        &engine,
        &repo,
        Some(&first.session.id),
        "continue",
        &mut scripted(vec![vec![patch_call("src/helper.rs")]]),
    );
    // The carried plan was never approved (the first turn only read), so its
    // first mutation is reviewed.
    let review = typed.plan_proposal.expect("the carried plan is reviewed");
    let proposed = approve_plan(
        &engine,
        &review.id,
        &mut scripted(vec![vec![patch_call("src/helper.rs")]]),
    );
    let patch = proposed.patch_proposal.expect("the patch is proposed");
    engine
        .edit_orchestrator
        .apply_stored_patch(&repo, &patch.patch_id, None, None, "tester", false)
        .expect("the patch should apply");

    let (cancel, mut on_token, mut on_progress) = sink_parts();
    let mut sink = TurnSink {
        on_token: &mut on_token,
        on_progress: &mut on_progress,
        cancel: &cancel,
        steering: None,
    };
    let mut adapter = MockModelAdapter::new("Documenting it next.");
    engine
        .chat_orchestrator
        .verify_due(&repo, &first.session.id, &mut adapter, &mut sink)
        .expect("the verification should run");

    let prompt = first_user_message(&adapter);
    assert!(prompt.contains("The task is not finished"), "{prompt}");
    assert!(prompt.contains("- Add the helper (step_1)"), "{prompt}");
    assert!(prompt.contains("- Document it (step_2)"), "{prompt}");
}
