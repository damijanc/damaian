//! Worker plan-review budgets, per `docs/specs/103_bounded_worker_continuation.md`
//! Task 2.
//!
//! The boundaries here are about identity and durability: only a session the
//! engine records as a worker is offered a budget, the exact proposed values
//! come back unchanged through the pending turn and recovery, a deliberate
//! approval writes the revision and budget in one event, and every malformed or
//! altered decision is refused before anything durable changes.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use workspace_engine::plan::{StepStatus, TaskPlan};
use workspace_engine::recovery::ReattachedApproval;
use workspace_engine::{
    AuditLog, CancelToken, ChatTurnResult, CommandStore, Config, MockModelAdapter, PatchStore,
    PausedTurns, PlanRevisionCheck, PlanRevisionStep, Session, TaskExecutionBudget, TaskStatus,
    ToolCall, TurnProgress, TurnSink, WorkerRecord, WorkspaceEngine, repository_id_for_root,
};

static COUNTER: AtomicU64 = AtomicU64::new(1);

fn temp_repo(name: &str) -> PathBuf {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock should work")
        .as_nanos();
    let repo = std::env::temp_dir().join(format!(
        "damaian-plan-budget-{name}-{now}-{}",
        COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir_all(repo.join("src")).expect("repository should be created");
    fs::write(repo.join("src/a.rs"), "fn main() {}\n").expect("a file to read");
    repo
}

fn engine_for(repo: &Path, adjust: impl FnOnce(&mut Config)) -> WorkspaceEngine {
    let mut config = Config {
        data_dir: repo.join(".damaian"),
        enable_index_watcher: false,
        ..Config::default()
    };
    adjust(&mut config);
    WorkspaceEngine::new(config)
}

fn worker_session(engine: &WorkspaceEngine, repo: &Path) -> Session {
    let session = engine
        .session_store
        .create_session(&repository_id_for_root(repo), "Worker")
        .expect("a worker session should be created");
    engine
        .session_store
        .record_worker_spawned(
            &session.id,
            &WorkerRecord {
                parent_session_id: None,
                parent_task_id: None,
                inherited_protected_paths: Vec::new(),
                source_repository: repo.to_path_buf(),
                worktree_path: repo.to_path_buf(),
                branch: "main".to_string(),
                base_commit: "0000000".to_string(),
                included_uncommitted: false,
                git_commands: Vec::new(),
            },
        )
        .expect("the worker record should be written");
    session
}

fn call(name: &str, arguments_json: &str) -> ToolCall {
    ToolCall {
        id: format!("call_{name}"),
        name: name.to_string(),
        arguments_json: arguments_json.to_string(),
    }
}

fn scripted() -> MockModelAdapter {
    MockModelAdapter::new_sequence_with_tool_calls(
        vec![String::new(), String::new(), "Done.".to_string()],
        vec![
            vec![call(
                "propose_plan",
                r#"{"steps":[{"title":"Read the retry helper"},{"title":"Add a bounded backoff"},{"title":"Rewrite the scheduler"}]}"#,
            )],
            vec![call(
                "propose_patch",
                r#"{"summary":"Add src/retry.rs","files":[{"path":"src/retry.rs","content":"pub fn added() {}\n"}]}"#,
            )],
            Vec::new(),
        ],
    )
}

fn ask(engine: &WorkspaceEngine, repo: &Path, session_id: Option<&str>) -> ChatTurnResult {
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
        .ask_with_session(
            repo,
            "Add retry handling",
            &[],
            session_id,
            &mut scripted(),
            &mut sink,
        )
        .expect("the turn should run")
}

fn ask_in_new_session(engine: &WorkspaceEngine, repo: &Path) -> ChatTurnResult {
    ask(engine, repo, None)
}

fn ask_in_worker(engine: &WorkspaceEngine, repo: &Path) -> (ChatTurnResult, Session) {
    let session = worker_session(engine, repo);
    (ask(engine, repo, Some(&session.id)), session)
}

#[allow(clippy::too_many_arguments)]
fn decide(
    engine: &WorkspaceEngine,
    proposal_id: &str,
    approved: bool,
    revised: Option<Vec<PlanRevisionStep>>,
    budget: Option<&TaskExecutionBudget>,
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
    let mut adapter = MockModelAdapter::new("Done.");
    engine
        .chat_orchestrator
        .resume_after_plan_decision_with_budget(
            proposal_id,
            approved,
            revised,
            budget,
            "tester",
            &mut adapter,
            &mut sink,
        )
}

fn log_events(engine: &WorkspaceEngine, session_id: &str) -> Vec<serde_json::Value> {
    let path = engine
        .config
        .data_dir
        .join("sessions")
        .join(format!("{session_id}.jsonl"));
    fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .filter_map(|line| serde_json::from_str(line).ok())
        .collect()
}

fn event_kinds(events: &[serde_json::Value]) -> Vec<&str> {
    events
        .iter()
        .filter_map(|event| event.get("eventType").and_then(|value| value.as_str()))
        .collect()
}

fn pending_path(engine: &WorkspaceEngine, proposal_id: &str) -> PathBuf {
    engine
        .config
        .data_dir
        .join("chat")
        .join("pending")
        .join(format!("{proposal_id}.json"))
}

fn plan_of(engine: &WorkspaceEngine, result: &ChatTurnResult) -> TaskPlan {
    engine
        .session_store
        .read_task_plan(&result.session.id, &result.task.id)
        .expect("the plan should read")
        .expect("the task should have a plan")
}

#[test]
fn only_a_durable_worker_session_is_offered_a_budget() {
    let repo = temp_repo("worker-only");
    let engine = engine_for(&repo, |_| {});

    let inline = ask_in_new_session(&engine, &repo);
    let inline_review = inline.plan_proposal.expect("the inline plan is reviewed");
    assert!(
        inline_review.budget_review.is_none(),
        "an ordinary session must keep the budgetless review"
    );

    let (worker, _session) = ask_in_worker(&engine, &repo);
    let worker_review = worker.plan_proposal.expect("the worker plan is reviewed");
    let review = worker_review
        .budget_review
        .expect("a genuine worker is offered a budget");
    let policy = engine.config.task_budget_policy();
    assert_eq!(review.budget, policy.default);
    assert_eq!(review.rows.len(), 4);
    assert!(review.rows.iter().any(|row| row.label == "Action rounds"));
    assert!(review.rows.iter().all(|row| !row.unit.is_empty()));
}

#[test]
fn the_pending_turn_persists_the_same_review_and_values() {
    let repo = temp_repo("pending-exact");
    let engine = engine_for(&repo, |_| {});
    let (worker, _session) = ask_in_worker(&engine, &repo);
    let proposal = worker.plan_proposal.expect("the worker plan is reviewed");
    let review = proposal
        .budget_review
        .clone()
        .expect("the review offers a budget");

    let paused = PausedTurns::new(&engine.config.data_dir);
    assert_eq!(
        paused.plan_budget_review(&proposal.id).unwrap(),
        Some(review.clone())
    );

    let pending: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(pending_path(&engine, &proposal.id)).unwrap())
            .expect("the pending turn is JSON");
    assert_eq!(
        pending["plan_review"]["budget"],
        serde_json::to_value(&review).expect("the review serializes")
    );
    // A fresh data-dir handle (the reload / worker-restart case) reconstructs
    // the same strings, not merely equal numbers.
    let reread = PausedTurns::new(engine.config.data_dir.clone())
        .plan_budget_review(&proposal.id)
        .expect("the review reloads")
        .expect("the review carries its budget");
    assert_eq!(
        serde_json::to_string(&reread).unwrap(),
        serde_json::to_string(&review).unwrap()
    );
}

#[test]
fn a_fresh_engine_resumes_the_stored_review_with_its_exact_budget() {
    let repo = temp_repo("fresh-engine");
    let first = engine_for(&repo, |_| {});
    let (worker, _session) = ask_in_worker(&first, &repo);
    let proposal = worker.plan_proposal.expect("the worker plan is reviewed");
    let review = proposal
        .budget_review
        .clone()
        .expect("the review offers a budget");
    drop(first);

    // A restarted worker process builds a new engine over the same data dir.
    // The pending review and its values come from disk, not from memory.
    let restarted = engine_for(&repo, |_| {});
    let result = decide(&restarted, &proposal.id, true, None, Some(&review.budget))
        .expect("the restarted engine should resume the review");
    assert_eq!(
        restarted
            .session_store
            .read_task_budget(&result.session.id, &result.task.id)
            .unwrap(),
        Some(review.budget)
    );
}

#[test]
fn a_revised_plan_and_budget_are_one_event_and_replay_fresh() {
    let repo = temp_repo("one-event");
    let engine = engine_for(&repo, |_| {});
    let (worker, _session) = ask_in_worker(&engine, &repo);
    let proposal = worker.plan_proposal.expect("the worker plan is reviewed");
    let review = proposal
        .budget_review
        .clone()
        .expect("the review offers a budget");
    let ids: Vec<String> = proposal
        .plan
        .steps
        .iter()
        .map(|step| step.id.clone())
        .collect();

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
    let result = decide(
        &engine,
        &proposal.id,
        true,
        Some(revision),
        Some(&review.budget),
    )
    .expect("the reviewed plan should be approved");

    let events = log_events(&engine, &result.session.id);
    let kinds = event_kinds(&events);
    assert!(
        !kinds.contains(&"plan_revised"),
        "a budgeted revision is not a second event: {kinds:?}"
    );
    let approvals: Vec<_> = events
        .iter()
        .filter(|event| event["eventType"] == "plan_approved")
        .collect();
    assert_eq!(approvals.len(), 1, "one decision, one line: {events:?}");
    assert_eq!(
        approvals[0]["payload"]["budget"],
        serde_json::to_value(review.budget).unwrap()
    );
    assert!(
        approvals[0]["payload"]["plan"].is_object(),
        "the reviewed plan travels in the same payload: {events:?}"
    );

    // Fresh store handle: replay, not an in-memory carry.
    let store = workspace_engine::SessionStore::new(engine.config.data_dir.clone());
    let plan = store
        .read_task_plan(&result.session.id, &result.task.id)
        .unwrap()
        .expect("the approved plan replays");
    assert_eq!(plan.steps.len(), 2);
    assert_eq!(plan.steps[1].title, "Add a backoff capped at 30s");
    assert_eq!(
        store
            .read_session_plans(&result.session.id)
            .unwrap()
            .get(&result.task.id)
            .expect("the session view has the plan")
            .steps
            .len(),
        2,
        "the session-wide reader agrees with the task reader"
    );
    assert!(
        store
            .read_plan_approved(&result.session.id, &result.task.id)
            .unwrap()
    );
    assert_eq!(
        store
            .read_task_budget(&result.session.id, &result.task.id)
            .unwrap(),
        Some(review.budget)
    );
    assert!(
        kinds.contains(&"plan_created"),
        "the original proposal remains readable: {kinds:?}"
    );
}

#[test]
fn an_invalid_revision_is_refused_before_the_pending_review_is_taken() {
    let repo = temp_repo("invalid-revision");
    let engine = engine_for(&repo, |_| {});
    let (worker, _session) = ask_in_worker(&engine, &repo);
    let proposal = worker.plan_proposal.expect("the worker plan is reviewed");
    let review = proposal.budget_review.clone().unwrap();
    let ids: Vec<String> = proposal
        .plan
        .steps
        .iter()
        .map(|step| step.id.clone())
        .collect();

    // The same step twice: the applied plan would carry two steps with one id,
    // so an update could never address the second of them.
    let duplicate = vec![
        PlanRevisionStep {
            id: ids[0].clone(),
            title: "First copy".to_string(),
            check: PlanRevisionCheck::Preserve,
        },
        PlanRevisionStep {
            id: ids[0].clone(),
            title: "Second copy".to_string(),
            check: PlanRevisionCheck::Preserve,
        },
    ];
    let before = log_events(&engine, &worker.session.id);
    let error = decide(
        &engine,
        &proposal.id,
        true,
        Some(duplicate),
        Some(&review.budget),
    )
    .expect_err("a structurally invalid revision must be refused");
    assert!(error.to_string().contains("more than one step"), "{error}");
    assert_eq!(
        log_events(&engine, &worker.session.id),
        before,
        "a refused revision writes no plan_revised or plan_approved event"
    );
    assert!(
        pending_path(&engine, &proposal.id).is_file(),
        "the review must remain answerable after a refused revision"
    );

    // The exact reviewed budget still approves the plan once the revision is
    // valid, which proves the refusal consumed nothing.
    let valid = vec![PlanRevisionStep {
        id: ids[0].clone(),
        title: "Read the retry helper".to_string(),
        check: PlanRevisionCheck::Preserve,
    }];
    let result = decide(
        &engine,
        &proposal.id,
        true,
        Some(valid),
        Some(&review.budget),
    )
    .expect("a valid revision after a refusal should be accepted");
    let events = log_events(&engine, &result.session.id);
    let kinds = event_kinds(&events);
    assert!(!kinds.contains(&"plan_revised"), "{kinds:?}");
    assert!(kinds.contains(&"plan_approved"), "{kinds:?}");
}

#[test]
fn a_missing_or_altered_budget_is_refused_before_any_write() {
    let repo = temp_repo("refusals");
    let engine = engine_for(&repo, |_| {});
    let (worker, _session) = ask_in_worker(&engine, &repo);
    let proposal = worker.plan_proposal.expect("the worker plan is reviewed");
    let review = proposal
        .budget_review
        .clone()
        .expect("the review offers a budget");
    let before = log_events(&engine, &worker.session.id);

    // Missing: the budgetless entry point cannot approve a budgeted review.
    assert!(
        decide(&engine, &proposal.id, true, None, None).is_err(),
        "a budgeted approval without its budget must be refused"
    );
    let raised = TaskExecutionBudget {
        action_rounds: review.budget.action_rounds + 1,
        ..review.budget
    };
    assert!(
        decide(&engine, &proposal.id, true, None, Some(&raised)).is_err(),
        "a raised value must be refused"
    );
    let lowered = TaskExecutionBudget {
        output_tokens: review.budget.output_tokens - 1,
        ..review.budget
    };
    assert!(
        decide(&engine, &proposal.id, true, None, Some(&lowered)).is_err(),
        "an altered value must be refused"
    );
    let zero = TaskExecutionBudget {
        action_rounds: 0,
        ..review.budget
    };
    assert!(
        decide(&engine, &proposal.id, true, None, Some(&zero)).is_err(),
        "zero must be refused"
    );

    assert_eq!(
        log_events(&engine, &worker.session.id),
        before,
        "every refusal must leave the log unchanged"
    );
    assert!(
        pending_path(&engine, &proposal.id).is_file(),
        "a refusal must leave the review answerable"
    );

    // The exact reviewed value still works after every refusal.
    decide(&engine, &proposal.id, true, None, Some(&review.budget))
        .expect("the exact reviewed budget should be accepted");
}

#[test]
fn a_historical_or_inline_review_rejects_an_injected_budget() {
    let repo = temp_repo("injected");
    let engine = engine_for(&repo, |_| {});
    let inline = ask_in_new_session(&engine, &repo);
    let proposal = inline.plan_proposal.expect("the inline plan is reviewed");
    assert!(proposal.budget_review.is_none());
    let budget = engine.config.task_budget_policy().default;

    let error = decide(&engine, &proposal.id, true, None, Some(&budget))
        .expect_err("a budgetless review must refuse an injected budget");
    assert!(
        error.to_string().contains("offers no task budget"),
        "{error}"
    );
    let events = log_events(&engine, &inline.session.id);
    assert!(!event_kinds(&events).contains(&"plan_approved"));

    // The old, budgetless behaviour still works.
    let result = decide(&engine, &proposal.id, true, None, None)
        .expect("a budgetless review keeps its approval behaviour");
    assert!(
        engine
            .session_store
            .read_plan_approved(&result.session.id, &result.task.id)
            .unwrap()
    );
    assert_eq!(
        engine
            .session_store
            .read_task_budget(&result.session.id, &result.task.id)
            .unwrap(),
        None
    );
}

#[test]
fn a_budget_above_the_current_ceiling_is_refused() {
    let repo = temp_repo("ceiling");
    let engine = engine_for(&repo, |_| {});
    let (worker, _session) = ask_in_worker(&engine, &repo);
    let proposal = worker.plan_proposal.expect("the worker plan is reviewed");
    let review = proposal
        .budget_review
        .clone()
        .expect("the review offers a budget");

    // Configuration changed while the review waited. The reviewed numbers are
    // now above the effective ceiling, so even the exact proposal is refused.
    let narrowed = engine_for(&repo, |config| {
        config.worker_task_budget_max_action_rounds = review.budget.action_rounds - 1;
    });
    let error = decide(&narrowed, &proposal.id, true, None, Some(&review.budget))
        .expect_err("a budget above the effective ceiling must be refused");
    assert!(error.to_string().contains("effective ceiling"), "{error}");
    assert!(!event_kinds(&log_events(&engine, &worker.session.id)).contains(&"plan_approved"));
}

#[test]
fn declining_records_neither_an_approval_nor_a_budget() {
    let repo = temp_repo("decline");
    let engine = engine_for(&repo, |_| {});
    let (worker, _session) = ask_in_worker(&engine, &repo);
    let proposal = worker.plan_proposal.expect("the worker plan is reviewed");
    let review = proposal
        .budget_review
        .clone()
        .expect("the review offers a budget");

    // A reject need not send the budget at all.
    let result = decide(&engine, &proposal.id, false, None, None)
        .expect("a decline should be accepted without a budget");
    assert!(!event_kinds(&log_events(&engine, &result.session.id)).contains(&"plan_approved"));
    assert!(
        !engine
            .session_store
            .read_plan_approved(&result.session.id, &result.task.id)
            .unwrap()
    );
    assert_eq!(
        engine
            .session_store
            .read_task_budget(&result.session.id, &result.task.id)
            .unwrap(),
        None
    );
    assert_eq!(
        plan_of(&engine, &result)
            .steps
            .iter()
            .filter(|step| step.status == StepStatus::Skipped)
            .count(),
        3
    );
    assert!(review.budget.action_rounds > 0);
}

#[test]
fn a_decline_survives_a_ceiling_lowered_while_the_review_waited() {
    let repo = temp_repo("decline-ceiling");
    let engine = engine_for(&repo, |_| {});
    let (worker, _session) = ask_in_worker(&engine, &repo);
    let proposal = worker.plan_proposal.expect("the worker plan is reviewed");
    let review = proposal
        .budget_review
        .clone()
        .expect("the review offers a budget");

    // Configuration changed while the review waited, so the exact reviewed
    // numbers no longer fit the effective ceiling and an approval now fails.
    let narrowed = engine_for(&repo, |config| {
        config.worker_task_budget_max_action_rounds = review.budget.action_rounds - 1;
    });
    let error = decide(&narrowed, &proposal.id, true, None, Some(&review.budget))
        .expect_err("a budget above the effective ceiling is still refused on approval");
    assert!(error.to_string().contains("effective ceiling"), "{error}");

    // A decline spends nothing, so the same reviewed budget the desktop sends
    // back must still be accepted: the user must always be able to refuse.
    let altered = TaskExecutionBudget {
        action_rounds: review.budget.action_rounds - 1,
        ..review.budget
    };
    let error = decide(&narrowed, &proposal.id, false, None, Some(&altered))
        .expect_err("a decline may not substitute a different budget");
    assert!(error.to_string().contains("does not match"), "{error}");
    assert!(
        pending_path(&narrowed, &proposal.id).is_file(),
        "a refused decline must leave the review answerable"
    );

    let result = decide(&narrowed, &proposal.id, false, None, Some(&review.budget))
        .expect("a decline with the exact reviewed budget should be accepted");
    let events = log_events(&engine, &result.session.id);
    let kinds = event_kinds(&events);
    assert!(!kinds.contains(&"plan_approved"), "{kinds:?}");
    assert!(
        !narrowed
            .session_store
            .read_plan_approved(&result.session.id, &result.task.id)
            .unwrap()
    );
    assert_eq!(
        narrowed
            .session_store
            .read_task_budget(&result.session.id, &result.task.id)
            .unwrap(),
        None
    );
}

#[test]
fn a_reattached_worker_review_carries_the_same_budget() {
    let repo = temp_repo("reattach");
    let engine = engine_for(&repo, |_| {});
    let (worker, _session) = ask_in_worker(&engine, &repo);
    let proposal = worker.plan_proposal.expect("the worker plan is reviewed");
    let review = proposal
        .budget_review
        .clone()
        .expect("the review offers a budget");

    let reattached = workspace_engine::recovery::reattach_pending_approvals(
        &engine.session_store,
        &AuditLog::new(&engine.config.data_dir, true, Default::default()),
        &CommandStore::new(&engine.config.data_dir),
        &PatchStore::new(&engine.config.data_dir),
        &PausedTurns::new(&engine.config.data_dir),
        &worker.session.id,
    )
    .expect("approvals should reattach");
    let plan = reattached
        .into_iter()
        .find_map(|approval| match approval {
            ReattachedApproval::Plan { budget_review, .. } => Some(budget_review),
            _ => None,
        })
        .expect("the plan review should reattach");
    assert_eq!(plan, Some(review));
}

#[test]
fn a_corrupt_persisted_budget_is_refused() {
    let repo = temp_repo("corrupt");
    let engine = engine_for(&repo, |_| {});
    let (worker, _session) = ask_in_worker(&engine, &repo);
    let proposal = worker.plan_proposal.expect("the worker plan is reviewed");

    // Hand-edit the paused review to a zero budget, as a corrupt or malicious
    // older writer could have left it.
    let path = pending_path(&engine, &proposal.id);
    let mut pending: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
    pending["plan_review"]["budget"]["budget"]["actionRounds"] = serde_json::json!(0);
    fs::write(&path, serde_json::to_string(&pending).unwrap()).unwrap();

    let corrupt = TaskExecutionBudget {
        action_rounds: 0,
        exploration_rounds: engine
            .config
            .task_budget_policy()
            .default
            .exploration_rounds,
        input_tokens: engine.config.task_budget_policy().default.input_tokens,
        output_tokens: engine.config.task_budget_policy().default.output_tokens,
    };
    assert!(
        decide(&engine, &proposal.id, true, None, Some(&corrupt)).is_err(),
        "a zero persisted budget must not become an approval"
    );
    assert!(!event_kinds(&log_events(&engine, &worker.session.id)).contains(&"plan_approved"));
    assert!(
        path.is_file(),
        "the corrupt review is not silently consumed"
    );
}

#[test]
fn a_historical_pending_review_without_a_budget_still_loads() {
    let repo = temp_repo("historical");
    let engine = engine_for(&repo, |_| {});
    let (worker, _session) = ask_in_worker(&engine, &repo);
    let proposal = worker.plan_proposal.expect("the worker plan is reviewed");

    // Simulate a pending turn written before task budgets existed.
    let path = pending_path(&engine, &proposal.id);
    let mut pending: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
    pending["plan_review"]
        .as_object_mut()
        .unwrap()
        .remove("budget");
    fs::write(&path, serde_json::to_string(&pending).unwrap()).unwrap();

    assert_eq!(
        PausedTurns::new(&engine.config.data_dir)
            .plan_budget_review(&proposal.id)
            .unwrap(),
        None
    );
    let result = decide(&engine, &proposal.id, true, None, None)
        .expect("a historical review keeps the old approval path");
    assert!(
        engine
            .session_store
            .read_plan_approved(&result.session.id, &result.task.id)
            .unwrap()
    );
    assert_eq!(
        engine
            .session_store
            .read_task_budget(&result.session.id, &result.task.id)
            .unwrap(),
        None
    );
}

#[test]
fn replay_ignores_a_foreign_or_corrupt_plan_embedded_in_an_approval() {
    let repo = temp_repo("replay-hardening");
    let engine = engine_for(&repo, |_| {});
    let (worker, _session) = ask_in_worker(&engine, &repo);
    let _proposal = worker.plan_proposal.expect("the worker plan is reviewed");
    let original = engine
        .session_store
        .read_task_plan(&worker.session.id, &worker.task.id)
        .unwrap()
        .expect("the task has a created plan");
    let log = engine
        .config
        .data_dir
        .join("sessions")
        .join(format!("{}.jsonl", worker.session.id));

    let append = |payload: serde_json::Value| {
        let line = serde_json::json!({
            "eventId": "evt_hand_edited",
            "seq": 900_000,
            "timestampMs": 1,
            "eventType": "plan_approved",
            "payload": payload,
        });
        let mut content = fs::read_to_string(&log).unwrap();
        content.push_str(&serde_json::to_string(&line).unwrap());
        content.push('\n');
        fs::write(&log, content).unwrap();
    };

    // Task B's plan under task A's approval: a hand-edited log must not move
    // it. `plan.taskId` disagrees with the approval's own key.
    let mut foreign = original.clone();
    foreign.task_id = "task_somewhere_else".to_string();
    foreign.steps[0].title = "Foreign work".to_string();
    append(serde_json::json!({
        "taskId": worker.task.id,
        "approvedBy": "tester",
        "plan": foreign,
    }));

    // Structurally corrupt under the right key: two steps share an id, so the
    // replayed plan would be one nobody could update.
    let mut duplicate = original.clone();
    duplicate.steps.push(duplicate.steps[0].clone());
    append(serde_json::json!({
        "taskId": worker.task.id,
        "approvedBy": "tester",
        "plan": duplicate,
    }));

    // Not even a plan: an old or damaged writer.
    append(serde_json::json!({
        "taskId": worker.task.id,
        "approvedBy": "tester",
        "plan": {"taskId": worker.task.id, "steps": "not a list"},
    }));

    let store = workspace_engine::SessionStore::new(engine.config.data_dir.clone());
    let read = store
        .read_task_plan(&worker.session.id, &worker.task.id)
        .unwrap()
        .expect("the original created plan remains current");
    assert_eq!(read, original);
    assert_eq!(
        store
            .read_session_plans(&worker.session.id)
            .unwrap()
            .get(&worker.task.id),
        Some(&original),
        "the session-wide reader applies the same rule"
    );
}

#[test]
fn a_plan_with_a_foreign_task_id_is_refused_before_the_write() {
    let repo = temp_repo("plan-invariant");
    let engine = engine_for(&repo, |_| {});
    let (worker, _session) = ask_in_worker(&engine, &repo);
    let proposal = worker.plan_proposal.expect("the worker plan is reviewed");
    let review = proposal.budget_review.clone().unwrap();
    let mut foreign = proposal.plan.clone();
    foreign.task_id = "task_somewhere_else".to_string();

    // The store refuses a plan that belongs to another task. This is the
    // structural half of "validate the plan before the single write".
    assert!(
        engine
            .session_store
            .approve_plan_with_budget_and_plan(&worker.task, "tester", &review.budget, &foreign,)
            .is_err()
    );
    assert!(!event_kinds(&log_events(&engine, &worker.session.id)).contains(&"plan_approved"));
    assert_eq!(worker.task.status, TaskStatus::WaitingForApproval);
    assert!(proposal.id.starts_with("planreview"));
}
