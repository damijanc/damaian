//! One pending decision per session, per
//! `docs/specs/113_one_pending_decision/proposal.md` requirement 3.
//!
//! A task that pauses on a decision settles every older pause of its session:
//! the older paused turn leaves the disk, the older task becomes `Cancelled`,
//! and `pending_decision_superseded` names the newer task. The startup sweep
//! does the same to a session that already holds several. Nothing is approved
//! or run by either, and a worker session is left alone (spec 57).

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use workspace_engine::{
    CancelToken, ChatTurnResult, CommandStore, Config, MockModelAdapter, PatchStore, PausedTurns,
    PendingApprovalRef, ReattachedApproval, Session, Task, TaskControlService, TaskStatus,
    ToolCall, TurnProgress, TurnSink, WorkerRecord, WorkspaceEngine, keep_newest_pending_decision,
    reattach_pending_approvals, repository_id_for_root,
};

static COUNTER: AtomicU64 = AtomicU64::new(1);

fn temp_repo(name: &str) -> PathBuf {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock should work")
        .as_nanos();
    let repo = std::env::temp_dir().join(format!(
        "damaian-one-pending-{name}-{now}-{}",
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

fn call(name: &str, arguments_json: &str) -> ToolCall {
    ToolCall {
        id: format!("call_{name}"),
        name: name.to_string(),
        arguments_json: arguments_json.to_string(),
    }
}

/// A turn that proposes a plan, which an ordinary session pauses on for
/// review before the patch that follows could be proposed.
fn plan_then_patch() -> MockModelAdapter {
    MockModelAdapter::new_sequence_with_tool_calls(
        vec![String::new(), String::new(), "Done.".to_string()],
        vec![
            vec![call(
                "propose_plan",
                r#"{"steps":[{"title":"Read the retry helper"},{"title":"Add a bounded backoff"}]}"#,
            )],
            vec![call(
                "propose_patch",
                r#"{"summary":"Add src/retry.rs","files":[{"path":"src/retry.rs","content":"pub fn added() {}\n"}]}"#,
            )],
            Vec::new(),
        ],
    )
}

fn ask(
    engine: &WorkspaceEngine,
    repo: &Path,
    prompt: &str,
    session: Option<&str>,
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
        .ask_with_session(
            repo,
            prompt,
            &[],
            session,
            &mut plan_then_patch(),
            &mut sink,
        )
        .expect("the turn should run")
}

/// The id of the decision a paused turn is waiting on, whatever its kind.
fn pending_id(result: &ChatTurnResult) -> String {
    if let Some(plan) = &result.plan_proposal {
        return plan.id.clone();
    }
    if let Some(command) = &result.command_proposal {
        return command.id.clone();
    }
    if let Some(patch) = &result.patch_proposal {
        return patch.patch_id.clone();
    }
    panic!("the turn should have paused on a decision, got {result:?}");
}

fn pending_path(engine: &WorkspaceEngine, proposal_id: &str) -> PathBuf {
    engine
        .config
        .data_dir
        .join("chat")
        .join("pending")
        .join(format!("{proposal_id}.json"))
}

fn status_of(engine: &WorkspaceEngine, session_id: &str, task_id: &str) -> TaskStatus {
    let raw = engine
        .session_store
        .read_task_statuses(session_id)
        .unwrap()
        .remove(task_id)
        .expect("the task should be in the log");
    TaskStatus::parse(&raw).expect("a known status")
}

fn audit_events(engine: &WorkspaceEngine) -> Vec<serde_json::Value> {
    fs::read_to_string(engine.config.data_dir.join("audit").join("events.jsonl"))
        .unwrap_or_default()
        .lines()
        .filter_map(|line| serde_json::from_str(line).ok())
        .collect()
}

fn superseded_events(engine: &WorkspaceEngine) -> Vec<serde_json::Value> {
    audit_events(engine)
        .into_iter()
        .filter(|event| event["eventType"] == "pending_decision_superseded")
        .collect()
}

fn reattach(engine: &WorkspaceEngine, session_id: &str) -> Vec<ReattachedApproval> {
    reattach_pending_approvals(
        &engine.session_store,
        &engine.audit_log,
        &CommandStore::new(&engine.config.data_dir),
        &PatchStore::new(&engine.config.data_dir),
        &PausedTurns::new(&engine.config.data_dir),
        session_id,
    )
    .expect("the sweep should run")
}

fn reattached_task(approval: &ReattachedApproval) -> &str {
    match approval {
        ReattachedApproval::Plan { task_id, .. }
        | ReattachedApproval::Command { task_id, .. }
        | ReattachedApproval::RoundBudget { task_id, .. }
        | ReattachedApproval::Unavailable { task_id, .. } => task_id,
        other => panic!("unexpected reattachment {other:?}"),
    }
}

/// Asserts the two settled-pause effects that must never happen: nothing the
/// older turns proposed was approved, and nothing was written or run.
fn assert_nothing_approved_or_run(engine: &WorkspaceEngine, repo: &Path) {
    assert!(
        !repo.join("src/retry.rs").exists(),
        "a settled pause must not apply the patch its turn would have proposed"
    );
    for event in audit_events(engine) {
        let kind = event["eventType"].as_str().unwrap_or_default();
        assert!(
            !matches!(
                kind,
                "plan_review_decision" | "command_executed" | "command_approved" | "patch_applied"
            ),
            "settling a pause approved or ran something: {event}"
        );
    }
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

/// Leaves a task waiting on `kind` with a paused turn on disk that is a copy of
/// `template` (a real paused turn), which is the state a version without this
/// spec left behind: several tasks of one session each waiting.
fn piled_up_pause(
    engine: &WorkspaceEngine,
    session_id: &str,
    template: &Path,
    proposal_id: &str,
    kind: &str,
) -> Task {
    let task = engine
        .session_store
        .create_task(session_id, "proceed with the plan", "mock", "m")
        .unwrap();
    let mut turn: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(template).unwrap()).unwrap();
    turn["proposal_id"] = serde_json::Value::from(proposal_id);
    fs::write(pending_path(engine, proposal_id), turn.to_string()).unwrap();
    engine
        .session_store
        .await_approval(
            &task,
            &PendingApprovalRef {
                kind: kind.to_string(),
                proposal_id: proposal_id.to_string(),
            },
        )
        .unwrap()
}

/// Requirement 3, first acceptance criterion: the incident's shape. Task A
/// paused on a plan review; the user typed again, and the new task B paused on
/// a decision of its own. A is settled, B is the one decision left.
#[test]
fn a_new_pause_settles_the_older_pause_of_the_same_session() {
    let repo = temp_repo("live");
    let engine = engine_for(&repo);

    let first = ask(&engine, &repo, "Add retry handling", None);
    let session_id = first.session.id.clone();
    let older = first.plan_proposal.as_ref().expect("A pauses on its plan");
    assert_eq!(first.task.status, TaskStatus::WaitingForApproval);
    assert!(pending_path(&engine, &older.id).exists());

    let second = ask(&engine, &repo, "proceed with the plan", Some(&session_id));
    assert_eq!(second.session.id, session_id);
    assert_ne!(second.task.id, first.task.id);
    assert_eq!(second.task.status, TaskStatus::WaitingForApproval);
    let newer_id = pending_id(&second);

    assert_eq!(
        status_of(&engine, &session_id, &first.task.id),
        TaskStatus::Cancelled
    );
    assert!(
        !pending_path(&engine, &older.id).exists(),
        "A's paused turn must be taken off disk"
    );
    assert!(
        pending_path(&engine, &newer_id).exists() || second.patch_proposal.is_some(),
        "B's own pause must survive"
    );
    assert_eq!(
        status_of(&engine, &session_id, &second.task.id),
        TaskStatus::WaitingForApproval
    );

    let superseded = superseded_events(&engine);
    assert_eq!(superseded.len(), 1, "{superseded:?}");
    let event = &superseded[0];
    assert_eq!(event["taskId"], first.task.id.as_str());
    assert_eq!(event["supersededBy"], second.task.id.as_str());
    assert_eq!(event["kind"], "plan");
    assert_eq!(event["pausedTurnTaken"], "true");

    // A has nothing pending any more, so neither the reload card nor the
    // task-control presentation offers it.
    assert!(
        TaskControlService::new(&engine.session_store, None)
            .pending_decision(&session_id, &first.task.id, &TaskStatus::Cancelled)
            .unwrap()
            .is_none()
    );

    let reattached = reattach(&engine, &session_id);
    assert_eq!(reattached.len(), 1, "{reattached:?}");
    assert_eq!(reattached_task(&reattached[0]), second.task.id);

    // The settled review's resume route has nothing left to resume.
    let cancel = CancelToken::new();
    let mut on_token = |_token: &str| {};
    let mut on_progress = |_event: TurnProgress| {};
    let mut sink = TurnSink {
        on_token: &mut on_token,
        on_progress: &mut on_progress,
        cancel: &cancel,
        steering: None,
    };
    let resumed = engine
        .chat_orchestrator
        .resume_after_plan_decision_with_budget(
            &older.id,
            true,
            None,
            None,
            "tester",
            &mut MockModelAdapter::new("Done."),
            &mut sink,
        );
    assert!(
        resumed.is_err(),
        "a settled plan review must not be approvable"
    );
    assert_eq!(
        status_of(&engine, &session_id, &first.task.id),
        TaskStatus::Cancelled
    );

    assert_nothing_approved_or_run(&engine, &repo);
}

/// Requirement 3, second acceptance criterion: a data dir left by a version
/// without this spec, one session holding three pending decisions. The sweep
/// keeps the newest and settles the other two before reattaching.
#[test]
fn the_startup_sweep_keeps_only_the_newest_of_three_pending_decisions() {
    let repo = temp_repo("sweep");
    let engine = engine_for(&repo);

    // A real paused plan review in another session supplies the shape of a
    // paused turn on disk. That session must come through untouched.
    let template = ask(&engine, &repo, "Add retry handling", None);
    let template_id = template.plan_proposal.as_ref().unwrap().id.clone();
    let template_path = pending_path(&engine, &template_id);

    let session = engine
        .session_store
        .create_session(&repository_id_for_root(&repo), "Snake")
        .unwrap();
    let oldest = piled_up_pause(&engine, &session.id, &template_path, "plan_one", "plan");
    let middle = piled_up_pause(&engine, &session.id, &template_path, "plan_two", "plan");
    // The newest is a round-budget pause, which reattaches from its paused
    // turn alone; the pile is of mixed kinds in practice too.
    let newest = engine
        .session_store
        .create_task(&session.id, "proceed with the plan", "mock", "m")
        .unwrap();
    fs::write(
        pending_path(&engine, "rb_three"),
        r#"{"proposal_id":"rb_three","round_budget":{"rounds_used":8,"rounds_granted":16}}"#,
    )
    .unwrap();
    engine
        .session_store
        .await_approval(
            &newest,
            &PendingApprovalRef {
                kind: "round_budget".to_string(),
                proposal_id: "rb_three".to_string(),
            },
        )
        .unwrap();

    let reattached = reattach(&engine, &session.id);

    assert_eq!(reattached.len(), 1, "{reattached:?}");
    match &reattached[0] {
        ReattachedApproval::RoundBudget {
            task_id,
            proposal_id,
            ..
        } => {
            assert_eq!(task_id, &newest.id);
            assert_eq!(proposal_id, "rb_three");
        }
        other => panic!("the newest pause should reattach, got {other:?}"),
    }
    for (task, proposal_id) in [(&oldest, "plan_one"), (&middle, "plan_two")] {
        assert_eq!(
            status_of(&engine, &session.id, &task.id),
            TaskStatus::Cancelled
        );
        assert!(!pending_path(&engine, proposal_id).exists());
    }
    assert!(pending_path(&engine, "rb_three").exists());

    let superseded = superseded_events(&engine);
    assert_eq!(superseded.len(), 2, "{superseded:?}");
    for event in &superseded {
        assert_eq!(event["supersededBy"], newest.id.as_str());
        assert_eq!(event["kind"], "plan");
    }

    // Another session's pause is its own decision.
    assert_eq!(
        status_of(&engine, &template.session.id, &template.task.id),
        TaskStatus::WaitingForApproval
    );
    assert!(template_path.exists());

    // A second launch finds one decision and settles nothing more.
    assert!(
        keep_newest_pending_decision(&engine.session_store, &engine.audit_log, &session.id)
            .unwrap()
            .is_empty()
    );
    assert_nothing_approved_or_run(&engine, &repo);
}

/// A worker session's pauses belong to the worker (spec 57): neither a new
/// pause nor the sweep settles an older one there.
#[test]
fn a_worker_sessions_pauses_are_left_alone() {
    let repo = temp_repo("worker");
    let engine = engine_for(&repo);
    let worker = worker_session(&engine, &repo);

    let first = ask(&engine, &repo, "Add retry handling", Some(&worker.id));
    let first_id = pending_id(&first);
    let second = ask(&engine, &repo, "Add retry handling", Some(&worker.id));
    assert_eq!(second.task.status, TaskStatus::WaitingForApproval);

    assert_eq!(
        status_of(&engine, &worker.id, &first.task.id),
        TaskStatus::WaitingForApproval
    );
    assert!(pending_path(&engine, &first_id).exists());

    assert!(
        keep_newest_pending_decision(&engine.session_store, &engine.audit_log, &worker.id)
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        status_of(&engine, &worker.id, &first.task.id),
        TaskStatus::WaitingForApproval
    );
    assert!(superseded_events(&engine).is_empty());
    assert_nothing_approved_or_run(&engine, &repo);
}
