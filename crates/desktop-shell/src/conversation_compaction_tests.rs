use super::{conversation_json, run_compact_request};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};
use workspace_engine::compaction::{
    ApprovalDigest, ConversationSummary, PlanStepDigest, SUMMARY_FORMAT_VERSION, ValidationDigest,
};
use workspace_engine::{
    CancelToken, Config, MockModelAdapter, Task, TurnProgress, TurnSink, WorkspaceEngine,
};

static COUNTER: AtomicU64 = AtomicU64::new(1);

fn temp_repo() -> PathBuf {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock should work")
        .as_nanos();
    let repo = std::env::temp_dir().join(format!(
        "damaian-shell-compaction-{now}-{}",
        COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir_all(repo.join("src")).expect("repository");
    repo
}

fn engine(repo: &Path) -> WorkspaceEngine {
    WorkspaceEngine::new(Config {
        data_dir: repo.join(".damaian"),
        // 0.25% of the 64,000-token default budget: a 160-token window, as
        // 1% of the old 16,000 default was.
        conversation_window_share: 25,
        enable_index_watcher: false,
        ..Config::default()
    })
}

fn seeded(engine: &WorkspaceEngine, repo: &Path) -> (String, Task) {
    let repository_id = engine.indexer.repository_id_for_path(repo).unwrap();
    let session = engine
        .session_store
        .create_session(&repository_id, "Compacted")
        .unwrap();
    let task = engine
        .session_store
        .create_task(&session.id, "work", "openai", "gpt-4.1")
        .unwrap();
    for index in 0..12 {
        engine
            .session_store
            .append_message(
                &session.id,
                Some(&task.id),
                if index % 2 == 0 { "user" } else { "assistant" },
                &format!("message {index} {}", "x".repeat(200)),
            )
            .unwrap();
    }
    (session.id, task)
}

fn summary(id: &str, through_seq: u64) -> ConversationSummary {
    ConversationSummary {
        id: id.to_string(),
        through_seq,
        format_version: SUMMARY_FORMAT_VERSION,
        objective: "Do not touch the public API.".to_string(),
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
        constraints: vec!["keep the API stable".to_string()],
        decisions: Vec::new(),
        failed_approaches: Vec::new(),
        uncertainty: Vec::new(),
        narrative_carried_forward: false,
        model_run_id: Some("modelrun_1".to_string()),
        message_count: 8,
    }
}

#[test]
fn the_session_payload_carries_its_compactions_and_boundary() {
    let repo = temp_repo();
    let engine = engine(&repo);
    let (session_id, _) = seeded(&engine, &repo);
    let messages = engine
        .session_store
        .read_messages_with_seq(&session_id)
        .unwrap();
    let through = messages[7].0;
    engine
        .session_store
        .append_conversation_compaction(&session_id, &summary("summary_1", through))
        .unwrap();

    let payload: serde_json::Value =
        serde_json::from_str(&conversation_json(&engine, &session_id)).expect("JSON");
    assert_eq!(payload["compactions"][0]["id"], "summary_1");
    assert_eq!(payload["compactions"][0]["formatVersion"], 1);
    assert_eq!(payload["compactions"][0]["messageCount"], 8);
    assert_eq!(payload["conversation"]["kind"], "summary");
    assert_eq!(payload["conversation"]["throughSeq"], through);
    assert_eq!(payload["conversation"]["autoCompact"], true);
    assert!(
        payload["conversation"]["omittedMessages"].as_u64().unwrap() >= 8,
        "the covered messages are counted as omitted from the request"
    );
    assert!(
        payload["conversation"]["canCompact"].as_bool().unwrap(),
        "the messages after the summary still exceed the window"
    );
    assert_eq!(
        engine
            .session_store
            .read_messages(&session_id)
            .unwrap()
            .len(),
        12,
        "the display reader still has every message"
    );
}

#[test]
fn the_boundary_counts_match_the_prompt_when_a_plan_reminder_is_present() {
    let repo = temp_repo();
    let engine = WorkspaceEngine::new(Config {
        data_dir: repo.join(".damaian"),
        // 0.13% of 64,000 tokens: 83, so a 12-message seed is only partly in
        // view and the notice has something to count. Automatic
        // compaction is off so the first request is the ask, not a summary
        // call.
        conversation_window_share: 13,
        conversation_auto_compact: false,
        enable_index_watcher: false,
        ..Config::default()
    });
    let (session_id, _) = seeded(&engine, &repo);
    // The engine's own reminder is a user-role message in storage but not
    // a sendable one; if it consumed the window the marker would disagree
    // with the request.
    engine
        .session_store
        .append_message_with_kind(
            &session_id,
            None,
            "user",
            &"r".repeat(4_000),
            Some("plan_reminder"),
        )
        .unwrap();

    let payload: serde_json::Value =
        serde_json::from_str(&conversation_json(&engine, &session_id)).expect("JSON");
    let conversation = &payload["conversation"];
    let omitted = conversation["omittedMessages"].as_u64().unwrap();
    let window = conversation["windowMessages"].as_u64().unwrap();
    assert_eq!(
        window + omitted,
        12,
        "the 12 sendable messages split into the window and the notice; the reminder is neither"
    );

    let mut adapter = MockModelAdapter::new("Done.");
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
            &repo,
            "What next?",
            &[],
            Some(&session_id),
            &mut adapter,
            &mut sink,
        )
        .expect("the turn runs");
    let prompt = &adapter.requests[0].messages[1].content;
    let notice = prompt
        .split("[Damaian: ")
        .nth(1)
        .and_then(|rest| rest.split(' ').next())
        .and_then(|count| count.parse::<u64>().ok())
        .expect("the request states how many messages were not sent");
    assert_eq!(
        notice, omitted,
        "the marker and the request must count the same messages: {prompt}"
    );
}

#[test]
fn the_session_payload_reports_an_uncompacted_boundary_as_elision() {
    let repo = temp_repo();
    let engine = engine(&repo);
    let (session_id, _) = seeded(&engine, &repo);
    let payload: serde_json::Value =
        serde_json::from_str(&conversation_json(&engine, &session_id)).expect("JSON");
    assert_eq!(payload["conversation"]["kind"], "elision");
    assert!(payload["conversation"]["omittedMessages"].as_u64().unwrap() > 0);
    assert!(payload["conversation"]["canCompact"].as_bool().unwrap());
}

#[test]
fn the_compact_route_refuses_while_a_turn_is_running() {
    let repo = temp_repo();
    let engine = engine(&repo);
    let (session_id, task) = seeded(&engine, &repo);
    engine
        .session_store
        .update_task_status(&task, workspace_engine::TaskStatus::RunningTool, None)
        .unwrap();
    let error = super::compact_session(&engine, &repo.to_string_lossy(), &session_id)
        .expect_err("a running turn is not a boundary");
    assert!(error.contains("A turn is running"), "{error}");
}

#[test]
fn the_compact_route_refuses_a_repository_without_a_session() {
    let repo = temp_repo();
    let error = run_compact_request(
        &repo.to_string_lossy(),
        "session_that_does_not_exist",
        &HashMap::new(),
    )
    .expect_err("an unknown session is an error");
    assert!(error.contains("Unknown session"), "{error}");
}
