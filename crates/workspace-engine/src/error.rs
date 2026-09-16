use std::fmt::{Display, Formatter};

/// A provider's refusal to answer, classified from HTTP status first and an
/// error object's structured `type`/`code` field second — never from free
/// prose. Spec 48 §5.2.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProviderRefusal {
    /// 429. Retry after `retry_after_secs` when the provider named one.
    RateLimited { retry_after_secs: Option<u64> },
    /// 500, 502, 503, 504 — the provider is unwell, not the request.
    Overloaded { retry_after_secs: Option<u64> },
    /// 402, or a 4xx whose body names an exhausted balance or quota.
    /// Permanent until the user does something outside Damaian.
    QuotaExhausted,
    /// 401, 403.
    AuthFailed,
    /// 400, 404, 422 — the request will not succeed if repeated.
    BadRequest,
    /// A refusal that was recognised as one but not classified further.
    /// Treated as permanent: guessing "transient" retries an error that will
    /// never clear.
    Unknown,
}

impl ProviderRefusal {
    /// The stable code the audit log and the UI branch on, instead of a
    /// sentence. Mirrors `ClientError::code`.
    pub fn code(&self) -> &'static str {
        match self {
            Self::RateLimited { .. } => "provider_rate_limited",
            Self::Overloaded { .. } => "provider_overloaded",
            Self::QuotaExhausted => "provider_quota_exhausted",
            Self::AuthFailed => "provider_auth_failed",
            Self::BadRequest => "provider_bad_request",
            Self::Unknown => "provider_refused",
        }
    }

    /// True for the two kinds of refusal worth retrying. A permanent refusal
    /// is retried only if the classifier guesses "transient" wrong, which is
    /// the bug this work exists to prevent.
    pub fn is_transient(&self) -> bool {
        matches!(self, Self::RateLimited { .. } | Self::Overloaded { .. })
    }

    /// The provider's own retry guidance, when it sent one.
    pub fn retry_after_secs(&self) -> Option<u64> {
        match self {
            Self::RateLimited { retry_after_secs } | Self::Overloaded { retry_after_secs } => {
                *retry_after_secs
            }
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClientError {
    AccessDenied(String),
    ApprovalRequired(String),
    PatchConflict(String),
    PolicyBlocked(String),
    Io(String),
    Git(String),
    InvalidInput(String),
    /// A provider refused the call, classified by [`ProviderRefusal`], with the
    /// provider's own message carried verbatim so the user learns what to fix.
    Provider(ProviderRefusal, String),
    /// The user stopped an in-flight turn. An internal control signal, not a
    /// failure: `run_agentic_turn` catches it and reports a cancelled result.
    Cancelled,
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
            Self::Provider(refusal, _) => refusal.code(),
            Self::Cancelled => "cancelled",
        }
    }

    /// True for transient failures worth retrying: provider rate limits, timeouts,
    /// and network/DNS-level connection failures. False for anything else (auth,
    /// malformed requests, policy decisions), where retrying can't help.
    pub fn is_retryable(&self) -> bool {
        match self {
            Self::Io(message) => is_retryable_message(message),
            Self::Provider(refusal, _) => refusal.is_transient(),
            _ => false,
        }
    }
}

/// Shared classifier for transient-vs-permanent transport error messages — the
/// text curl itself puts on stderr when a transfer fails below the model layer.
/// Rate limits were once matched here too, but that arm was dead: a provider's
/// 429 reaches Damaian as a parsed response body, never as curl stderr, and is
/// classified from its status by [`ProviderRefusal`] instead. Removing the arm
/// means the next person cannot conclude the case is handled here.
pub fn is_retryable_message(message: &str) -> bool {
    let lower = message.to_lowercase();
    lower.contains("timeout")
        || lower.contains("timed out")
        // curl's wording when it aborts a transfer that fell below
        // `speed-limit` for `speed-time` — a stalled stream, not a dead one.
        || lower.contains("too slow")
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
            | Self::InvalidInput(message)
            | Self::Provider(_, message) => formatter.write_str(message),
            Self::Cancelled => formatter.write_str("Stopped by user"),
        }
    }
}

impl std::error::Error for ClientError {}

impl From<std::io::Error> for ClientError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_message_classifier_no_longer_names_rate_limits() {
        // Removed because it was dead: a provider's 429 reaches Damaian as a
        // parsed response body, never as curl stderr, and is classified from
        // status by `ProviderRefusal`. Leaving the arm would let the next
        // person conclude the case is handled here.
        assert!(!is_retryable_message("provider rate limit exceeded"));
        assert!(!is_retryable_message("http status 429"));
        assert!(is_retryable_message("could not resolve host"));
        assert!(is_retryable_message("operation timed out"));
    }

    #[test]
    fn a_provider_error_carries_the_refusal_and_a_code() {
        let error =
            ClientError::Provider(ProviderRefusal::QuotaExhausted, "out of credit".to_string());
        assert_eq!(error.code(), "provider_quota_exhausted");
        assert!(format!("{error}").contains("out of credit"));
    }

    #[test]
    fn a_transient_refusal_is_retryable_and_a_permanent_one_is_not() {
        let transient = ClientError::Provider(
            ProviderRefusal::RateLimited {
                retry_after_secs: None,
            },
            "slow".into(),
        );
        assert!(transient.is_retryable());
        let permanent = ClientError::Provider(ProviderRefusal::AuthFailed, "bad key".into());
        assert!(!permanent.is_retryable());
    }
}
