//! Pause and steering notes for a running turn (spec 57 §5.4).
//!
//! A turn only ever looks at this between rounds, before the next model
//! request: a pause never interrupts a model call or a tool in flight, and a
//! note joins the conversation as the next user message. Only a worker host
//! hands a turn one; every other host passes `None` and runs the turn exactly
//! as before.

use std::collections::VecDeque;
use std::sync::{Condvar, Mutex, MutexGuard};
use std::time::Duration;

use crate::cancel::CancelToken;

/// How often a paused turn re-checks cancellation on its own. `CancelToken` is
/// a bare flag with no way to wake a waiter, so a missed [`TurnSteering::wake`]
/// must cost at most this long rather than hang the turn.
const RECHECK: Duration = Duration::from_millis(100);

#[derive(Debug, Default)]
struct State {
    paused: bool,
    notes: VecDeque<String>,
}

/// Shared between the turn and whatever controls it from another thread.
#[derive(Debug, Default)]
pub struct TurnSteering {
    state: Mutex<State>,
    changed: Condvar,
}

impl TurnSteering {
    pub fn new() -> Self {
        Self::default()
    }

    /// Takes effect at the next round boundary, not before.
    pub fn pause(&self) {
        self.lock().paused = true;
    }

    pub fn resume(&self) {
        self.lock().paused = false;
        self.changed.notify_all();
    }

    pub fn queue_note(&self, text: String) {
        self.lock().notes.push_back(text);
    }

    pub fn is_paused(&self) -> bool {
        self.lock().paused
    }

    /// Every queued note, oldest first, leaving the queue empty.
    pub fn drain_notes(&self) -> Vec<String> {
        self.lock().notes.drain(..).collect()
    }

    /// Blocks while paused. Returns `false` if `cancel` fired, `true` once
    /// resumed (or immediately, when not paused).
    pub fn wait_while_paused(&self, cancel: &CancelToken) -> bool {
        let mut state = self.lock();
        loop {
            if cancel.is_cancelled() {
                return false;
            }
            if !state.paused {
                return true;
            }
            state = self
                .changed
                .wait_timeout(state, RECHECK)
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .0;
        }
    }

    /// Wakes a waiting turn so it re-checks cancellation now rather than at
    /// the next [`RECHECK`]. Stop calls this after cancelling.
    pub fn wake(&self) {
        let _state = self.lock();
        self.changed.notify_all();
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::mpsc;
    use std::thread;
    use std::time::Instant;

    #[test]
    fn wait_while_paused_returns_on_cancel_without_wake() {
        let steering = Arc::new(TurnSteering::new());
        let cancel = CancelToken::new();
        steering.pause();
        let (done_tx, done_rx) = mpsc::channel();
        let waiter = {
            let steering = Arc::clone(&steering);
            let cancel = cancel.clone();
            thread::spawn(move || {
                let resumed = steering.wait_while_paused(&cancel);
                done_tx.send(()).unwrap();
                resumed
            })
        };

        // Long enough for the waiter to be parked on the condition variable;
        // if it is not yet, the test passes trivially rather than flakily.
        thread::sleep(Duration::from_millis(50));
        let started = Instant::now();
        // Deliberately no `wake`: the re-check alone must notice.
        cancel.cancel();
        done_rx
            .recv_timeout(Duration::from_secs(5))
            .expect("a cancelled wait must return without being woken");

        assert!(!waiter.join().unwrap(), "cancelled, not resumed");
        assert!(started.elapsed() < Duration::from_secs(5));
    }

    #[test]
    fn resume_releases_a_waiting_turn() {
        let steering = Arc::new(TurnSteering::new());
        let cancel = CancelToken::new();
        steering.pause();
        let waiter = {
            let steering = Arc::clone(&steering);
            thread::spawn(move || steering.wait_while_paused(&cancel))
        };

        steering.resume();

        assert!(waiter.join().unwrap());
        assert!(!steering.is_paused());
    }

    #[test]
    fn notes_drain_in_order_and_once() {
        let steering = TurnSteering::new();
        steering.queue_note("first".to_string());
        steering.queue_note("second".to_string());

        assert_eq!(steering.drain_notes(), vec!["first", "second"]);
        assert!(steering.drain_notes().is_empty());
    }
}
