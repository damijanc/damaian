//! What this process has learned about each MCP server's health (spec 33
//! §5.3, `context.md` D5).
//!
//! The store is process-wide and in memory only. It is process-wide because
//! the engine is rebuilt for every request and the MCP runtime for every turn,
//! so neither outlives one contact; the process is the narrowest thing that
//! remembers across turns. Since spec 110 Task 1b each session's work runs in
//! a session host process of its own, so in the desktop app a host's store is
//! that session's health — which is what `Unknown` ("never contacted this
//! session") and `Failed` ("not retried automatically this session") are
//! defined against. The shell's own store holds only what the shell itself
//! contacted. A reader that wants every session's view (Task 7's health
//! route) collects [`mcp_health_snapshot`] from each process and folds the
//! lists with [`merge_mcp_health`].
//!
//! Nothing here is persisted: a persisted `Unknown` or `Failed` would
//! contradict its own definition.

use crate::error::ClientError;
use crate::mcp::McpServerHealth;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

/// One server's latest health as seen by one process.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpHealthObservation {
    pub repository_id: String,
    pub server_id: String,
    pub health: McpServerHealth,
    /// When the contact that set `health` ended, in Unix milliseconds. Lets a
    /// reader merging several processes' snapshots keep the latest.
    pub observed_at_ms: u64,
    /// The session whose turn made the contact; empty for a manual retry
    /// from the settings screen, which belongs to no session.
    pub session_id: String,
    /// Failed contacts in a row in this process since the last success or
    /// manual retry. [`MCP_FAILED_AFTER_CONSECUTIVE_FAILURES`] of them latch
    /// the server `Failed`.
    #[serde(default)]
    pub consecutive_failures: u32,
    /// What the latest failed contact showed (`unreachable`, `timeout`,
    /// `unauthenticated`), so a `Failed` server says why. `None` after a
    /// success.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_failure: Option<McpServerHealth>,
}

impl McpHealthObservation {
    /// Why this server is `Failed`, in words for the model and the user, or
    /// `None` when it is not (spec 33 §5.3: "with the reason visible").
    pub fn failed_reason(&self) -> Option<String> {
        (self.health == McpServerHealth::Failed).then(|| {
            let last = match self.last_failure {
                Some(McpServerHealth::Timeout) => "it timed out",
                Some(McpServerHealth::Unauthenticated) => "it refused the credentials",
                _ => "it could not be reached",
            };
            format!(
                "The MCP server `{}` failed {} times in a row (last time {last}), so it is \
                 not retried automatically in this session and its tools are not offered. \
                 The user can retry it from the MCP settings.",
                self.server_id, self.consecutive_failures
            )
        })
    }
}

/// How many failed contacts in a row make a server `Failed` (spec 33 §5.3):
/// not retried automatically for the rest of the session and its tools not
/// offered, until the user retries it. Every contact counts — a connect and
/// list, or a call that had to connect — so a turn whose call fails usually
/// spends two (the listing before the permission check, then the call), and
/// the second turn that tries latches it. A server that only refuses one call
/// with a JSON-RPC error is `Ready` and counts nothing.
pub const MCP_FAILED_AFTER_CONSECUTIVE_FAILURES: u32 = 3;

type Store = HashMap<(String, String), McpHealthObservation>;

fn store() -> &'static Mutex<Store> {
    static STORE: OnceLock<Mutex<Store>> = OnceLock::new();
    STORE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn now_ms() -> u64 {
    u64::try_from(crate::hash::now_millis()).unwrap_or(u64::MAX)
}

/// Records the outcome of a contact with `server_id`. An empty repository id
/// (an inert runtime) records nothing.
pub(crate) fn record_mcp_health(
    repository_id: &str,
    server_id: &str,
    session_id: &str,
    health: McpServerHealth,
) {
    if repository_id.is_empty() {
        return;
    }
    // A poisoned lock still holds a usable map: health is advisory, and a
    // panic elsewhere must not stop the next contact being recorded.
    let mut map = store()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let key = (repository_id.to_string(), server_id.to_string());
    let previous = map.get(&key);
    let (health, consecutive_failures, last_failure) = match health {
        McpServerHealth::Ready => (McpServerHealth::Ready, 0, None),
        // Never recorded by a contact; kept for completeness.
        McpServerHealth::Unknown | McpServerHealth::Failed => {
            let failures = previous.map_or(0, |kept| kept.consecutive_failures);
            (
                health,
                failures,
                previous.and_then(|kept| kept.last_failure),
            )
        }
        failure => {
            let failures = previous.map_or(0, |kept| kept.consecutive_failures) + 1;
            let latched = failures >= MCP_FAILED_AFTER_CONSECUTIVE_FAILURES
                || previous.is_some_and(|kept| kept.health == McpServerHealth::Failed);
            let shown = if latched {
                McpServerHealth::Failed
            } else {
                failure
            };
            (shown, failures, Some(failure))
        }
    };
    let observation = McpHealthObservation {
        repository_id: repository_id.to_string(),
        server_id: server_id.to_string(),
        health,
        observed_at_ms: now_ms(),
        session_id: session_id.to_string(),
        consecutive_failures,
        last_failure,
    };
    map.insert(key, observation);
}

/// The manual retry's first half (spec 33 §5.3): forgets what this process
/// knew about `server_id` — a `Failed` latch and the failure count included —
/// so the server reads `Unknown` and the next use contacts it again. Returns
/// the health it had. The second half, contacting the server now, is
/// [`crate::McpRuntime::retry_server`].
pub fn clear_mcp_health(repository_id: &str, server_id: &str) -> McpServerHealth {
    let mut map = store()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    map.remove(&(repository_id.to_string(), server_id.to_string()))
        .map_or(McpServerHealth::Unknown, |observation| observation.health)
}

/// This process's latest observation of one server, if it contacted it.
pub fn mcp_health_observation(
    repository_id: &str,
    server_id: &str,
) -> Option<McpHealthObservation> {
    let map = store()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    map.get(&(repository_id.to_string(), server_id.to_string()))
        .cloned()
}

/// This process's view of one server: `Unknown` until something here has
/// contacted it.
pub fn mcp_server_health(repository_id: &str, server_id: &str) -> McpServerHealth {
    let map = store()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    map.get(&(repository_id.to_string(), server_id.to_string()))
        .map_or(McpServerHealth::Unknown, |observation| observation.health)
}

/// Every server this process has contacted, ordered by repository then
/// server. A configured server that is absent is `Unknown`.
pub fn mcp_health_snapshot() -> Vec<McpHealthObservation> {
    let map = store()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let mut observations: Vec<_> = map.values().cloned().collect();
    observations
        .sort_by(|a, b| (&a.repository_id, &a.server_id).cmp(&(&b.repository_id, &b.server_id)));
    observations
}

/// Folds several processes' snapshots into one, keeping the latest
/// observation per repository and server (ties keep the first seen). For a
/// reader that gathers the shell's and each session host's snapshots.
pub fn merge_mcp_health(
    observations: impl IntoIterator<Item = McpHealthObservation>,
) -> Vec<McpHealthObservation> {
    let mut latest: Store = HashMap::new();
    for observation in observations {
        let key = (
            observation.repository_id.clone(),
            observation.server_id.clone(),
        );
        match latest.get(&key) {
            Some(kept) if kept.observed_at_ms >= observation.observed_at_ms => {}
            _ => {
                latest.insert(key, observation);
            }
        }
    }
    let mut merged: Vec<_> = latest.into_values().collect();
    merged.sort_by(|a, b| (&a.repository_id, &a.server_id).cmp(&(&b.repository_id, &b.server_id)));
    merged
}

/// The contact phase an error came from: connecting or listing (the server
/// could not be used at all), or a call (the server was already in use).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum McpContactPhase {
    Connect,
    Call,
}

/// The health a failed contact implies, or `None` when the failure says
/// nothing about the server (the user stopped the turn).
///
/// - a timeout in any phase is `Timeout` (§5.4);
/// - a refusal of the credentials (HTTP 401/403, surfaced as
///   [`ClientError::AccessDenied`]) is `Unauthenticated`;
/// - in a call, a JSON-RPC error answer ([`ClientError::InvalidInput`]) means
///   the server is up and refused that one call, so it stays `Ready`;
/// - anything else — spawn, network, handshake, or a server that cannot list
///   its tools — is `Unreachable`.
pub(crate) fn health_after_failure(
    phase: McpContactPhase,
    error: &ClientError,
) -> Option<McpServerHealth> {
    Some(match error {
        ClientError::Cancelled => return None,
        ClientError::McpTimeout(_) => McpServerHealth::Timeout,
        ClientError::AccessDenied(_) => McpServerHealth::Unauthenticated,
        ClientError::InvalidInput(_) if phase == McpContactPhase::Call => McpServerHealth::Ready,
        _ => McpServerHealth::Unreachable,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn observation(server: &str, health: McpServerHealth, at: u64) -> McpHealthObservation {
        McpHealthObservation {
            repository_id: "repo".to_string(),
            server_id: server.to_string(),
            health,
            observed_at_ms: at,
            session_id: format!("session_{at}"),
            consecutive_failures: 0,
            last_failure: None,
        }
    }

    #[test]
    fn an_uncontacted_server_is_unknown_and_a_contact_is_remembered() {
        // Unique ids: library tests share this process's store.
        let repo = format!("health-test-repo-{}", crate::hash::create_id("r"));
        assert_eq!(
            mcp_server_health(&repo, "tracker"),
            McpServerHealth::Unknown
        );
        record_mcp_health(&repo, "tracker", "session_1", McpServerHealth::Ready);
        assert_eq!(mcp_server_health(&repo, "tracker"), McpServerHealth::Ready);
        assert_eq!(mcp_server_health(&repo, "other"), McpServerHealth::Unknown);
        record_mcp_health(&repo, "tracker", "session_1", McpServerHealth::Timeout);
        assert_eq!(
            mcp_server_health(&repo, "tracker"),
            McpServerHealth::Timeout
        );
        let mine: Vec<_> = mcp_health_snapshot()
            .into_iter()
            .filter(|observation| observation.repository_id == repo)
            .collect();
        assert_eq!(mine.len(), 1);
        assert_eq!(mine[0].session_id, "session_1");
    }

    #[test]
    fn repeated_failures_latch_failed_until_cleared() {
        let repo = format!("health-test-repo-{}", crate::hash::create_id("r"));
        for attempt in 1..MCP_FAILED_AFTER_CONSECUTIVE_FAILURES {
            record_mcp_health(&repo, "tracker", "s", McpServerHealth::Unreachable);
            assert_eq!(
                mcp_server_health(&repo, "tracker"),
                McpServerHealth::Unreachable,
                "attempt {attempt} is not yet repeated failure"
            );
        }
        record_mcp_health(&repo, "tracker", "s", McpServerHealth::Timeout);
        let observation = mcp_health_observation(&repo, "tracker").unwrap();
        assert_eq!(observation.health, McpServerHealth::Failed);
        assert_eq!(observation.last_failure, Some(McpServerHealth::Timeout));
        assert_eq!(
            observation.consecutive_failures,
            MCP_FAILED_AFTER_CONSECUTIVE_FAILURES
        );
        assert_eq!(clear_mcp_health(&repo, "tracker"), McpServerHealth::Failed);
        assert_eq!(
            mcp_server_health(&repo, "tracker"),
            McpServerHealth::Unknown
        );
        // A success in between resets the count.
        record_mcp_health(&repo, "tracker", "s", McpServerHealth::Unreachable);
        record_mcp_health(&repo, "tracker", "s", McpServerHealth::Ready);
        record_mcp_health(&repo, "tracker", "s", McpServerHealth::Unreachable);
        let observation = mcp_health_observation(&repo, "tracker").unwrap();
        assert_eq!(observation.health, McpServerHealth::Unreachable);
        assert_eq!(observation.consecutive_failures, 1);
    }

    #[test]
    fn an_inert_runtime_records_nothing() {
        let before = mcp_health_snapshot().len();
        record_mcp_health("", "tracker", "session_1", McpServerHealth::Ready);
        assert!(
            mcp_health_snapshot()
                .iter()
                .all(|observation| !observation.repository_id.is_empty())
        );
        assert!(mcp_health_snapshot().len() >= before);
    }

    #[test]
    fn merging_keeps_the_latest_observation_per_server() {
        let merged = merge_mcp_health([
            observation("a", McpServerHealth::Ready, 10),
            observation("b", McpServerHealth::Unreachable, 5),
            observation("a", McpServerHealth::Timeout, 20),
            observation("a", McpServerHealth::Unauthenticated, 15),
        ]);
        assert_eq!(merged.len(), 2);
        assert_eq!(merged[0].server_id, "a");
        assert_eq!(merged[0].health, McpServerHealth::Timeout);
        assert_eq!(merged[1].health, McpServerHealth::Unreachable);
    }

    #[test]
    fn a_failure_maps_to_the_health_it_implies() {
        use McpContactPhase::{Call, Connect};
        let timeout = ClientError::McpTimeout("t".to_string());
        let refused = ClientError::AccessDenied("401".to_string());
        let rpc = ClientError::InvalidInput("MCP tools/call failed: no".to_string());
        let io = ClientError::Io("broken pipe".to_string());
        assert_eq!(
            health_after_failure(Connect, &timeout),
            Some(McpServerHealth::Timeout)
        );
        assert_eq!(
            health_after_failure(Call, &timeout),
            Some(McpServerHealth::Timeout)
        );
        assert_eq!(
            health_after_failure(Connect, &refused),
            Some(McpServerHealth::Unauthenticated)
        );
        assert_eq!(
            health_after_failure(Connect, &rpc),
            Some(McpServerHealth::Unreachable)
        );
        assert_eq!(
            health_after_failure(Call, &rpc),
            Some(McpServerHealth::Ready)
        );
        assert_eq!(
            health_after_failure(Call, &io),
            Some(McpServerHealth::Unreachable)
        );
        assert_eq!(health_after_failure(Call, &ClientError::Cancelled), None);
    }
}
