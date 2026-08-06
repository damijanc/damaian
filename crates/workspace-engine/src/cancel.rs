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
