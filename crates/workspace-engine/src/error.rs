use std::fmt::{Display, Formatter};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClientError {
    AccessDenied(String),
    ApprovalRequired(String),
    PatchConflict(String),
    PolicyBlocked(String),
    Io(String),
    Git(String),
    InvalidInput(String),
}

pub type Result<T> = std::result::Result<T, ClientError>;

impl ClientError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::AccessDenied(_) => "access_denied",
            Self::ApprovalRequired(_) => "approval_required",
            Self::PatchConflict(_) => "patch_conflict",
            Self::PolicyBlocked(_) => "policy_blocked",
            Self::Io(_) => "io_error",
            Self::Git(_) => "git_error",
            Self::InvalidInput(_) => "invalid_input",
        }
    }

    /// True for transient failures worth retrying: provider rate limits, timeouts,
    /// and network/DNS-level connection failures. False for anything else (auth,
    /// malformed requests, policy decisions), where retrying can't help.
    pub fn is_retryable(&self) -> bool {
        match self {
            Self::Io(message) => is_retryable_message(message),
            _ => false,
        }
    }
}

/// Shared classifier for transient-vs-permanent error messages, used both by
/// `ClientError::is_retryable` and by user-facing message mapping so retry
/// decisions and displayed messages never drift out of sync.
pub fn is_retryable_message(message: &str) -> bool {
    let lower = message.to_lowercase();
    lower.contains("rate limit")
        || lower.contains("429")
        || lower.contains("timeout")
        || lower.contains("timed out")
        || lower.contains("connection")
        || lower.contains("could not resolve")
}

impl Display for ClientError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::AccessDenied(message)
            | Self::ApprovalRequired(message)
            | Self::PatchConflict(message)
            | Self::PolicyBlocked(message)
            | Self::Io(message)
            | Self::Git(message)
            | Self::InvalidInput(message) => formatter.write_str(message),
        }
    }
}

impl std::error::Error for ClientError {}

impl From<std::io::Error> for ClientError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error.to_string())
    }
}
