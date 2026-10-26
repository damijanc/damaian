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
