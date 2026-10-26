//! Conversation compaction over the append-only session log (spec 55).
//!
//! Requirement 2 is the pair these tests hold together: compaction appends one
//! event and changes nothing that is stored. Every message stays readable for
//! display, rewind and export; only the model's view is reduced.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};
use workspace_engine::compaction::{
    ApprovalDigest, ConversationSummary, PlanStepDigest, SUMMARY_FORMAT_VERSION, ValidationDigest,
};
use workspace_engine::{PendingApprovalRef, SessionStore};

static COUNTER: AtomicU64 = AtomicU64::new(1);

fn temp_data_dir(name: &str) -> PathBuf {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock should work")
        .as_nanos();
    let dir = std::env::temp_dir().join(format!(
        "damaian-compaction-{name}-{now}-{}",
        COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir_all(&dir).expect("temp dir should be created");
    dir
}

fn session_log(data_dir: &Path, session_id: &str) -> PathBuf {
    data_dir
        .join("sessions")
        .join(format!("{session_id}.jsonl"))
}

fn summary(id: &str, through_seq: u64, messages: usize) -> ConversationSummary {
    ConversationSummary {
        id: id.to_string(),
        through_seq,
        format_version: SUMMARY_FORMAT_VERSION,
        objective: "Keep the 2021 edition and do not touch the public API.".to_string(),
        files_changed: vec!["src/lib.rs".to_string()],
        plan_steps: vec![PlanStepDigest {
            id: "step_1".to_string(),
            title: "Add the parser".to_string(),
            status: workspace_engine::plan::StepStatus::Completed,
        }],
        approvals: vec![ApprovalDigest {
            kind: "command".to_string(),
            id: "cmd_1".to_string(),
            decision: "approved".to_string(),
            subject: Some("cargo test".to_string()),
        }],
        validation: vec![ValidationDigest {
            label: "cargo test (repository root)".to_string(),
            outcome: "passed".to_string(),
        }],
        constraints: vec!["Do not change the public API.".to_string()],
        decisions: Vec::new(),
        failed_approaches: Vec::new(),
        uncertainty: Vec::new(),
        model_run_id: Some("modelrun_1".to_string()),
        message_count: messages,
    }
}

/// Twelve messages, returning their event seqs.
fn twelve_messages(store: &SessionStore, session_id: &str) -> Vec<u64> {
    let mut seqs = Vec::new();
    for index in 0..12 {
        let role = if index % 2 == 0 { "user" } else { "assistant" };
        store
            .append_message(session_id, None, role, &format!("message {index}"))
            .unwrap();
        seqs.push(store.latest_event_seq(session_id).unwrap());
    }
    seqs
}

#[test]
fn a_compaction_is_appended_and_never_rewrites_a_message() {
    let data_dir = temp_data_dir("append");
    let store = SessionStore::new(&data_dir);
    let session = store.create_session("repo_1", "Compacted").unwrap();
    let seqs = twelve_messages(&store, &session.id);
    let before = fs::read_to_string(session_log(&data_dir, &session.id)).unwrap();

    store
        .append_conversation_compaction(&session.id, &summary("s1", seqs[7], 8))
        .unwrap();

    let messages = store.read_messages(&session.id).unwrap();
    assert_eq!(messages.len(), 12, "every stored message is still returned");
    assert_eq!(messages[0].content, "message 0");
    let after = fs::read_to_string(session_log(&data_dir, &session.id)).unwrap();
    assert!(
        after.starts_with(&before),
        "the log only grew; no earlier byte changed"
    );
    assert!(after.contains("conversation_compacted"));
}

#[test]
fn the_model_view_is_the_summary_plus_later_messages() {
    let data_dir = temp_data_dir("view");
    let store = SessionStore::new(&data_dir);
    let session = store.create_session("repo_1", "Compacted").unwrap();
    let seqs = twelve_messages(&store, &session.id);
    store
        .append_conversation_compaction(&session.id, &summary("s1", seqs[7], 8))
        .unwrap();

    let view = store.read_conversation(&session.id).unwrap();
    assert_eq!(view.summary.as_ref().unwrap().id, "s1");
    assert_eq!(view.summarised_messages, 8);
    assert_eq!(
        view.messages
            .iter()
            .map(|message| message.content.as_str())
            .collect::<Vec<_>>(),
        vec!["message 8", "message 9", "message 10", "message 11"]
    );
    assert_eq!(
        store.read_messages(&session.id).unwrap().len(),
        12,
        "the display reader is unchanged"
    );
}

#[test]
fn rewinding_before_a_compaction_restores_the_full_model_view() {
    let data_dir = temp_data_dir("rewind");
    let store = SessionStore::new(&data_dir);
    let session = store.create_session("repo_1", "Compacted").unwrap();
    let seqs = twelve_messages(&store, &session.id);
    store
        .append_conversation_compaction(&session.id, &summary("s1", seqs[7], 8))
        .unwrap();
    store.rewind_conversation(&session.id, seqs[11]).unwrap();

    let view = store.read_conversation(&session.id).unwrap();
    assert!(
        view.summary.is_none(),
        "the compaction left the conversation"
    );
    assert_eq!(view.messages.len(), 12);
    assert_eq!(view.summarised_messages, 0);
}

#[test]
fn two_compactions_leave_one_active_summary_and_the_newest_wins() {
    let data_dir = temp_data_dir("twice");
    let store = SessionStore::new(&data_dir);
    let session = store.create_session("repo_1", "Compacted twice").unwrap();
    let seqs = twelve_messages(&store, &session.id);
    store
        .append_conversation_compaction(&session.id, &summary("s1", seqs[5], 6))
        .unwrap();
    store
        .append_message(&session.id, None, "user", "message 12")
        .unwrap();
    let last = store.latest_event_seq(&session.id).unwrap();
    store
        .append_conversation_compaction(&session.id, &summary("s2", last, 13))
        .unwrap();

    let compactions = store.read_conversation_compactions(&session.id).unwrap();
    assert_eq!(compactions.len(), 2);
    let view = store.read_conversation(&session.id).unwrap();
    assert_eq!(view.summary.as_ref().unwrap().id, "s2");
    assert!(view.messages.is_empty());
}

#[test]
fn a_compaction_payload_with_an_unknown_version_is_ignored() {
    let data_dir = temp_data_dir("version");
    let store = SessionStore::new(&data_dir);
    let session = store.create_session("repo_1", "Versioned").unwrap();
    let seqs = twelve_messages(&store, &session.id);
    store
        .append_conversation_compaction(&session.id, &summary("s1", seqs[5], 6))
        .unwrap();
    let mut future = summary("s999", seqs[11], 12);
    future.format_version = SUMMARY_FORMAT_VERSION + 1;
    let payload = serde_json::to_string(&future).unwrap();
    let mut file = fs::OpenOptions::new()
        .append(true)
        .open(session_log(&data_dir, &session.id))
        .unwrap();
    writeln!(
        file,
        "{{\"eventId\":\"evt_future\",\"seq\":{},\"timestampMs\":1,\"eventType\":\"conversation_compacted\",\"payload\":{payload}}}",
        store.latest_event_seq(&session.id).unwrap() + 1
    )
    .unwrap();

    let view = store.read_conversation(&session.id).unwrap();
    assert_eq!(
        view.summary.as_ref().unwrap().id,
        "s1",
        "an unknown format degrades to the summary that is readable"
    );
}

#[test]
fn a_pending_approval_clamps_the_compaction_limit_before_its_message() {
    let data_dir = temp_data_dir("pending");
    let store = SessionStore::new(&data_dir);
    let session = store.create_session("repo_1", "Pending").unwrap();
    let task = store
        .create_task(&session.id, "do the work", "provider", "model")
        .unwrap();
    store
        .append_message(&session.id, Some(&task.id), "user", "first")
        .unwrap();
    let first_seq = store.latest_event_seq(&session.id).unwrap();
    store
        .append_message(&session.id, Some(&task.id), "assistant", "working")
        .unwrap();
    store
        .await_approval(
            &task,
            &PendingApprovalRef {
                kind: "command".to_string(),
                proposal_id: "proposal_1".to_string(),
            },
        )
        .unwrap();

    let limit = store.compaction_limit(&session.id).unwrap();
    assert!(
        limit < first_seq,
        "the limit must stop before the paused task's first message"
    );
}

#[test]
fn a_dangling_action_clamps_the_compaction_limit_before_its_start() {
    let data_dir = temp_data_dir("dangling");
    let store = SessionStore::new(&data_dir);
    let session = store.create_session("repo_1", "Dangling").unwrap();
    let task = store
        .create_task(&session.id, "do the work", "provider", "model")
        .unwrap();
    store
        .append_message(&session.id, Some(&task.id), "user", "first")
        .unwrap();
    let marker = store
        .start_action(&task, "model_call", "call_1", false)
        .unwrap();
    let action_seq = store.latest_event_seq(&session.id).unwrap();
    store
        .append_message(&session.id, Some(&task.id), "assistant", "after")
        .unwrap();

    let limit = store.compaction_limit(&session.id).unwrap();
    assert_eq!(limit, action_seq - 1);
    let _ = marker;
}

#[test]
fn a_command_decision_is_recorded_in_the_session_log() {
    let data_dir = temp_data_dir("approval");
    let store = SessionStore::new(&data_dir);
    let session = store.create_session("repo_1", "Approvals").unwrap();
    store
        .record_conversation_approval(
            &session.id,
            "command",
            "cmd_1",
            "approved",
            Some("cargo test"),
        )
        .unwrap();
    let through = store.latest_event_seq(&session.id).unwrap();
    store
        .record_conversation_approval(&session.id, "command", "cmd_2", "rejected", None)
        .unwrap();

    let approvals = store
        .read_conversation_approvals_through(&session.id, through)
        .unwrap();
    assert_eq!(approvals.len(), 1);
    assert_eq!(approvals[0].id, "cmd_1");
    assert_eq!(approvals[0].decision, "approved");
    assert_eq!(approvals[0].subject.as_deref(), Some("cargo test"));
}

// ---------------------------------------------------------------------------
// The compaction engine (Task 5)
// ---------------------------------------------------------------------------

use workspace_engine::plan::{PlanStep, StepStatus, TaskPlan};
use workspace_engine::verification::{CheckId, CheckOutcome, VerificationEvent};
use workspace_engine::{
    CancelToken, ChatTurnResult, ClientError, Config, MockModelAdapter, ModelAdapter, ModelRequest,
    ModelRun, PatchFileDecision, Session, Task, TurnProgress, TurnSink, WorkspaceEngine,
};

fn temp_repo(name: &str) -> PathBuf {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock should work")
        .as_nanos();
    let repo = std::env::temp_dir().join(format!(
        "damaian-compaction-engine-{name}-{now}-{}",
        COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir_all(repo.join("src")).expect("repository should be created");
    fs::write(repo.join("src/lib.rs"), "pub fn one() {}\n").expect("a file");
    repo
}

/// A window of 1% of the default 16,000-token budget, so a few seeded turns
/// fall outside it. Automatic compaction is the caller's choice.
fn engine(repo: &Path, auto: bool) -> WorkspaceEngine {
    let config = Config {
        data_dir: repo.join(".damaian"),
        conversation_auto_compact: auto,
        conversation_window_share: 100,
        enable_index_watcher: false,
        ..Config::default()
    };
    WorkspaceEngine::new(config)
}

fn seed_session(engine: &WorkspaceEngine, repo: &Path, name: &str) -> (Session, Task) {
    let repository_id = engine
        .indexer
        .repository_id_for_path(repo)
        .expect("repository id");
    let session = engine
        .session_store
        .create_session(&repository_id, name)
        .unwrap();
    let task = engine
        .session_store
        .create_task(&session.id, "do the work", "openai", "gpt-4.1")
        .unwrap();
    (session, task)
}

fn seed_long_conversation(
    engine: &WorkspaceEngine,
    session: &Session,
    task: &Task,
    objective: &str,
) {
    let messages = (0..30)
        .map(|index| {
            let role = if index % 2 == 0 { "user" } else { "assistant" };
            let content = if index == 0 {
                objective.to_string()
            } else {
                format!("{role} message {index} {}", "x".repeat(180))
            };
            (role, content)
        })
        .collect::<Vec<_>>();
    for (role, content) in messages {
        engine
            .session_store
            .append_message(&session.id, Some(&task.id), role, &content)
            .unwrap();
    }
}

fn seed_facts(engine: &WorkspaceEngine, session: &Session, task: &Task) {
    let mut plan = TaskPlan::new(&task.id, 1);
    plan.steps.push(PlanStep {
        id: "step_1".to_string(),
        title: "Add the parser".to_string(),
        detail: None,
        status: StepStatus::Completed,
        depends_on: Vec::new(),
        started_at_ms: Some(1),
        completed_at_ms: Some(2),
        evidence: Vec::new(),
    });
    engine.session_store.create_plan(task, &plan).unwrap();
    engine
        .session_store
        .record_conversation_approval(
            &session.id,
            "command",
            "cmd_1",
            "approved",
            Some("cargo test"),
        )
        .unwrap();
    engine
        .session_store
        .append_patch_file_decision(
            &session.id,
            &PatchFileDecision {
                task_id: task.id.clone(),
                patch_id: "patch_1".to_string(),
                paths: vec!["src/lib.rs".to_string()],
                applied: true,
                applied_hashes: std::collections::BTreeMap::new(),
                accepted_hunks: std::collections::BTreeMap::new(),
            },
        )
        .unwrap();
    // A rejected patch is a decision, not a change. It must appear in the
    // approvals digest and never in `files_changed`.
    engine
        .session_store
        .append_patch_file_decision(
            &session.id,
            &PatchFileDecision {
                task_id: task.id.clone(),
                patch_id: "patch_2".to_string(),
                paths: vec!["src/rejected.rs".to_string()],
                applied: false,
                applied_hashes: std::collections::BTreeMap::new(),
                accepted_hunks: std::collections::BTreeMap::new(),
            },
        )
        .unwrap();
    engine
        .session_store
        .append_verification_event(
            &session.id,
            &VerificationEvent::Check {
                task_id: task.id.clone(),
                check: CheckId::new("", "cargo test"),
                outcome: CheckOutcome::Ran {
                    exit_code: Some(0),
                    marker_id: "marker_1".to_string(),
                },
                file_hashes: std::collections::BTreeMap::new(),
            },
        )
        .unwrap();
}

fn cancel() -> CancelToken {
    CancelToken::new()
}

#[test]
fn compaction_with_the_model_stubbed_out_still_has_every_fact() {
    let repo = temp_repo("facts");
    let engine = engine(&repo, false);
    let (session, task) = seed_session(&engine, &repo, "Facts");
    let objective = "Keep the 2021 edition and do not touch the public API.";
    // Facts are recorded during the span, so they precede the messages they
    // describe in this fixture.
    seed_facts(&engine, &session, &task);
    seed_long_conversation(&engine, &session, &task, objective);

    // The reply is not a narrative: the model half contributes nothing.
    let mut adapter = MockModelAdapter::new("The agent worked on the parser.");
    let outcome = engine
        .chat_orchestrator
        .compact_conversation(&repo, &session.id, &mut adapter, &cancel())
        .unwrap();
    let summary = outcome.summary().expect("compacted");

    assert_eq!(summary.objective, objective, "quoted, not paraphrased");
    assert_eq!(
        summary.files_changed,
        vec!["src/lib.rs"],
        "a rejected patch did not change a file"
    );
    assert!(
        summary
            .approvals
            .iter()
            .any(|approval| approval.kind == "patch"
                && approval.id == "patch_2"
                && approval.decision == "rejected"
                && approval.subject.as_deref() == Some("src/rejected.rs")),
        "the rejection is still a recorded decision"
    );
    assert_eq!(summary.plan_steps.len(), 1);
    assert_eq!(summary.plan_steps[0].title, "Add the parser");
    assert!(
        summary
            .approvals
            .iter()
            .any(|approval| approval.kind == "command"
                && approval.id == "cmd_1"
                && approval.decision == "approved")
    );
    assert!(summary.validation.iter().any(|validation| {
        validation.label == "cargo test (repository root)" && validation.outcome == "passed"
    }));
    assert!(summary.is_facts_only(), "the model wrote no narrative");
    assert!(
        summary.message_count > 0,
        "the summary covers messages, not a token count"
    );
    assert!(
        summary.model_run_id.is_some(),
        "the call is still accounted"
    );
}

struct FailingAdapter;

impl ModelAdapter for FailingAdapter {
    fn stream_response(
        &mut self,
        _request: &ModelRequest,
        _cancel: &CancelToken,
        _on_token: &mut dyn FnMut(&str),
        _on_wait: &mut dyn FnMut(u64),
    ) -> workspace_engine::Result<ModelRun> {
        Err(ClientError::Provider(
            workspace_engine::ProviderRefusal::Unknown,
            "provider refused the call".to_string(),
        ))
    }
}

#[test]
fn a_failed_summarisation_call_produces_a_facts_only_summary_and_no_turn_error() {
    let repo = temp_repo("failure");
    let engine = engine(&repo, false);
    let (session, task) = seed_session(&engine, &repo, "Failure");
    seed_facts(&engine, &session, &task);
    seed_long_conversation(&engine, &session, &task, "Keep the public API stable.");

    let mut adapter = FailingAdapter;
    let outcome = engine
        .chat_orchestrator
        .compact_conversation(&repo, &session.id, &mut adapter, &cancel())
        .unwrap();
    let summary = outcome.summary().expect("a facts-only summary is taken");
    assert!(summary.is_facts_only());
    assert!(summary.constraints.is_empty());
    assert_eq!(summary.files_changed, vec!["src/lib.rs"]);
    assert_eq!(
        engine
            .session_store
            .read_conversation(&session.id)
            .unwrap()
            .summary
            .as_ref()
            .unwrap()
            .id,
        summary.id
    );

    // The following turn still runs on the same engine.
    let mut adapter = MockModelAdapter::new("Done.");
    let result = ask(
        &engine,
        &repo,
        "What next?",
        Some(&session.id),
        &mut adapter,
    );
    assert_eq!(result.response, "Done.");
}

#[test]
fn a_compaction_that_would_cross_a_pending_approval_is_a_no_op() {
    let repo = temp_repo("pending-engine");
    let engine = engine(&repo, false);
    let (session, task) = seed_session(&engine, &repo, "Pending");
    engine
        .session_store
        .append_message(&session.id, Some(&task.id), "user", "first")
        .unwrap();
    engine
        .session_store
        .append_message(&session.id, Some(&task.id), "assistant", "working")
        .unwrap();
    engine
        .session_store
        .await_approval(
            &task,
            &PendingApprovalRef {
                kind: "command".to_string(),
                proposal_id: "proposal_1".to_string(),
            },
        )
        .unwrap();

    let mut adapter = MockModelAdapter::new("{}");
    let outcome = engine
        .chat_orchestrator
        .compact_conversation(&repo, &session.id, &mut adapter, &cancel())
        .unwrap();
    assert!(matches!(
        outcome,
        workspace_engine::compaction::CompactionOutcome::NothingToCompact
    ));
    assert!(
        engine
            .session_store
            .read_conversation_compactions(&session.id)
            .unwrap()
            .is_empty()
    );
    assert!(
        adapter.requests.is_empty(),
        "a refused span must not spend a model call"
    );
}

#[test]
fn the_summarisation_call_is_in_session_totals_but_not_a_turn_ceiling() {
    let repo = temp_repo("accounting");
    let engine = engine(&repo, false);
    let (session, task) = seed_session(&engine, &repo, "Accounting");
    seed_long_conversation(&engine, &session, &task, "Keep it.");
    let mut adapter = MockModelAdapter::new(
        r#"{"constraints":["keep it"],"decisions":[],"failed_approaches":[],"uncertainty":[]}"#,
    );
    let outcome = engine
        .chat_orchestrator
        .compact_conversation(&repo, &session.id, &mut adapter, &cancel())
        .unwrap();
    let summary = outcome.summary().expect("compacted");

    let totals = engine.session_store.read_task_usage(&session.id).unwrap();
    assert!(
        totals.keys().any(|id| id.starts_with("compaction_")),
        "the call appears in the session's totals"
    );
    assert!(
        !totals.contains_key(&task.id),
        "the summarisation call is not charged to the turn"
    );
    assert!(summary.model_run_id.is_some());
}

#[test]
fn automatic_compaction_runs_before_the_request_is_assembled() {
    let repo = temp_repo("automatic");
    let engine = engine(&repo, true);
    let (session, task) = seed_session(&engine, &repo, "Automatic");
    seed_long_conversation(&engine, &session, &task, "Do not touch the public API.");

    let mut adapter = MockModelAdapter::new_sequence(vec![
        r#"{"constraints":["do not touch the public API"],"decisions":[],"failed_approaches":[],"uncertainty":[]}"#.to_string(),
        "Done.".to_string(),
    ]);
    let result = ask(
        &engine,
        &repo,
        "What next?",
        Some(&session.id),
        &mut adapter,
    );
    assert_eq!(result.response, "Done.");

    assert_eq!(
        engine
            .session_store
            .read_conversation_compactions(&session.id)
            .unwrap()
            .len(),
        1
    );
    assert!(
        adapter.requests[0].messages[0]
            .content
            .contains("You are recording what happened"),
        "the first call is the summarisation"
    );
    assert!(
        adapter.requests[1].messages[1]
            .content
            .contains("--- begin prior conversation summary"),
        "the turn's request carries the summary"
    );
    assert!(
        adapter.requests[1].messages[1]
            .content
            .contains("What next?"),
        "the current request is never compacted away"
    );
}

fn ask(
    engine: &WorkspaceEngine,
    repo: &Path,
    prompt: &str,
    session_id: Option<&str>,
    adapter: &mut MockModelAdapter,
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
        .ask_with_session(repo, prompt, &[], session_id, adapter, &mut sink)
        .expect("the turn should run")
}

#[test]
fn auto_compaction_makes_no_model_call_when_switched_off() {
    let repo = temp_repo("off");
    let engine = engine(&repo, false);
    let (session, task) = seed_session(&engine, &repo, "Off");
    seed_long_conversation(&engine, &session, &task, "Do not touch the public API.");

    let mut adapter = MockModelAdapter::new("Done.");
    let result = ask(
        &engine,
        &repo,
        "What next?",
        Some(&session.id),
        &mut adapter,
    );
    assert_eq!(result.response, "Done.");
    assert_eq!(adapter.requests.len(), 1, "only the turn's own call");
    assert!(
        !adapter.requests[0].messages[0]
            .content
            .contains("You are recording what happened"),
        "no summarisation call was made"
    );
    assert!(
        engine
            .session_store
            .read_conversation_compactions(&session.id)
            .unwrap()
            .is_empty()
    );
}

#[test]
fn a_compacted_session_sends_the_summary_and_the_window_and_read_messages_still_has_everything() {
    let repo = temp_repo("both-halves");
    let engine = engine(&repo, false);
    let (session, task) = seed_session(&engine, &repo, "Both halves");
    seed_facts(&engine, &session, &task);
    seed_long_conversation(&engine, &session, &task, "Do not touch the public API.");

    let mut adapter = MockModelAdapter::new(
        r#"{"constraints":["do not touch the public API"],"decisions":[],"failed_approaches":[],"uncertainty":[]}"#,
    );
    engine
        .chat_orchestrator
        .compact_conversation(&repo, &session.id, &mut adapter, &cancel())
        .unwrap();

    let mut adapter = MockModelAdapter::new("Done.");
    ask(
        &engine,
        &repo,
        "What next?",
        Some(&session.id),
        &mut adapter,
    );
    let sent = &adapter.requests[0].messages[1].content;

    assert!(sent.contains("--- begin prior conversation summary"));
    assert!(sent.contains("do not touch the public API"));
    assert!(sent.contains("What next?"), "the request is still sent");
    assert!(
        sent.contains("message 29"),
        "the newest messages are sent verbatim"
    );

    let stored = engine.session_store.read_messages(&session.id).unwrap();
    assert_eq!(
        stored.len(),
        32,
        "30 seeded + the new request and answer, all still readable"
    );
    assert_eq!(stored[0].content, "Do not touch the public API.");
}

#[test]
fn a_fact_the_model_omitted_is_still_in_the_next_request() {
    let repo = temp_repo("facts-only-request");
    let engine = engine(&repo, false);
    let (session, task) = seed_session(&engine, &repo, "Facts only request");
    seed_facts(&engine, &session, &task);
    seed_long_conversation(&engine, &session, &task, "Keep the public API stable.");

    // A reply that is not a narrative: the model wrote nothing.
    let mut adapter = MockModelAdapter::new("no record available");
    engine
        .chat_orchestrator
        .compact_conversation(&repo, &session.id, &mut adapter, &cancel())
        .unwrap();

    let mut adapter = MockModelAdapter::new("Done.");
    ask(
        &engine,
        &repo,
        "What next?",
        Some(&session.id),
        &mut adapter,
    );
    let sent = &adapter.requests[0].messages[1].content;
    assert!(
        sent.contains("Files changed (from Damaian's patch records): src/lib.rs"),
        "a fact the model did not write is still sent: {sent}"
    );
    assert!(sent.contains("cargo test (repository root): passed"));
    assert!(sent.contains("no model-written narrative was recorded for this span"));
}

#[test]
fn switching_auto_off_still_states_the_loss_and_leaves_manual_compaction_working() {
    let repo = temp_repo("off-manual");
    let engine = engine(&repo, false);
    let (session, task) = seed_session(&engine, &repo, "Off manual");
    seed_long_conversation(&engine, &session, &task, "Do not touch the public API.");

    // Auto off: no call, and the request says what was left out.
    let mut adapter = MockModelAdapter::new("Done.");
    ask(
        &engine,
        &repo,
        "What next?",
        Some(&session.id),
        &mut adapter,
    );
    let sent = &adapter.requests[0].messages[1].content;
    assert!(
        sent.contains("earlier message") && sent.contains("were not sent to you"),
        "the omission is stated: {sent}"
    );
    assert!(
        sent.contains("were not summarised"),
        "and the user is told it was not summarised: {sent}"
    );
    assert_eq!(adapter.requests.len(), 1);

    // Manual compaction still works with the automatic half off.
    let mut adapter = MockModelAdapter::new(
        r#"{"constraints":[],"decisions":[],"failed_approaches":[],"uncertainty":[]}"#,
    );
    let outcome = engine
        .chat_orchestrator
        .compact_conversation(&repo, &session.id, &mut adapter, &cancel())
        .unwrap();
    assert!(outcome.summary().is_some());
    assert_eq!(
        engine
            .session_store
            .read_conversation_compactions(&session.id)
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn a_second_compaction_summarises_the_first_one_plus_what_followed() {
    let repo = temp_repo("compound");
    let engine = engine(&repo, false);
    let (session, task) = seed_session(&engine, &repo, "Compound");
    seed_facts(&engine, &session, &task);
    seed_long_conversation(&engine, &session, &task, "Do not touch the public API.");

    let mut first = MockModelAdapter::new(
        r#"{"constraints":["do not touch the public API"],"decisions":["kept the parser"],"failed_approaches":[],"uncertainty":[]}"#,
    );
    engine
        .chat_orchestrator
        .compact_conversation(&repo, &session.id, &mut first, &cancel())
        .unwrap();

    // More conversation after the first boundary.
    for index in 0..8 {
        engine
            .session_store
            .append_message(
                &session.id,
                Some(&task.id),
                if index % 2 == 0 { "user" } else { "assistant" },
                &format!("later {index} {}", "y".repeat(200)),
            )
            .unwrap();
    }
    let mut second = MockModelAdapter::new(
        r#"{"constraints":["do not touch the public API"],"decisions":[],"failed_approaches":[],"uncertainty":[]}"#,
    );
    engine
        .chat_orchestrator
        .compact_conversation(&repo, &session.id, &mut second, &cancel())
        .unwrap();

    let prompt = &second.requests[0].messages[0].content;
    assert!(
        prompt.contains("A previous summary covers the earlier part"),
        "the second summary is asked for with the first as input"
    );
    assert!(
        prompt.contains("--- begin prior conversation summary"),
        "the first summary is rendered into the call"
    );
    assert!(prompt.contains("kept the parser"));
    let summaries = engine
        .session_store
        .read_conversation_compactions(&session.id)
        .unwrap();
    assert_eq!(summaries.len(), 2);
    let second_summary = summaries.last().unwrap();
    assert_eq!(
        second_summary.objective, "Do not touch the public API.",
        "the original objective survives a second summary"
    );
    assert_eq!(
        second_summary.files_changed,
        vec!["src/lib.rs"],
        "a file changed before the first boundary is still in the record"
    );
    assert!(
        second_summary
            .approvals
            .iter()
            .any(|approval| approval.id == "cmd_1"),
        "an approval before the first boundary is still in the record"
    );
    assert!(
        second_summary
            .validation
            .iter()
            .any(|validation| validation.label == "cargo test (repository root)"),
        "validation before the first boundary is still in the record"
    );
    assert!(
        second_summary.message_count > summaries[0].message_count,
        "the covered-message count accumulates"
    );
}

#[test]
fn a_failed_compaction_is_recorded_for_the_user_and_not_only_in_the_audit_log() {
    let data_dir = temp_data_dir("failure-note");
    let store = SessionStore::new(&data_dir);
    let session = store.create_session("repo_1", "Note").unwrap();
    assert!(
        store
            .read_conversation_compaction_failure(&session.id)
            .unwrap()
            .is_none()
    );
    store
        .record_conversation_compaction_failure(&session.id, "provider refused")
        .unwrap();
    assert_eq!(
        store
            .read_conversation_compaction_failure(&session.id)
            .unwrap()
            .as_deref(),
        Some("provider refused")
    );
    assert!(
        store.read_messages(&session.id).unwrap().is_empty(),
        "a failure event is not a message"
    );
}

#[test]
fn a_secret_in_the_conversation_never_leaves_through_the_summary_or_its_call() {
    let repo = temp_repo("secret");
    let engine = engine(&repo, false);
    let (session, task) = seed_session(&engine, &repo, "Secret");
    let objective = "Never print the key AKIAIOSFODNN7EXAMPLE in an answer.";
    seed_long_conversation(&engine, &session, &task, objective);

    let mut adapter = MockModelAdapter::new(
        r#"{"constraints":[],"decisions":[],"failed_approaches":[],"uncertainty":[]}"#,
    );
    engine
        .chat_orchestrator
        .compact_conversation(&repo, &session.id, &mut adapter, &cancel())
        .unwrap();
    assert!(
        !adapter.requests[0].messages[0]
            .content
            .contains("AKIAIOSFODNN7EXAMPLE"),
        "the summarisation call is redacted before it leaves the machine"
    );

    // The stored objective is still the user's own words, exactly.
    let summary = engine
        .session_store
        .read_conversation(&session.id)
        .unwrap()
        .summary
        .expect("a summary");
    assert!(summary.objective.contains("AKIAIOSFODNN7EXAMPLE"));

    // The next request renders the summary, and the boundary redacts it.
    let mut adapter = MockModelAdapter::new("Done.");
    ask(
        &engine,
        &repo,
        "What did I ask?",
        Some(&session.id),
        &mut adapter,
    );
    assert!(
        !adapter.requests[0].messages[1]
            .content
            .contains("AKIAIOSFODNN7EXAMPLE"),
        "the summary reaches the model redacted"
    );
}

#[test]
fn a_cancelled_compaction_leaves_no_failure_notice() {
    let repo = temp_repo("cancelled");
    let engine = engine(&repo, false);
    let (session, task) = seed_session(&engine, &repo, "Cancelled");
    seed_long_conversation(&engine, &session, &task, "Do not touch the public API.");

    let cancel = CancelToken::new();
    cancel.cancel();
    let mut adapter = MockModelAdapter::new("{}");
    let outcome = engine
        .chat_orchestrator
        .compact_conversation(&repo, &session.id, &mut adapter, &cancel)
        .unwrap();
    assert!(matches!(
        outcome,
        workspace_engine::compaction::CompactionOutcome::Cancelled
    ));
    assert!(
        engine
            .session_store
            .read_conversation_compactions(&session.id)
            .unwrap()
            .is_empty()
    );
    assert!(
        engine
            .session_store
            .read_conversation_compaction_failure(&session.id)
            .unwrap()
            .is_none(),
        "stopping a turn must not leave a permanent failure notice"
    );
    assert!(adapter.requests.is_empty(), "no call is made after a stop");
}
