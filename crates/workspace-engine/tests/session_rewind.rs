//! Conversation rewind over the append-only session log, per
//! `docs/specs/16_session_checkpoints_and_rewind.md` §5.5.
//!
//! Rewind must not truncate the log: tasks are replayed from it and it is the
//! audit trail. So it appends a `conversation_rewound` marker and every reader
//! treats the events between the rewound-to point and the marker as inert.
//! These tests hold that line — the log keeps every byte, and the active
//! conversation moves back, then continues from the marker.

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
fn messages_appended_after_the_rewind_marker_are_part_of_the_conversation() {
    // §5.5: the marker removes the events between the rewound-to point and
    // itself. Everything appended after it is the conversation continuing, and
    // a restore that hid those too would make the session unusable from the
    // next turn on.
    let data_dir = temp_data_dir("continue");
    let store = SessionStore::new(&data_dir);
    let session = store.create_session("repo_1", "Continue").unwrap();
    store
        .append_message(&session.id, None, "user", "keep me")
        .unwrap();
    let rewind_to = store.latest_event_seq(&session.id).unwrap();
    store
        .append_message(&session.id, None, "assistant", "wrong direction")
        .unwrap();
    store.rewind_conversation(&session.id, rewind_to).unwrap();
    store
        .append_message(&session.id, None, "user", "second try")
        .unwrap();

    let messages = store.read_messages(&session.id).unwrap();
    assert_eq!(
        messages
            .iter()
            .map(|message| message.content.as_str())
            .collect::<Vec<_>>(),
        vec!["keep me", "second try"]
    );
    assert!(session_log(&data_dir, &session.id).contains("wrong direction"));
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
        .update_task_status(&task, TaskStatus::PreparingContext, None)
        .unwrap();
    let rewind_to = store.latest_event_seq(&session.id).unwrap();
    store
        .update_task_status(&task, TaskStatus::Failed, None)
        .unwrap();

    store.rewind_conversation(&session.id, rewind_to).unwrap();

    let statuses = store.read_task_statuses(&session.id).unwrap();
    assert_eq!(
        statuses.get(&task.id).map(String::as_str),
        Some("preparing_context")
    );
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

fn session_log_path(data_dir: &Path, session_id: &str) -> PathBuf {
    data_dir
        .join("sessions")
        .join(format!("{session_id}.jsonl"))
}

/// Requirement 3 of `docs/specs/17_durable_task_state_and_crash_recovery/`: a
/// partially written event is never readable as a valid state.
///
/// The torn line here carries *every* message field and is missing only its
/// closing braces — which is what a crash mid-`writeln!` actually leaves. That
/// shape matters: substring field extraction finds all six fields and yields a
/// bogus third message, so this test distinguishes parse-first reading from
/// `line.contains` plus slicing, rather than passing by luck on a line that
/// happens to be unparsable either way.
#[test]
fn a_torn_final_line_is_discarded_and_the_rest_replays() {
    let data_dir = temp_data_dir("torn-tail");
    let store = SessionStore::new(&data_dir);
    let session = store.create_session("repo_1", "Torn").unwrap();
    store
        .append_message(&session.id, None, "user", "first")
        .unwrap();
    store
        .append_message(&session.id, None, "assistant", "second")
        .unwrap();

    let path = session_log_path(&data_dir, &session.id);
    let mut torn = fs::read_to_string(&path).unwrap();
    // The tail is cut *after* a trailing field, so dropping the final character
    // — which is what the substring reader does before scanning — still leaves
    // all six message fields findable. Without that, the old reader rejects this
    // line by accident rather than by parsing, and the test proves nothing.
    torn.push_str(&format!(
        "{{\"eventId\":\"evt_torn\",\"seq\":99,\"timestampMs\":1,\
          \"eventType\":\"message_appended\",\"payload\":{{\"id\":\"msg_torn\",\
          \"sessionId\":\"{}\",\"taskId\":null,\"role\":\"user\",\
          \"content\":\"torn\",\"createdAtMs\":7,\"trailing\":\"cut",
        session.id
    ));
    fs::write(&path, &torn).unwrap();

    let messages = store.read_messages(&session.id).expect("log should read");
    assert_eq!(
        messages.len(),
        2,
        "the torn tail must be discarded, not parsed into a message; got {:?}",
        messages
            .iter()
            .map(|m| m.content.clone())
            .collect::<Vec<_>>()
    );
    assert!(
        messages.iter().all(|message| message.content != "torn"),
        "no message may come from the torn line"
    );

    let after = fs::read_to_string(&path).unwrap();
    assert!(
        after.contains("msg_torn"),
        "reading must not rewrite the log: the torn bytes stay on disk"
    );
}

/// Events written before spec 16 added `seq` are numbered by line order. A
/// parse-first reader must keep accepting them, or every session that predates
/// that field becomes unreadable.
#[test]
fn an_event_without_a_seq_is_still_readable() {
    let data_dir = temp_data_dir("no-seq");
    let store = SessionStore::new(&data_dir);
    let session = store.create_session("repo_1", "Legacy").unwrap();
    store
        .append_message(&session.id, None, "user", "only")
        .unwrap();

    let path = session_log_path(&data_dir, &session.id);
    let content = fs::read_to_string(&path).unwrap();
    let stripped: String = content
        .lines()
        .map(|line| {
            let start = line.find("\"seq\":").expect("seq should be present");
            let end = line[start..]
                .find(',')
                .map(|offset| start + offset + 1)
                .expect("seq should be followed by a comma");
            format!("{}{}", &line[..start], &line[end..])
        })
        .collect::<Vec<_>>()
        .join("\n");
    fs::write(&path, format!("{stripped}\n")).unwrap();

    assert_eq!(
        store.read_messages(&session.id).expect("read").len(),
        1,
        "an event with no seq must still replay"
    );
}

/// The torn tail is evidence of a crash, so the count must be *reportable* even
/// though `SessionStore` does not audit it itself — the recovery classifier
/// does, and it needs a number to record.
#[test]
fn a_torn_tail_is_reported_as_an_unreadable_event() {
    let data_dir = temp_data_dir("torn-count");
    let store = SessionStore::new(&data_dir);
    let session = store.create_session("repo_1", "Counted").unwrap();
    store
        .append_message(&session.id, None, "user", "first")
        .unwrap();

    assert_eq!(
        store.unreadable_event_count(&session.id).unwrap(),
        0,
        "an intact log has nothing unreadable"
    );

    let path = session_log_path(&data_dir, &session.id);
    let mut torn = fs::read_to_string(&path).unwrap();
    torn.push_str("{\"eventId\":\"evt_torn\",\"eventType\":\"message_app");
    fs::write(&path, &torn).unwrap();

    assert_eq!(
        store.unreadable_event_count(&session.id).unwrap(),
        1,
        "the torn tail must be counted"
    );
}

/// Append cost, measured rather than assumed. `#[ignore]`d because it is a
/// timing probe, not an assertion — per `AGENTS.md`, run it by hand:
///
/// ```text
/// cargo test -p workspace-engine --locked -- --ignored append_cost --nocapture
/// ```
///
/// Spec 17 adds two marker events per action across six action types, so this
/// is the figure that decides whether the session log can carry them.
#[test]
#[ignore = "timing probe, not an assertion"]
fn append_cost_over_a_long_session() {
    let data_dir = temp_data_dir("append-cost");
    let store = SessionStore::new(&data_dir);
    let session = store.create_session("repo_1", "Long").unwrap();

    for batch in [500usize, 1000, 2000] {
        let dir = temp_data_dir(&format!("append-cost-{batch}"));
        let store = SessionStore::new(&dir);
        let session = store.create_session("repo_1", "Long").unwrap();
        let start = std::time::Instant::now();
        for index in 0..batch {
            store
                .append_message(&session.id, None, "user", &format!("event {index}"))
                .unwrap();
        }
        let elapsed = start.elapsed();
        println!(
            "  {batch:>5} appends: {:>8.0}ms  ({:.3}ms/append)",
            elapsed.as_secs_f64() * 1000.0,
            elapsed.as_secs_f64() * 1000.0 / batch as f64
        );
    }
    let _ = (store, session);
}

fn recorded_seqs(data_dir: &Path, session_id: &str) -> Vec<u64> {
    session_log(data_dir, session_id)
        .lines()
        .filter_map(|line| {
            line.split("\"seq\":")
                .nth(1)?
                .split(',')
                .next()?
                .parse::<u64>()
                .ok()
        })
        .collect()
}

/// Computing the next `seq` must not require re-reading the whole log. Two
/// independently constructed stores over the same directory must still agree,
/// which is what the lock sidecar's reserved counter provides.
#[test]
fn a_second_store_over_the_same_session_continues_the_sequence() {
    let data_dir = temp_data_dir("seq-sidecar-two-stores");
    let first = SessionStore::new(&data_dir);
    let session = first.create_session("repo_1", "Shared").unwrap();
    first
        .append_message(&session.id, None, "user", "one")
        .unwrap();

    let second = SessionStore::new(&data_dir);
    second
        .append_message(&session.id, None, "user", "two")
        .unwrap();

    let seqs = recorded_seqs(&data_dir, &session.id);
    assert!(
        seqs.windows(2).all(|pair| pair[1] > pair[0]),
        "sequence numbers must stay strictly increasing across stores, got {seqs:?}"
    );
}

/// Two real processes, not two threads: the per-session lock and its reserved
/// sequence counter must keep one shared log strictly increasing and return the
/// same worker task exactly once.
#[test]
fn two_processes_append_and_return_one_worker_result() {
    use std::process::Command;

    let data_dir = temp_data_dir("cross-process");
    let store = SessionStore::new(&data_dir);
    let parent = store.create_session("repo_1", "Planner").unwrap().id;
    let go = data_dir.join("go");

    let spawn = |tag: &str| {
        Command::new(std::env::current_exe().expect("test binary path"))
            .args([
                "--ignored",
                "--exact",
                "cross_process_append_and_return_helper",
            ])
            .env("DAMAIAN_XPROC_DIR", &data_dir)
            .env("DAMAIAN_XPROC_PARENT", &parent)
            .env("DAMAIAN_XPROC_TAG", tag)
            .env("DAMAIAN_XPROC_GO", &go)
            .spawn()
            .expect("child test process should spawn")
    };
    let mut first = spawn("first");
    let mut second = spawn("second");
    fs::write(&go, b"go").unwrap();

    let first_status = first.wait().expect("first child should be reaped");
    let second_status = second.wait().expect("second child should be reaped");
    assert!(first_status.success(), "{first_status:?}");
    assert!(second_status.success(), "{second_status:?}");

    let seqs = recorded_seqs(&data_dir, &parent);
    let unique: std::collections::HashSet<u64> = seqs.iter().copied().collect();
    assert_eq!(unique.len(), seqs.len(), "duplicate seq: {seqs:?}");
    assert!(
        seqs.windows(2).all(|pair| pair[1] > pair[0]),
        "sequence numbers must strictly increase: {seqs:?}"
    );
    let raw = session_log(&data_dir, &parent);
    assert_eq!(
        raw.matches("\"eventType\":\"worker_returned\"").count(),
        1,
        "two processes must return the same task exactly once"
    );
    assert_eq!(store.pending_worker_returns(&parent).unwrap().len(), 1);
    let _ = fs::remove_dir_all(data_dir);
}

/// Not a test on its own: the child half of the cross-process test above,
/// re-executed with a shared data directory, session and start file.
///
/// Run it by hand only through the parent test:
///
/// ```text
/// cargo test -p workspace-engine --test session_rewind -- --exact \
///   two_processes_append_and_return_one_worker_result
/// ```
#[test]
#[ignore = "the child half of the cross-process append test"]
fn cross_process_append_and_return_helper() {
    use workspace_engine::{SecretScanner, WorkerReturn};

    let Ok(data_dir) = std::env::var("DAMAIAN_XPROC_DIR") else {
        return;
    };
    let parent = std::env::var("DAMAIAN_XPROC_PARENT").unwrap();
    let tag = std::env::var("DAMAIAN_XPROC_TAG").unwrap();
    let go = PathBuf::from(std::env::var("DAMAIAN_XPROC_GO").unwrap());
    while !go.exists() {
        std::thread::sleep(std::time::Duration::from_millis(5));
    }

    let store = SessionStore::new(data_dir);
    for index in 0..30 {
        store
            .append_message(&parent, None, "user", &format!("{tag} append {index}"))
            .unwrap();
    }
    let returned = WorkerReturn::new(
        &parent,
        "ses_worker",
        "task_worker",
        "damaian/worker",
        "/code/app/.damaian/worktrees/worker",
        "complete",
        "Finished in a child process.",
    );
    store
        .record_worker_return(&returned, &SecretScanner::default())
        .unwrap();
}

/// `SessionStore` derives `Clone` and is cloned into both the chat and the edit
/// orchestrator (`workspace_engine.rs:97`, `:112`). Each clone must draw from
/// the same per-session reservation, or both would hand out the same next `seq`
/// and the log would contain duplicates — which rewind resolves by sequence
/// number, so it would rewind to the wrong place.
#[test]
fn clones_of_one_store_never_hand_out_a_duplicate_sequence_number() {
    let data_dir = temp_data_dir("seq-sidecar-clones");
    let store = SessionStore::new(&data_dir);
    let session = store.create_session("repo_1", "Cloned").unwrap();

    let chat_side = store.clone();
    let edit_side = store.clone();
    for round in 0..5 {
        chat_side
            .append_message(&session.id, None, "user", &format!("chat {round}"))
            .unwrap();
        edit_side
            .append_message(&session.id, None, "assistant", &format!("edit {round}"))
            .unwrap();
    }

    let seqs = recorded_seqs(&data_dir, &session.id);
    let mut unique = seqs.clone();
    unique.sort_unstable();
    unique.dedup();
    assert_eq!(
        unique.len(),
        seqs.len(),
        "clones must share one sequence, got duplicates in {seqs:?}"
    );
    assert!(
        seqs.windows(2).all(|pair| pair[1] > pair[0]),
        "sequence numbers must stay strictly increasing, got {seqs:?}"
    );
}

/// Every state. Enumerated via `all()` rather than listed, so a variant added
/// later fails these tests until it is given a string form, a terminality, and
/// a side-effect answer — which is the mechanism that stops the state machine
/// drifting away from the spec's table.
#[test]
fn every_state_round_trips_through_its_string_form() {
    for status in TaskStatus::all() {
        assert_eq!(
            TaskStatus::parse(status.as_str()),
            Some(status.clone()),
            "{} must round-trip",
            status.as_str()
        );
    }
}

/// §5.1: a crash in these states may have left something half-done. A crash in
/// any other state cannot have, which is what makes auto-resume safe there.
#[test]
fn the_states_that_can_leave_a_side_effect_in_flight_are_exactly_these() {
    let flagged: Vec<&str> = TaskStatus::all()
        .iter()
        .filter(|status| status.may_have_side_effect_in_flight())
        .map(|status| status.as_str())
        .collect();
    assert_eq!(
        flagged,
        vec!["running_tool", "applying_patch", "validating"]
    );
}

#[test]
fn terminal_states_are_exactly_these() {
    let terminal: Vec<&str> = TaskStatus::all()
        .iter()
        .filter(|status| status.is_terminal())
        .map(|status| status.as_str())
        .collect();
    assert_eq!(
        terminal,
        vec![
            "complete",
            // Spec 101. Terminal for this run, but not successful: the work
            // is still open, so a later Continue task resumes it.
            "incomplete",
            "failed",
            "cancelled",
            "tool_budget_exhausted",
            // Spec 21 §5.4. Beside the round budget rather than folded into
            // it: one means the work needed more rounds, the other that it
            // needed more money, and the remedies differ.
            "token_budget_exhausted",
            // Spec 103 Task 6. Distinct non-success stops at the approved
            // task bound: one names which dimension ran out, one the request
            // that could not fit, one repeated progress.
            "task_budget_exhausted",
            "task_compaction_cannot_fit",
            "task_no_progress"
        ]
    );
}

/// §5.6: `running` is a legacy value. A task left in it carries exactly the
/// information this spec exists to eliminate — something was in flight and
/// nothing recorded what — so it reads back as `interrupted`.
#[test]
fn the_legacy_running_status_parses_as_interrupted() {
    assert_eq!(
        TaskStatus::parse("running"),
        Some(TaskStatus::Interrupted),
        "a session written before this change must still classify"
    );
    assert!(
        !TaskStatus::all()
            .iter()
            .any(|status| status.as_str() == "running"),
        "`running` must no longer be a state anything can be set to"
    );
}

#[test]
fn an_unknown_status_string_does_not_parse() {
    assert_eq!(TaskStatus::parse("nonsense"), None);
}

fn store_with_task(name: &str) -> (PathBuf, SessionStore, workspace_engine::Task) {
    let data_dir = temp_data_dir(name);
    let store = SessionStore::new(&data_dir);
    let session = store.create_session("repo_1", "Marked").unwrap();
    let task = store
        .create_task(&session.id, "do the thing", "mock", "m")
        .unwrap();
    (data_dir, store, task)
}

#[test]
fn a_finished_action_leaves_no_dangling_marker() {
    let (_dir, store, task) = store_with_task("marker-finished");
    let marker = store
        .start_action(&task, "apply_patch", "patch_1", true)
        .unwrap();
    store.finish_action(marker, "ok").unwrap();

    assert!(store.dangling_actions(&task.session_id).unwrap().is_empty());
}

/// The crash signature: a start with no matching finish. `sideEffecting` is
/// recorded on the *start* event so the classifier never has to re-derive the
/// action's nature after the code path that knew it is gone.
#[test]
fn an_unfinished_action_is_reported_as_dangling_with_its_side_effect_flag() {
    let (_dir, store, task) = store_with_task("marker-dangling");
    // Never finished — stands in for the process dying here. `ActionMarker` has
    // no `Drop`, deliberately: an automatic finish-on-drop would erase exactly
    // the signal this spec exists to detect.
    let _marker = store
        .start_action(&task, "apply_patch", "patch_1", true)
        .unwrap();

    let dangling = store.dangling_actions(&task.session_id).unwrap();
    assert_eq!(dangling.len(), 1);
    assert_eq!(dangling[0].action, "apply_patch");
    assert_eq!(dangling[0].reference, "patch_1");
    assert_eq!(dangling[0].task_id, task.id);
    assert!(dangling[0].side_effecting);
}

/// Pairing must be by marker id, not by action name: the same action can run
/// twice in one turn, and matching by name would let the second start cancel
/// out the first.
#[test]
fn the_same_action_running_twice_pairs_by_identity_not_by_name() {
    let (_dir, store, task) = store_with_task("marker-twice");
    let first = store
        .start_action(&task, "run_command", "cargo test", true)
        .unwrap();
    store.finish_action(first, "ok").unwrap();
    let _second = store
        .start_action(&task, "run_command", "cargo build", true)
        .unwrap();

    let dangling = store.dangling_actions(&task.session_id).unwrap();
    assert_eq!(
        dangling.len(),
        1,
        "the second run is dangling; the first must not be resurrected by name"
    );
    assert_eq!(dangling[0].reference, "cargo build");
}

/// A rewind moves the *conversation* back. Whether an action completed is a
/// fact about the world, not about the conversation — so a dangling
/// side-effecting action must stay visible to the classifier even when the
/// conversation has been rewound past it. Reading only the active events here
/// would let a rewind hide precisely the thing requirement 5 protects against.
#[test]
fn a_rewind_does_not_hide_a_dangling_action() {
    let (_dir, store, task) = store_with_task("marker-rewound");
    let before = store.latest_event_seq(&task.session_id).unwrap();
    let _marker = store
        .start_action(&task, "run_command", "rm -rf build", true)
        .unwrap();

    store.rewind_conversation(&task.session_id, before).unwrap();

    let dangling = store.dangling_actions(&task.session_id).unwrap();
    assert_eq!(
        dangling.len(),
        1,
        "a rewind must not conceal an action whose outcome is unknown"
    );
}

/// A patch stored by the previous version carries no `SESSION_ID`. It must
/// still load, with an empty session, rather than failing — there is no
/// conversion step and no file is rewritten. The format's `..._V1` header line
/// exists for exactly this.
#[test]
fn a_v1_stored_patch_still_loads_with_an_empty_session() {
    let data_dir = temp_data_dir("patch-v1");
    let store = workspace_engine::PatchStore::new(&data_dir);
    let legacy = concat!(
        "DAMAIAN_STORED_PATCH_V1\n",
        "PATCH_ID 12\npatch_legacy\n",
        "TASK_ID 11\ntask_legacy\n",
        "SUMMARY 6\nlegacy\n",
        "STATUS 7\npending\n",
        "CREATED_AT_MS 1\n1\n",
        "FILE_COUNT 1\n1\n",
        "FILE\n",
        "PATH 9\nsrc/x.rs\n",
        "FILE_STATUS 8\nmodified\n",
        "BASE_HASH 0\n\n",
        "NEW_HASH 4\nabcd\n",
        "NEW_CONTENT 3\nhi\n\n",
        "DIFF 0\n\n",
        "HUNKS 2\n[]\n",
        "END_FILE\n",
    );
    let patches = data_dir.join("patches").join("pending");
    fs::create_dir_all(&patches).unwrap();
    fs::write(patches.join("patch_legacy.dpatch"), legacy).unwrap();

    let loaded = store
        .load("patch_legacy")
        .expect("a V1 patch must still load");
    assert_eq!(loaded.id, "patch_legacy");
    assert_eq!(
        loaded.session_id, "",
        "a legacy patch has no session, and that is not an error"
    );
}

/// A patch saved now carries its session, so its marker and its recovery
/// reattachment have a log to name.
#[test]
fn a_patch_saved_now_round_trips_its_session() {
    let data_dir = temp_data_dir("patch-v2");
    let store = workspace_engine::PatchStore::new(&data_dir);
    let patch = workspace_engine::ProposedPatch {
        id: "patch_v2".to_string(),
        session_id: "session_abc".to_string(),
        origin_session_id: "session_chat".to_string(),
        task_id: Some("task_abc".to_string()),
        summary: "with a session".to_string(),
        status: "pending".to_string(),
        created_at_ms: 7,
        files: Vec::new(),
        repair_scope: None,
    };
    store.save(&patch).expect("save");

    let loaded = store.load("patch_v2").expect("load");
    assert_eq!(loaded.session_id, "session_abc");
    assert_eq!(loaded, patch, "the whole patch must round-trip unchanged");

    let raw = fs::read_to_string(
        data_dir
            .join("patches")
            .join("pending")
            .join("patch_v2.dpatch"),
    )
    .unwrap();
    assert!(
        raw.starts_with("DAMAIAN_STORED_PATCH_V4\n"),
        "new patches are written as V4"
    );
}

/// V4 added `REPAIR_SCOPE`. A V3 patch already on disk keeps its session and
/// origin and simply has no repair-scope review, which is what an ordinary
/// patch has now.
#[test]
fn a_v3_stored_patch_still_loads_without_a_repair_scope() {
    let data_dir = temp_data_dir("patch-v3-legacy");
    let store = workspace_engine::PatchStore::new(&data_dir);
    let v3 = concat!(
        "DAMAIAN_STORED_PATCH_V3\n",
        "PATCH_ID 8\npatch_v3\n",
        "SESSION_ID 11\nsession_abc\n",
        "ORIGIN_SESSION_ID 12\nsession_chat\n",
        "TASK_ID 0\n\n",
        "SUMMARY 2\nv3\n",
        "STATUS 7\npending\n",
        "CREATED_AT_MS 1\n1\n",
        "FILE_COUNT 1\n0\n",
        "END_PATCH\n",
    );
    let patches = data_dir.join("patches").join("pending");
    fs::create_dir_all(&patches).unwrap();
    fs::write(patches.join("patch_v3.dpatch"), v3).unwrap();

    let loaded = store.load("patch_v3").expect("a V3 patch must still load");
    assert_eq!(loaded.session_id, "session_abc");
    assert_eq!(loaded.origin_session_id, "session_chat");
    assert!(
        loaded.repair_scope.is_none(),
        "a V3 patch has no scope review"
    );
}

/// The typed repair-scope review survives the stored-patch format, so a
/// reloaded or reattached card shows the same expansion fact (spec 102 Task 5).
#[test]
fn a_v4_patch_round_trips_its_repair_scope() {
    let data_dir = temp_data_dir("patch-v4-scope");
    let store = workspace_engine::PatchStore::new(&data_dir);
    let review = workspace_engine::RepairScopeReview::compute(
        &["src/a.rs".to_string()],
        &["src/b.rs".to_string()],
        &["src/new.rs".to_string()],
    );
    let patch = workspace_engine::ProposedPatch {
        id: "patch_v4".to_string(),
        session_id: "session_abc".to_string(),
        origin_session_id: String::new(),
        task_id: Some("task_abc".to_string()),
        summary: "repair".to_string(),
        status: "pending".to_string(),
        created_at_ms: 7,
        files: Vec::new(),
        repair_scope: Some(review),
    };
    store.save(&patch).expect("save");

    let loaded = store.load("patch_v4").expect("load");
    assert_eq!(loaded, patch, "the scope review must round-trip unchanged");
    assert_eq!(
        loaded
            .repair_scope
            .as_ref()
            .expect("the review is stored")
            .expansion_paths,
        vec!["src/new.rs".to_string()]
    );
}

/// V3 added `ORIGIN_SESSION_ID`. A V2 patch already on disk keeps its session
/// and simply has no origin, which apply treats as nothing extra to check.
#[test]
fn a_v2_stored_patch_still_loads_with_an_empty_origin() {
    let data_dir = temp_data_dir("patch-v2-legacy");
    let store = workspace_engine::PatchStore::new(&data_dir);
    let v2 = concat!(
        "DAMAIAN_STORED_PATCH_V2\n",
        "PATCH_ID 8\npatch_v2\n",
        "SESSION_ID 11\nsession_abc\n",
        "TASK_ID 0\n\n",
        "SUMMARY 2\nv2\n",
        "STATUS 7\npending\n",
        "CREATED_AT_MS 1\n1\n",
        "FILE_COUNT 1\n0\n",
        "END_PATCH\n",
    );
    let patches = data_dir.join("patches").join("pending");
    fs::create_dir_all(&patches).unwrap();
    fs::write(patches.join("patch_v2.dpatch"), v2).unwrap();

    let loaded = store.load("patch_v2").expect("a V2 patch must still load");
    assert_eq!(loaded.session_id, "session_abc");
    assert_eq!(loaded.origin_session_id, "", "a V2 patch has no origin");
}
