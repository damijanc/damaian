//! Conversation rewind over the append-only session log, per
//! `docs/specs/16_session_checkpoints_and_rewind.md` §5.5.
//!
//! Rewind must not truncate the log: tasks are replayed from it and it is the
//! audit trail. So it appends a `conversation_rewound` marker and every reader
//! treats later events as inert. These tests hold that line — the log keeps
//! every byte, and the active conversation moves back.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};
use workspace_engine::{SessionStore, TaskStatus};

static COUNTER: AtomicU64 = AtomicU64::new(1);

fn temp_data_dir(name: &str) -> PathBuf {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock should work")
        .as_nanos();
    let dir = std::env::temp_dir().join(format!(
        "damaian-rewind-{name}-{now}-{}",
        COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir_all(&dir).expect("temp dir should be created");
    dir
}

fn session_log(data_dir: &Path, session_id: &str) -> String {
    fs::read_to_string(
        data_dir
            .join("sessions")
            .join(format!("{session_id}.jsonl")),
    )
    .expect("session log should exist")
}

#[test]
fn every_appended_event_carries_the_next_sequence_number() {
    let data_dir = temp_data_dir("seq");
    let store = SessionStore::new(&data_dir);

    let session = store.create_session("repo_1", "Sequenced").unwrap();
    store
        .append_message(&session.id, None, "user", "one")
        .unwrap();
    store
        .append_message(&session.id, None, "assistant", "two")
        .unwrap();

    let log = session_log(&data_dir, &session.id);
    let seqs: Vec<&str> = log
        .lines()
        .map(|line| {
            line.split("\"seq\":")
                .nth(1)
                .expect("every event should carry a seq")
                .split(',')
                .next()
                .unwrap()
        })
        .collect();
    assert_eq!(seqs, vec!["1", "2", "3"]);
    assert_eq!(store.latest_event_seq(&session.id).unwrap(), 3);
}

#[test]
fn messages_after_the_rewind_point_leave_the_active_conversation() {
    let data_dir = temp_data_dir("messages");
    let store = SessionStore::new(&data_dir);
    let session = store.create_session("repo_1", "Rewound").unwrap();
    store
        .append_message(&session.id, None, "user", "keep me")
        .unwrap();
    let rewind_to = store.latest_event_seq(&session.id).unwrap();
    store
        .append_message(&session.id, None, "assistant", "wrong direction")
        .unwrap();
    store
        .append_message(&session.id, None, "user", "still wrong")
        .unwrap();
    let lines_before = session_log(&data_dir, &session.id).lines().count();

    store.rewind_conversation(&session.id, rewind_to).unwrap();

    let messages = store.read_messages(&session.id).unwrap();
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].content, "keep me");
    // The rewind is an append, so the superseded events are still on disk and
    // still auditable — one more line than before, not fewer.
    assert_eq!(
        session_log(&data_dir, &session.id).lines().count(),
        lines_before + 1
    );
    assert!(session_log(&data_dir, &session.id).contains("still wrong"));
}

#[test]
fn a_second_rewind_to_an_earlier_point_supersedes_the_first() {
    let data_dir = temp_data_dir("supersede");
    let store = SessionStore::new(&data_dir);
    let session = store.create_session("repo_1", "Twice rewound").unwrap();
    store
        .append_message(&session.id, None, "user", "first")
        .unwrap();
    let earlier = store.latest_event_seq(&session.id).unwrap();
    store
        .append_message(&session.id, None, "user", "second")
        .unwrap();
    let later = store.latest_event_seq(&session.id).unwrap();
    store
        .append_message(&session.id, None, "user", "third")
        .unwrap();

    store.rewind_conversation(&session.id, later).unwrap();
    store.rewind_conversation(&session.id, earlier).unwrap();

    let messages = store.read_messages(&session.id).unwrap();
    assert_eq!(
        messages
            .iter()
            .map(|message| message.content.as_str())
            .collect::<Vec<_>>(),
        vec!["first"]
    );
}

#[test]
fn task_status_updates_after_the_rewind_point_are_inert() {
    let data_dir = temp_data_dir("statuses");
    let store = SessionStore::new(&data_dir);
    let session = store.create_session("repo_1", "Task rewind").unwrap();
    let task = store
        .create_task(&session.id, "do the thing", "mock", "m")
        .unwrap();
    let task = store
        .update_task_status(&task, TaskStatus::Running, None)
        .unwrap();
    let rewind_to = store.latest_event_seq(&session.id).unwrap();
    store
        .update_task_status(&task, TaskStatus::Failed, None)
        .unwrap();

    store.rewind_conversation(&session.id, rewind_to).unwrap();

    let statuses = store.read_task_statuses(&session.id).unwrap();
    assert_eq!(statuses.get(&task.id).map(String::as_str), Some("running"));
}

// Every session written before this change has no `seq` field. Numbering those
// events by line order is exactly their append order, so they need no rewrite —
// but a reader that got this wrong would rewind a pre-existing session to the
// wrong place, which is worse than not offering rewind at all.
#[test]
fn sessions_written_before_the_seq_field_are_numbered_by_line_order() {
    let data_dir = temp_data_dir("legacy");
    let store = SessionStore::new(&data_dir);
    let sessions_dir = data_dir.join("sessions");
    fs::create_dir_all(&sessions_dir).unwrap();
    let legacy = [
        r#"{"eventId":"evt_1","timestampMs":1,"eventType":"session_created","payload":{"id":"session_legacy","repositoryId":"repo_1","title":"Legacy","createdAtMs":1,"updatedAtMs":1,"summary":""}}"#,
        r#"{"eventId":"evt_2","timestampMs":2,"eventType":"message_appended","payload":{"id":"msg_1","sessionId":"session_legacy","taskId":null,"role":"user","content":"kept","createdAtMs":2}}"#,
        r#"{"eventId":"evt_3","timestampMs":3,"eventType":"message_appended","payload":{"id":"msg_2","sessionId":"session_legacy","taskId":null,"role":"assistant","content":"dropped","createdAtMs":3}}"#,
    ]
    .join("\n");
    fs::write(
        sessions_dir.join("session_legacy.jsonl"),
        format!("{legacy}\n"),
    )
    .unwrap();

    assert_eq!(store.latest_event_seq("session_legacy").unwrap(), 3);
    store.rewind_conversation("session_legacy", 2).unwrap();

    let messages = store.read_messages("session_legacy").unwrap();
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].content, "kept");
    // The marker appended after three unnumbered events is event four.
    assert!(session_log(&data_dir, "session_legacy").contains("\"seq\":4"));
}
