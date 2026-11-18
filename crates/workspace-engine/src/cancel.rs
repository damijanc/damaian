//! Cooperative cancellation for an in-flight chat turn.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::error::{ClientError, Result};

/// A stop signal shared between the request handler that owns the client
/// connection and the worker thread running the turn.
///
/// Cancellation is cooperative: the turn checks the flag at points where
/// stopping is safe (between tool rounds, and while waiting on the provider)
/// rather than being killed from outside.
#[derive(Clone, Debug, Default)]
pub struct CancelToken(Arc<AtomicBool>);

impl CancelToken {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn cancel(&self) {
        self.0.store(true, Ordering::SeqCst);
    }

    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }

    /// `Err(ClientError::Cancelled)` once cancelled, so a check point can be a
    /// single `?` inside code that already returns [`Result`].
    pub fn check(&self) -> Result<()> {
        if self.is_cancelled() {
            return Err(ClientError::Cancelled);
        }
        Ok(())
    }

    /// Runs `work` with `self` as the token of the turn running on this
    /// thread, restoring whatever was there before (spec 109 D8).
    ///
    /// The runners a turn reaches without a [`TurnSink`](crate::TurnSink) in
    /// hand — hooks, the commands a hook asks for, MCP calls and the browser
    /// diagnostics built on them — read it through [`Self::current`], so the
    /// user's Stop ends them as it ends a command, without every dispatch
    /// signature carrying the token. They all run on the turn's own thread.
    pub fn scope<T>(&self, work: impl FnOnce() -> T) -> T {
        struct Restore(Option<CancelToken>);
        impl Drop for Restore {
            fn drop(&mut self) {
                let previous = self.0.take();
                TURN_CANCEL.with(|current| *current.borrow_mut() = previous);
            }
        }
        let previous = TURN_CANCEL.with(|current| current.borrow_mut().replace(self.clone()));
        // Restored on unwind too, so a panicking turn cannot leave its token
        // to the next piece of work on a reused thread.
        let _restore = Restore(previous);
        work()
    }

    /// The token of the turn running on this thread, or a fresh one nothing
    /// can cancel when no turn set one (the CLI, a settings test run).
    pub fn current() -> CancelToken {
        TURN_CANCEL
            .with(|current| current.borrow().clone())
            .unwrap_or_default()
    }
}

thread_local! {
    static TURN_CANCEL: std::cell::RefCell<Option<CancelToken>> =
        const { std::cell::RefCell::new(None) };
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::ClientError;
    use crate::session::TaskStatus;

    #[test]
    fn a_fresh_token_is_not_cancelled() {
        let token = CancelToken::default();
        assert!(!token.is_cancelled());
        assert!(token.check().is_ok());
    }

    #[test]
    fn cancelling_makes_check_fail() {
        let token = CancelToken::default();
        token.cancel();
        assert!(token.is_cancelled());
        assert_eq!(token.check().unwrap_err(), ClientError::Cancelled);
    }

    #[test]
    fn a_clone_observes_a_cancellation_made_through_the_original() {
        // The point of the type: the shell handler keeps one handle and the
        // worker thread another, so a stop on either must be visible to both.
        let token = CancelToken::default();
        let worker_handle = token.clone();
        token.cancel();
        assert!(worker_handle.is_cancelled());
    }

    #[test]
    fn a_scoped_token_is_current_only_inside_its_scope_and_nests() {
        assert!(!CancelToken::current().is_cancelled());
        let outer = CancelToken::new();
        let inner = CancelToken::new();
        outer.scope(|| {
            inner.scope(|| {
                inner.cancel();
                assert!(CancelToken::current().is_cancelled());
            });
            // The outer turn's token is back, and it was never cancelled.
            assert!(!CancelToken::current().is_cancelled());
            outer.cancel();
            assert!(CancelToken::current().is_cancelled());
        });
        assert!(!CancelToken::current().is_cancelled());
    }

    #[test]
    fn a_scope_is_restored_when_its_work_panics() {
        let token = CancelToken::new();
        token.cancel();
        let unwound = std::panic::catch_unwind(|| token.scope(|| panic!("turn failed")));
        assert!(unwound.is_err());
        assert!(!CancelToken::current().is_cancelled());
    }

    #[test]
    fn a_cancelled_turn_is_never_retried() {
        // `OpenAICompatibleAdapter::stream_response` retries on
        // `is_retryable()`. A user pressing Stop must not be mistaken for a
        // transient provider failure and re-sent.
        assert!(!ClientError::Cancelled.is_retryable());
    }

    #[test]
    fn cancelled_reports_its_own_error_code() {
        assert_eq!(ClientError::Cancelled.code(), "cancelled");
    }

    #[test]
    fn cancelled_is_a_distinct_task_status() {
        assert_eq!(TaskStatus::Cancelled.as_str(), "cancelled");
    }
}
