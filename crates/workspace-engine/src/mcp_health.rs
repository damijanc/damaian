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
    /// The session whose turn made the contact.
    pub session_id: String,
}

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
    let observation = McpHealthObservation {
        repository_id: repository_id.to_string(),
        server_id: server_id.to_string(),
        health,
        observed_at_ms: now_ms(),
        session_id: session_id.to_string(),
    };
    // A poisoned lock still holds a usable map: health is advisory, and a
    // panic elsewhere must not stop the next contact being recorded.
    let mut map = store()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    map.insert(
        (repository_id.to_string(), server_id.to_string()),
        observation,
    );
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
