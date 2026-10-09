//! Pause and steering notes at the round boundary, per
//! `docs/specs/57_worker_sessions/proposal.md` §5.4.
//!
//! Every test drives a three-round scripted turn and steers it from the
//! progress callback or from a second thread. Timing is gated on progress
//! events, never on sleeps: the callback runs on the turn's own thread at
//! known points, and a paused turn reports `Paused` before it blocks.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, mpsc};
use std::thread;
use std::time::{SystemTime, UNIX_EPOCH};

use workspace_engine::{
    CancelToken, ChatTurnResult, Config, MockModelAdapter, PhaseKind, TaskStatus, TurnProgress,
    TurnSink, TurnSteering, WorkspaceEngine,
};

static COUNTER: AtomicU64 = AtomicU64::new(1);

fn temp_repo(name: &str) -> PathBuf {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock should work")
        .as_nanos();
    let repo = std::env::temp_dir().join(format!(
        "damaian-steering-{name}-{now}-{}",
        COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir_all(repo.join("src")).expect("repository should be created");
    fs::write(repo.join("src/a.rs"), "fn main() {}\n").expect("a file to read");
    repo
}

fn engine(repo: &Path) -> WorkspaceEngine {
    WorkspaceEngine::new(Config {
        data_dir: repo.join(".damaian"),
        // Throwaway repository: a watcher would only cost FSEvents registration.
        enable_index_watcher: false,
        ..Config::default()
    })
}

/// Two rounds that each run a sandbox-safe command, then an answer: three
/// model calls, so there are two round boundaries to steer at.
fn three_rounds() -> MockModelAdapter {
    let command = "DAMAIAN_COMMAND_V1\nCOMMAND: ls src\nREASON: Look.\nEND_COMMAND\n".to_string();
    MockModelAdapter::new_sequence_with_tool_calls(
        vec![command.clone(), command, "Done looking.".to_string()],
        vec![Vec::new(), Vec::new(), Vec::new()],
    )
}

fn is_model_phase(event: &TurnProgress, round: u32) -> bool {
    matches!(event, TurnProgress::Phase(phase) if phase.kind == PhaseKind::Model && phase.round == round)
}

/// Runs one turn. `react` sees every progress event on the turn's thread,
/// after it has been recorded, and returns every event in order.
fn run(
    engine: &WorkspaceEngine,
    repo: &Path,
    adapter: &mut MockModelAdapter,
    steering: Option<&TurnSteering>,
    cancel: &CancelToken,
    mut react: impl FnMut(&TurnProgress),
) -> (ChatTurnResult, Vec<TurnProgress>) {
    let mut events = Vec::new();
    let mut on_token = |_token: &str| {};
    let mut on_progress = |event: TurnProgress| {
        react(&event);
        events.push(event);
    };
    let mut sink = TurnSink {
        on_token: &mut on_token,
        on_progress: &mut on_progress,
        cancel,
        steering,
    };
    let result = engine
        .chat_orchestrator
        .ask_with_session(repo, "Look at the source", &[], None, adapter, &mut sink)
        .expect("the turn should run");
    (result, events)
}

/// A thread that resumes `steering` once told the turn has paused. The turn
/// is blocked by then, so this is the only way it can continue.
fn resumer(steering: &Arc<TurnSteering>) -> (mpsc::Sender<()>, thread::JoinHandle<()>) {
    let (paused_tx, paused_rx) = mpsc::channel::<()>();
    let steering = Arc::clone(steering);
    let handle = thread::spawn(move || {
        paused_rx.recv().expect("the turn should pause");
        steering.resume();
    });
    (paused_tx, handle)
}

fn position(events: &[TurnProgress], matches: impl Fn(&TurnProgress) -> bool) -> usize {
    events
        .iter()
        .position(matches)
        .unwrap_or_else(|| panic!("event not found in {events:?}"))
}

fn statuses_of(repo: &Path, session_id: &str) -> Vec<(String, String)> {
    let log = fs::read_to_string(repo.join(format!(".damaian/sessions/{session_id}.jsonl")))
        .expect("session log");
    log.lines()
        .map(|line| serde_json::from_str::<serde_json::Value>(line).unwrap())
        .filter(|event| event["eventType"] == "task_status_updated")
        .map(|event| {
            // The task itself, or `{"task": …}` when the update carries extras.
            let task = event["payload"]
                .get("task")
                .unwrap_or(&event["payload"])
                .clone();
            (
                task["id"].as_str().unwrap().to_string(),
                task["status"].as_str().unwrap().to_string(),
            )
        })
        .collect()
}

#[test]
fn pause_takes_effect_only_at_a_round_boundary() {
    let repo = temp_repo("boundary");
    let engine = engine(&repo);
    let steering = Arc::new(TurnSteering::new());
    let (paused_tx, resumer) = resumer(&steering);
    let mut adapter = three_rounds();

    let (_, events) = run(
        &engine,
        &repo,
        &mut adapter,
        Some(steering.as_ref()),
        &CancelToken::new(),
        |event| {
            // Asked for just before round 1's model call goes out.
            if is_model_phase(event, 1) {
                steering.pause();
            }
            if *event == TurnProgress::Paused {
                paused_tx.send(()).unwrap();
            }
        },
    );
    resumer.join().unwrap();

    let first_call = position(&events, |event| is_model_phase(event, 1));
    let paused = position(&events, |event| *event == TurnProgress::Paused);
    let resumed = position(&events, |event| *event == TurnProgress::Resumed);
    let second_call = position(&events, |event| is_model_phase(event, 2));
    assert!(
        first_call < paused,
        "the call in flight when pause was asked for completes first: {events:?}"
    );
    // And the rest of its round with it: a pause between a model call and the
    // tool it asked for would be mid-round, not at the boundary.
    let first_tool = position(
        &events,
        |event| matches!(event, TurnProgress::Phase(phase) if phase.kind == PhaseKind::Tool && phase.round == 1),
    );
    assert!(
        first_tool < paused,
        "round 1's tool runs before the pause holds: {events:?}"
    );
    assert!(
        paused < resumed && resumed < second_call,
        "the next call waits for resume: {events:?}"
    );
    assert!(
        !events[paused..resumed].iter().any(
            |event| matches!(event, TurnProgress::Phase(phase) if phase.kind == PhaseKind::Model)
        ),
        "no model call may start while paused: {events:?}"
    );
    assert_eq!(adapter.requests.len(), 3);
    let _ = fs::remove_dir_all(repo);
}

#[test]
fn a_paused_turn_persists_paused_then_running_again() {
    let repo = temp_repo("persists");
    let engine = engine(&repo);
    let steering = Arc::new(TurnSteering::new());
    let (paused_tx, resumer) = resumer(&steering);
    let mut adapter = three_rounds();

    let (result, _) = run(
        &engine,
        &repo,
        &mut adapter,
        Some(steering.as_ref()),
        &CancelToken::new(),
        |event| {
            if is_model_phase(event, 1) {
                steering.pause();
            }
            if *event == TurnProgress::Paused {
                paused_tx.send(()).unwrap();
            }
        },
    );
    resumer.join().unwrap();

    let statuses: Vec<String> = statuses_of(&repo, &result.session.id)
        .into_iter()
        .map(|(_, status)| status)
        .collect();
    let paused = statuses
        .iter()
        .position(|status| status == "paused")
        .unwrap_or_else(|| panic!("`paused` was never persisted: {statuses:?}"));
    assert_eq!(
        statuses[paused - 1],
        statuses[paused + 1],
        "resume restores the status the turn paused from: {statuses:?}"
    );
    assert_eq!(statuses.last().map(String::as_str), Some("complete"));
    let _ = fs::remove_dir_all(repo);
}

#[test]
fn resume_continues_the_same_turn() {
    let repo = temp_repo("same-turn");
    let engine = engine(&repo);
    let steering = Arc::new(TurnSteering::new());
    let (paused_tx, resumer) = resumer(&steering);
    let mut adapter = three_rounds();

    let (result, events) = run(
        &engine,
        &repo,
        &mut adapter,
        Some(steering.as_ref()),
        &CancelToken::new(),
        |event| {
            if is_model_phase(event, 1) {
                steering.pause();
            }
            if *event == TurnProgress::Paused {
                paused_tx.send(()).unwrap();
            }
        },
    );
    resumer.join().unwrap();

    let task_ids: Vec<String> = statuses_of(&repo, &result.session.id)
        .into_iter()
        .map(|(task_id, _)| task_id)
        .collect();
    assert!(
        task_ids.iter().all(|id| *id == result.task.id),
        "one task from start to finish: {task_ids:?}"
    );
    assert_eq!(result.task.status, TaskStatus::Complete);
    let resumed = position(&events, |event| *event == TurnProgress::Resumed);
    assert!(
        events[resumed..]
            .iter()
            .any(|event| is_model_phase(event, 2)),
        "the round count continues rather than restarting: {events:?}"
    );
    let _ = fs::remove_dir_all(repo);
}

#[test]
fn a_note_becomes_a_user_message_before_the_next_model_request() {
    let repo = temp_repo("note");
    let engine = engine(&repo);
    let steering = Arc::new(TurnSteering::new());
    let note = "Only look at src/a.rs.";
    let mut adapter = three_rounds();

    let (result, events) = run(
        &engine,
        &repo,
        &mut adapter,
        Some(steering.as_ref()),
        &CancelToken::new(),
        |event| {
            if is_model_phase(event, 1) {
                steering.queue_note(note.to_string());
            }
        },
    );

    let carries_note = |n: usize| {
        adapter.requests[n]
            .messages
            .iter()
            .any(|m| m.content == note)
    };
    assert!(
        !carries_note(0),
        "queued during round 1, so not in its request"
    );
    let last = adapter.requests[1].messages.last().unwrap();
    assert_eq!(
        (last.role.as_str(), last.content.as_str()),
        ("user", note),
        "the note is the newest message of the next request"
    );
    assert!(carries_note(2), "and stays in the conversation");

    let message_id = events
        .iter()
        .find_map(|event| match event {
            TurnProgress::NoteAdded { message_id } => Some(message_id.clone()),
            _ => None,
        })
        .expect("a NoteAdded event");
    let stored = engine
        .session_store
        .read_messages(&result.session.id)
        .unwrap()
        .into_iter()
        .find(|message| message.id == message_id)
        .expect("the note is in the session log");
    assert_eq!(
        (stored.role.as_str(), stored.content.as_str()),
        ("user", note)
    );
    let log =
        fs::read_to_string(repo.join(format!(".damaian/sessions/{}.jsonl", result.session.id)))
            .unwrap();
    assert!(
        log.lines()
            .any(|line| line.contains("\"steering_note_added\"")
                && line.contains(&format!("\"messageId\":\"{message_id}\""))),
        "a steering_note_added event names the message"
    );
    let _ = fs::remove_dir_all(repo);
}

/// Correction C3: a note is redacted on its way to the model. The session
/// log keeps it as written, as it keeps every user message.
#[test]
fn a_note_reaches_the_model_redacted() {
    let repo = temp_repo("note-redacted");
    let engine = engine(&repo);
    let steering = Arc::new(TurnSteering::new());
    let secret = "AKIAIOSFODNN7EXAMPLE";
    let mut adapter = three_rounds();

    let (result, _) = run(
        &engine,
        &repo,
        &mut adapter,
        Some(steering.as_ref()),
        &CancelToken::new(),
        |event| {
            if is_model_phase(event, 1) {
                steering.queue_note(format!("Use key {secret} for this."));
            }
        },
    );

    for request in &adapter.requests {
        assert!(
            request.messages.iter().all(|m| !m.content.contains(secret)),
            "the key must not reach the model"
        );
    }
    let notes: Vec<String> = engine
        .session_store
        .read_messages(&result.session.id)
        .unwrap()
        .into_iter()
        .filter(|message| message.content.starts_with("Use key"))
        .map(|message| message.content)
        .collect();
    assert_eq!(notes, vec![format!("Use key {secret} for this.")]);
    let _ = fs::remove_dir_all(repo);
}

#[test]
fn cancel_while_paused_ends_cancelled() {
    let repo = temp_repo("cancel-paused");
    let engine = engine(&repo);
    let steering = Arc::new(TurnSteering::new());
    let cancel = CancelToken::new();
    let (paused_tx, paused_rx) = mpsc::channel::<()>();
    let stopper = {
        let steering = Arc::clone(&steering);
        let cancel = cancel.clone();
        thread::spawn(move || {
            paused_rx.recv().expect("the turn should pause");
            cancel.cancel();
            steering.wake();
        })
    };
    let mut adapter = three_rounds();

    let (result, events) = run(
        &engine,
        &repo,
        &mut adapter,
        Some(steering.as_ref()),
        &cancel,
        |event| {
            if is_model_phase(event, 1) {
                steering.pause();
            }
            if *event == TurnProgress::Paused {
                paused_tx.send(()).unwrap();
            }
        },
    );
    stopper.join().unwrap();

    assert_eq!(result.task.status, TaskStatus::Cancelled);
    assert_eq!(adapter.requests.len(), 1, "nothing ran after the pause");
    assert!(!events.contains(&TurnProgress::Resumed), "{events:?}");
    let _ = fs::remove_dir_all(repo);
}

/// Requirement 4's other half: a host that steers nothing gets exactly the
/// turn it got before. An untouched `TurnSteering` and `None` must agree.
#[test]
fn a_turn_without_steering_is_unchanged() {
    // One repository for both runs, so nothing but `steering` differs.
    let repo = temp_repo("unchanged");
    let engine = engine(&repo);
    let requests = |steering: Option<&TurnSteering>| -> Vec<Vec<(String, String)>> {
        let mut adapter = three_rounds();
        let (result, _) = run(
            &engine,
            &repo,
            &mut adapter,
            steering,
            &CancelToken::new(),
            |_| {},
        );
        assert_eq!(result.task.status, TaskStatus::Complete);
        adapter
            .requests
            .iter()
            .map(|request| {
                request
                    .messages
                    .iter()
                    .map(|m| (m.role.clone(), m.content.clone()))
                    .collect()
            })
            .collect()
    };

    let without = requests(None);
    let untouched = TurnSteering::new();
    let with = requests(Some(&untouched));

    assert_eq!(without.len(), 3);
    assert_eq!(without, with);
    let _ = fs::remove_dir_all(repo);
}
