//! One lane of work per session (spec 110 D1–D3).
//!
//! The shell serves every connection on its own thread, so nothing serialises
//! a session's work any more except this: a session-work request claims its
//! session's lane for as long as it runs, and a second claim while the first
//! holds it is refused at once with the running work's label, never queued.
//! Archive and delete read the same table to refuse a working session, and a
//! UI route lists working sessions and cancels one.
//!
//! The lane lives in the shell process. When the session's work runs in a
//! session host process (spec 110 Task 1b), the lane is marked remote: its
//! label, cancel and notes come from and go to that host (D3, D7), and the
//! host keeps a lane table of its own for the work it runs.
//!
//! A running chat turn also puts its [`NoteQueue`] in the lane (D7), so a
//! message the user sends while the session works reaches the turn at its
//! next round boundary instead of being refused.

use std::cell::RefCell;
use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Instant;

use workspace_engine::{CancelToken, TurnSteering};

#[derive(Debug, Clone)]
struct Lane {
    /// Unique per claim, so a guard released late never frees a later claim.
    claim: u64,
    label: String,
    started: Instant,
    cancel: Option<CancelToken>,
    notes: Option<Arc<NoteQueue>>,
    /// The session host running this work, when it runs in one.
    remote: Option<Arc<dyn RemoteWork>>,
}

impl std::fmt::Debug for dyn RemoteWork {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("RemoteWork")
    }
}

/// Work running in another process (a session host, spec 110 Task 1b). Each
/// call is one short exchange with that process; `None` from `status` means it
/// did not answer, and the lane then shows what it last knew.
pub(crate) trait RemoteWork: Send + Sync {
    /// What the work is doing now and whether it can be cancelled, or `None`.
    fn status(&self) -> Option<RemoteStatus>;
    fn cancel(&self) -> Result<(), CancelRefusal>;
    /// Queues a note into the running turn; `false` when it has none.
    fn note(&self, text: String) -> bool;
}

/// One answer from a host about its session's work.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RemoteStatus {
    pub(crate) label: String,
    pub(crate) cancellable: bool,
}

/// One running turn's steering notes (spec 110 D7, built on spec 57 §5.4).
///
/// The turn drains [`TurnSteering`] at each round boundary; this wrapper adds
/// what the shell needs around it. `close` and `queue` take the same lock, so
/// a note is either queued before the turn ended — and then returned by
/// `close` if no boundary reached it — or refused; it is never accepted and
/// then dropped. `pending` mirrors the queue's texts in order, because the
/// engine announces a delivered note by its message id only and the window
/// shows the text.
#[derive(Debug, Default)]
pub(crate) struct NoteQueue {
    steering: TurnSteering,
    state: Mutex<NoteState>,
}

#[derive(Debug, Default)]
struct NoteState {
    closed: bool,
    pending: VecDeque<String>,
}

impl NoteQueue {
    pub(crate) fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// What the turn is handed as `TurnSink::steering`.
    pub(crate) fn steering(&self) -> &TurnSteering {
        &self.steering
    }

    /// Queues `text` for the next round boundary; `false` once the turn ended.
    pub(crate) fn queue(&self, text: String) -> bool {
        let mut state = self.lock();
        if state.closed {
            return false;
        }
        state.pending.push_back(text.clone());
        self.steering.queue_note(text);
        true
    }

    /// The oldest queued text, now that the turn has delivered it. The engine
    /// drains in queue order, so the front is always the note just added.
    pub(crate) fn delivered(&self) -> Option<String> {
        self.lock().pending.pop_front()
    }

    /// Ends the queue and returns every note no boundary reached, oldest
    /// first. Later `queue` calls are refused.
    pub(crate) fn close(&self) -> Vec<String> {
        let mut state = self.lock();
        state.closed = true;
        state.pending.clear();
        self.steering.drain_notes()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, NoteState> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

#[derive(Default)]
struct Lanes {
    next_claim: u64,
    by_session: HashMap<String, Lane>,
}

static LANES: OnceLock<Mutex<Lanes>> = OnceLock::new();

fn lanes() -> std::sync::MutexGuard<'static, Lanes> {
    LANES
        .get_or_init(|| Mutex::new(Lanes::default()))
        .lock()
        // A panic while holding the table leaves it consistent: every write is
        // one insert or remove.
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// A held lane. Dropping it frees the session for its next piece of work.
#[derive(Debug)]
pub(crate) struct LaneGuard {
    session_id: String,
    claim: u64,
}

impl LaneGuard {
    /// Replaces what the sidebar and a refusal say this session is doing.
    pub(crate) fn set_label(&self, label: &str) {
        if let Some(lane) = lanes().by_session.get_mut(&self.session_id)
            && lane.claim == self.claim
        {
            lane.label = label.to_string();
        }
    }

    /// The token that stops this work, so a UI request can reach it.
    pub(crate) fn set_cancel(&self, cancel: &CancelToken) {
        if let Some(lane) = lanes().by_session.get_mut(&self.session_id)
            && lane.claim == self.claim
        {
            lane.cancel = Some(cancel.clone());
        }
    }

    /// The host process this work runs in: label, cancel and notes go to it.
    pub(crate) fn set_remote(&self, remote: Arc<dyn RemoteWork>) {
        if let Some(lane) = lanes().by_session.get_mut(&self.session_id)
            && lane.claim == self.claim
        {
            lane.remote = Some(remote);
        }
    }

    /// The running turn's note queue, so a UI request can add to it (D7).
    pub(crate) fn set_notes(&self, notes: &Arc<NoteQueue>) {
        if let Some(lane) = lanes().by_session.get_mut(&self.session_id)
            && lane.claim == self.claim
        {
            lane.notes = Some(Arc::clone(notes));
        }
    }
}

impl Drop for LaneGuard {
    fn drop(&mut self) {
        let mut lanes = lanes();
        if lanes
            .by_session
            .get(&self.session_id)
            .is_some_and(|lane| lane.claim == self.claim)
        {
            lanes.by_session.remove(&self.session_id);
        }
    }
}

/// Claims `session_id`'s lane, or returns the label of the work holding it.
pub(crate) fn claim(session_id: &str, label: &str) -> Result<LaneGuard, String> {
    let mut lanes = lanes();
    if let Some(running) = lanes.by_session.get(session_id) {
        let (stored, remote) = (running.label.clone(), running.remote.clone());
        drop(lanes);
        return Err(current_label(stored, remote.as_deref()));
    }
    lanes.next_claim += 1;
    let claim = lanes.next_claim;
    lanes.by_session.insert(
        session_id.to_string(),
        Lane {
            claim,
            label: label.to_string(),
            started: Instant::now(),
            cancel: None,
            notes: None,
            remote: None,
        },
    );
    Ok(LaneGuard {
        session_id: session_id.to_string(),
        claim,
    })
}

/// What `session_id` is doing, or `None` while it is idle.
pub(crate) fn working_label(session_id: &str) -> Option<String> {
    let (stored, remote) = {
        let lanes = lanes();
        let lane = lanes.by_session.get(session_id)?;
        (lane.label.clone(), lane.remote.clone())
    };
    Some(current_label(stored, remote.as_deref()))
}

/// The host's own label for remote work, else the one the lane holds. Never
/// called with the table locked: asking a host is a socket exchange.
fn current_label(stored: String, remote: Option<&dyn RemoteWork>) -> String {
    remote
        .and_then(RemoteWork::status)
        .map(|status| status.label)
        .filter(|label| !label.trim().is_empty())
        .unwrap_or(stored)
}

/// One working session as the UI lists it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Working {
    pub(crate) session_id: String,
    pub(crate) label: String,
    pub(crate) elapsed_ms: u128,
    pub(crate) cancellable: bool,
}

/// Every working session, ordered by id so the answer is stable.
pub(crate) fn working() -> Vec<Working> {
    let rows = lanes()
        .by_session
        .iter()
        .map(|(session_id, lane)| (session_id.clone(), lane.clone()))
        .collect::<Vec<_>>();
    // Hosts are asked with the table unlocked.
    let mut working = rows
        .into_iter()
        .map(|(session_id, lane)| {
            let status = lane.remote.as_deref().and_then(RemoteWork::status);
            Working {
                session_id,
                label: status
                    .as_ref()
                    .map(|status| status.label.clone())
                    .filter(|label| !label.trim().is_empty())
                    .unwrap_or(lane.label),
                elapsed_ms: lane.started.elapsed().as_millis(),
                cancellable: status.map_or(lane.cancel.is_some(), |status| status.cancellable),
            }
        })
        .collect::<Vec<_>>();
    working.sort_by(|left, right| left.session_id.cmp(&right.session_id));
    working
}

/// Why a cancel did nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CancelRefusal {
    Idle,
    /// The work holding the lane has no cancel point (a patch apply, a
    /// rewind): it is short and ends by itself.
    NotCancellable,
}

/// Fires the cancel token of `session_id`'s running work. The work stops at
/// its next safe point, exactly as when the window that started it goes away.
pub(crate) fn cancel(session_id: &str) -> Result<(), CancelRefusal> {
    let lane = lanes()
        .by_session
        .get(session_id)
        .cloned()
        .ok_or(CancelRefusal::Idle)?;
    if let Some(remote) = lane.remote {
        return remote.cancel();
    }
    let cancel = lane.cancel.as_ref().ok_or(CancelRefusal::NotCancellable)?;
    cancel.cancel();
    Ok(())
}

/// Fires every cancel token in this process: a session host shutting down
/// stops its work so the turn ends as stopped, not as a crash (D6).
pub(crate) fn cancel_all() {
    let tokens = lanes()
        .by_session
        .values()
        .filter_map(|lane| lane.cancel.clone())
        .collect::<Vec<_>>();
    for token in tokens {
        token.cancel();
    }
}

/// Queues a note into `session_id`'s running turn (D7). `false` when the
/// session has no running chat turn: it is idle, its work is not a turn (a
/// patch apply), or the turn has just ended. Never touches a paused turn: a
/// turn waiting for an approval or a question holds no lane, so a note can
/// never stand in for an answer.
pub(crate) fn queue_note(session_id: &str, text: String) -> bool {
    let Some((notes, remote)) = lanes()
        .by_session
        .get(session_id)
        .map(|lane| (lane.notes.clone(), lane.remote.clone()))
    else {
        return false;
    };
    if let Some(remote) = remote {
        return remote.note(text);
    }
    notes.is_some_and(|notes| notes.queue(text))
}

/// Notes a paused turn left undelivered, kept beside the paused turns until
/// the session's next turn takes them (D7). On disk rather than in memory, so
/// a pause that outlives the app still delivers them.
fn carried_notes_path(data_dir: &Path, session_id: &str) -> Option<PathBuf> {
    let plain = !session_id.is_empty()
        && session_id.len() <= 256
        && session_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-');
    plain.then(|| {
        data_dir
            .join("chat")
            .join("pending-notes")
            .join(format!("{session_id}.json"))
    })
}

/// Adds `notes` after any already carried for `session_id`.
pub(crate) fn carry_notes(
    data_dir: &Path,
    session_id: &str,
    notes: &[String],
) -> Result<(), String> {
    if notes.is_empty() {
        return Ok(());
    }
    let path = carried_notes_path(data_dir, session_id)
        .ok_or_else(|| format!("not a plain session id: {session_id}"))?;
    let mut all = read_carried(&path);
    all.extend(notes.iter().cloned());
    let parent = path.parent().expect("a carried-notes path has a parent");
    std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    let body = serde_json::to_string(&all).map_err(|error| error.to_string())?;
    // Written whole and renamed, so a crash mid-write leaves the old list.
    let temporary = path.with_extension("json.tmp");
    std::fs::write(&temporary, body).map_err(|error| error.to_string())?;
    std::fs::rename(&temporary, &path).map_err(|error| error.to_string())
}

/// Every note carried for `session_id`, oldest first, removing them.
pub(crate) fn take_carried_notes(data_dir: &Path, session_id: &str) -> Vec<String> {
    let Some(path) = carried_notes_path(data_dir, session_id) else {
        return Vec::new();
    };
    let notes = read_carried(&path);
    if !notes.is_empty() {
        let _ = std::fs::remove_file(&path);
    }
    notes
}

fn read_carried(path: &Path) -> Vec<String> {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|body| serde_json::from_str(&body).ok())
        .unwrap_or_default()
}

thread_local! {
    /// The note queue of the turn running on this thread. Set by the shell's
    /// turn thread for the length of one turn, so every route that runs a
    /// turn hands it to `TurnSink::steering` without each one being passed it.
    static TURN_NOTES: RefCell<Option<Arc<NoteQueue>>> = const { RefCell::new(None) };
}

/// Runs `turn` with `notes` as this thread's running turn's queue.
pub(crate) fn with_turn_notes<T>(notes: &Arc<NoteQueue>, turn: impl FnOnce() -> T) -> T {
    TURN_NOTES.with(|current| *current.borrow_mut() = Some(Arc::clone(notes)));
    let outcome = turn();
    TURN_NOTES.with(|current| *current.borrow_mut() = None);
    outcome
}

/// The note queue for a turn starting on this thread, if the shell's turn
/// thread set one; `None` for a turn run any other way (a test, a
/// non-streaming fallback route), which then takes no notes.
pub(crate) fn turn_notes() -> Option<Arc<NoteQueue>> {
    TURN_NOTES.with(|current| current.borrow().clone())
}

/// The session whose lane this connection holds, if any.
pub(crate) fn current_session() -> Option<String> {
    CURRENT.with(|current| {
        current
            .borrow()
            .as_ref()
            .map(|guard| guard.session_id.clone())
    })
}

thread_local! {
    /// The lane the current connection holds. Each connection has its own
    /// thread (D5), so this is the connection's lane, and the streaming
    /// helpers reach it without every route passing it along.
    static CURRENT: RefCell<Option<LaneGuard>> = const { RefCell::new(None) };
}

/// Holds `guard` for the rest of this connection; dropped by [`release_current`].
pub(crate) fn hold_current(guard: LaneGuard) {
    CURRENT.with(|current| *current.borrow_mut() = Some(guard));
}

/// Frees this connection's lane, if it holds one.
pub(crate) fn release_current() {
    let guard = CURRENT.with(|current| current.borrow_mut().take());
    drop(guard);
}

/// Runs `with` on this connection's lane, if it holds one.
pub(crate) fn with_current(with: impl FnOnce(&LaneGuard)) {
    CURRENT.with(|current| {
        if let Some(guard) = current.borrow().as_ref() {
            with(guard);
        }
    });
}

/// A turn that started without a session (the first message of a new one)
/// learns its id from the turn's first event and claims that lane then, so the
/// new session is listed as working and cannot be archived mid-turn. A new
/// session cannot already be busy; if it somehow is, the turn keeps running
/// unlaned rather than being cut off after it has begun.
pub(crate) fn adopt_current(session_id: &str, label: &str) {
    let held = CURRENT.with(|current| current.borrow().is_some());
    if held {
        return;
    }
    if let Ok(guard) = claim(session_id, label) {
        hold_current(guard);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unique(name: &str) -> String {
        format!(
            "lane_test_{name}_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        )
    }

    #[test]
    fn a_second_claim_is_refused_with_the_running_label_until_the_first_is_dropped() {
        let session = unique("second");
        let first = claim(&session, "Answering a message").unwrap();
        first.set_label("Waiting for the model");
        assert_eq!(
            claim(&session, "Running checks").unwrap_err(),
            "Waiting for the model"
        );
        assert_eq!(
            working_label(&session).as_deref(),
            Some("Waiting for the model")
        );
        drop(first);
        assert_eq!(working_label(&session), None);
        let again = claim(&session, "Running checks").unwrap();
        assert_eq!(working_label(&session).as_deref(), Some("Running checks"));
        drop(again);
    }

    #[test]
    fn other_sessions_claim_independently() {
        let one = unique("one");
        let two = unique("two");
        let _first = claim(&one, "a").unwrap();
        let _second = claim(&two, "b").unwrap();
        let listed = working()
            .into_iter()
            .filter(|row| row.session_id == one || row.session_id == two)
            .count();
        assert_eq!(listed, 2);
    }

    #[test]
    fn cancel_fires_the_token_and_says_why_it_cannot() {
        let session = unique("cancel");
        assert_eq!(cancel(&session), Err(CancelRefusal::Idle));
        let guard = claim(&session, "Applying a patch").unwrap();
        assert_eq!(cancel(&session), Err(CancelRefusal::NotCancellable));
        let token = CancelToken::new();
        guard.set_cancel(&token);
        assert_eq!(cancel(&session), Ok(()));
        assert!(token.is_cancelled());
    }

    #[test]
    fn a_stale_guard_never_frees_a_later_claim() {
        let session = unique("stale");
        let first = claim(&session, "first").unwrap();
        // Simulates a guard whose lane was already taken over: remove it by
        // hand, claim again, then drop the old guard.
        lanes().by_session.remove(&session);
        let second = claim(&session, "second").unwrap();
        drop(first);
        assert_eq!(working_label(&session).as_deref(), Some("second"));
        drop(second);
    }

    #[test]
    fn a_connection_adopts_a_new_sessions_lane_once() {
        let session = unique("adopt");
        let handle = std::thread::spawn(move || {
            adopt_current(&session, "Answering a message");
            let label = working_label(&session);
            adopt_current(&format!("{session}_other"), "x");
            let other = working_label(&format!("{session}_other"));
            release_current();
            (label, other, working_label(&session))
        });
        let (label, other, after) = handle.join().unwrap();
        assert_eq!(label.as_deref(), Some("Answering a message"));
        assert_eq!(other, None, "a connection holds one lane");
        assert_eq!(after, None);
    }

    #[test]
    fn a_note_reaches_only_a_lane_with_a_running_turn_and_never_after_it_ends() {
        let session = unique("note");
        assert!(!queue_note(&session, "idle".to_string()), "no lane");
        let guard = claim(&session, "Applying a patch").unwrap();
        assert!(
            !queue_note(&session, "patch".to_string()),
            "work that is not a turn takes no notes"
        );
        let notes = NoteQueue::new();
        guard.set_notes(&notes);
        assert!(queue_note(&session, "first".to_string()));
        assert!(queue_note(&session, "second".to_string()));
        // The turn delivers the first at a boundary.
        assert_eq!(notes.steering().drain_notes().len(), 2);
        notes.queue("third".to_string());
        assert_eq!(notes.delivered().as_deref(), Some("first"));
        assert_eq!(
            notes.close(),
            vec!["third"],
            "only the undelivered note is left"
        );
        assert!(
            !queue_note(&session, "late".to_string()),
            "a closed turn refuses, so nothing is accepted and dropped"
        );
        drop(guard);
    }

    /// A host that answers with a fixed label and records what reached it.
    #[derive(Default)]
    struct FakeHost {
        silent: bool,
        cancelled: Mutex<bool>,
        notes: Mutex<Vec<String>>,
    }

    impl RemoteWork for FakeHost {
        fn status(&self) -> Option<RemoteStatus> {
            (!self.silent).then(|| RemoteStatus {
                label: "Running `cargo test`".to_string(),
                cancellable: true,
            })
        }

        fn cancel(&self) -> Result<(), CancelRefusal> {
            *self.cancelled.lock().unwrap() = true;
            Ok(())
        }

        fn note(&self, text: String) -> bool {
            self.notes.lock().unwrap().push(text);
            true
        }
    }

    #[test]
    fn a_remote_lanes_label_cancel_and_notes_come_from_its_host() {
        let session = unique("remote");
        let guard = claim(&session, "Answering a message").unwrap();
        let host = Arc::new(FakeHost::default());
        guard.set_remote(host.clone());
        assert_eq!(
            working_label(&session).as_deref(),
            Some("Running `cargo test`")
        );
        assert_eq!(
            claim(&session, "Running checks").unwrap_err(),
            "Running `cargo test`",
            "a refusal names what the host is doing"
        );
        let row = working()
            .into_iter()
            .find(|row| row.session_id == session)
            .unwrap();
        assert!(row.cancellable, "the shell itself holds no token");
        assert!(queue_note(&session, "a note".to_string()));
        assert_eq!(*host.notes.lock().unwrap(), vec!["a note"]);
        assert_eq!(cancel(&session), Ok(()));
        assert!(*host.cancelled.lock().unwrap());
        drop(guard);

        // A host that does not answer leaves the lane's own label.
        let guard = claim(&session, "Answering a message").unwrap();
        guard.set_remote(Arc::new(FakeHost {
            silent: true,
            ..FakeHost::default()
        }));
        assert_eq!(
            working_label(&session).as_deref(),
            Some("Answering a message")
        );
        drop(guard);
    }

    #[test]
    fn carried_notes_accumulate_and_are_taken_once() {
        let data_dir = std::env::temp_dir().join(unique("carried"));
        let session = "session_carried";
        carry_notes(&data_dir, session, &["one".to_string()]).unwrap();
        carry_notes(&data_dir, session, &["two".to_string()]).unwrap();
        assert_eq!(take_carried_notes(&data_dir, session), vec!["one", "two"]);
        assert!(take_carried_notes(&data_dir, session).is_empty());
        assert!(carry_notes(&data_dir, "../escape", &["x".to_string()]).is_err());
        assert!(take_carried_notes(&data_dir, "../escape").is_empty());
        let _ = std::fs::remove_dir_all(&data_dir);
    }
}
