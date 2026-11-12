//! One lane of work per session (spec 110 D1–D3).
//!
//! The shell serves every connection on its own thread, so nothing serialises
//! a session's work any more except this: a session-work request claims its
//! session's lane for as long as it runs, and a second claim while the first
//! holds it is refused at once with the running work's label, never queued.
//! Archive and delete read the same table to refuse a working session, and a
//! UI route lists working sessions and cancels one.
//!
//! The lane lives in the shell process today; spec 110 Task 1b moves the work
//! behind it into a host process per session and keeps this interface: claim,
//! label, cancel, refusal.

use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::Instant;

use workspace_engine::CancelToken;

#[derive(Debug, Clone)]
struct Lane {
    /// Unique per claim, so a guard released late never frees a later claim.
    claim: u64,
    label: String,
    started: Instant,
    cancel: Option<CancelToken>,
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
        return Err(running.label.clone());
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
        },
    );
    Ok(LaneGuard {
        session_id: session_id.to_string(),
        claim,
    })
}

/// What `session_id` is doing, or `None` while it is idle.
pub(crate) fn working_label(session_id: &str) -> Option<String> {
    lanes()
        .by_session
        .get(session_id)
        .map(|lane| lane.label.clone())
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
    let mut working = lanes()
        .by_session
        .iter()
        .map(|(session_id, lane)| Working {
            session_id: session_id.clone(),
            label: lane.label.clone(),
            elapsed_ms: lane.started.elapsed().as_millis(),
            cancellable: lane.cancel.is_some(),
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
    let lanes = lanes();
    let lane = lanes
        .by_session
        .get(session_id)
        .ok_or(CancelRefusal::Idle)?;
    let cancel = lane.cancel.as_ref().ok_or(CancelRefusal::NotCancellable)?;
    cancel.cancel();
    Ok(())
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
}
