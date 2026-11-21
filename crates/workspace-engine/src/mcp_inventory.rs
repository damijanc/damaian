//! The persisted MCP tool inventory and the deferral-ceiling decision it feeds
//! (spec 33 §5.1, `context.md` D4 and D7).
//!
//! Whether a turn sends MCP tool schemas directly or defers them behind a
//! search tool must be decided **without connecting** to any server, or "a
//! session that uses no MCP tool starts no server process" cannot hold. So the
//! decision reads what an earlier successful `tools/list` recorded here, and a
//! server with no record reads as *unknown cost* and is deferred — nothing is
//! started to find out.
//!
//! Storage is one JSON file per server under
//! `<data_dir>/mcp/inventory/<repository_id>/<server_id>.json`. Keying by
//! repository identity follows the per-checkout rule of
//! [`crate::hash::repository_id_for_root`]; a file per server means two
//! sessions refreshing different servers of one repository never overwrite
//! each other's listing. A missing or unreadable file reads as "no inventory",
//! which defers: the safe direction.

use crate::context_manager::TokenEstimate;
use crate::error::{ClientError, Result};
use crate::hash::{now_millis, sha256};
use crate::mcp::McpTool;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

/// One tool as a server last listed it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpInventoryTool {
    /// The tool's own name, not the `mcp__<server>__<tool>` namespaced one.
    pub name: String,
    pub description: String,
    /// Byte length of the serialized `inputSchema`.
    pub schema_bytes: usize,
    /// Spec 26's estimate at the structured rate over `description` plus the
    /// serialized `inputSchema` (D7) — the measurement the context budget
    /// already trusts.
    pub estimated_tokens: usize,
    /// The server's own read-only claim, kept as an attributed assertion
    /// (proposal §5.5), never as a guarantee.
    pub read_only_hint: Option<bool>,
    /// `sha256:` over name, description, schema, and claim, so a later listing
    /// can tell an unchanged tool from one whose definition moved.
    pub content_hash: String,
    /// When this tool name first appeared in a listing of this server, kept
    /// across refreshes so the UI can mark tools that are new since the user
    /// last looked (proposal §5.2, §5.9).
    pub first_seen_at_ms: u128,
}

impl McpInventoryTool {
    fn from_tool(tool: &McpTool, first_seen_at_ms: u128) -> Self {
        Self {
            name: tool.name.clone(),
            description: tool.description.clone(),
            schema_bytes: tool.input_schema_json.len(),
            estimated_tokens: estimate_mcp_tool_tokens(tool),
            read_only_hint: tool.read_only_hint,
            content_hash: mcp_tool_content_hash(tool),
            first_seen_at_ms,
        }
    }
}

/// One server's last successful listing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpServerInventory {
    pub server_id: String,
    pub refreshed_at_ms: u128,
    pub tools: Vec<McpInventoryTool>,
}

impl McpServerInventory {
    /// The summed estimate of every listed tool, before any filtering.
    pub fn estimated_tokens(&self) -> usize {
        self.tools.iter().map(|tool| tool.estimated_tokens).sum()
    }
}

/// The inventory of every server recorded for one repository.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct McpToolInventory {
    pub repository_id: String,
    pub servers: BTreeMap<String, McpServerInventory>,
}

impl McpToolInventory {
    /// The directory one repository's inventory lives in.
    pub fn directory(data_dir: &Path, repository_id: &str) -> PathBuf {
        data_dir.join("mcp").join("inventory").join(repository_id)
    }

    fn server_path(data_dir: &Path, repository_id: &str, server_id: &str) -> PathBuf {
        Self::directory(data_dir, repository_id).join(format!("{server_id}.json"))
    }

    /// Loads the inventory for `server_ids` only. A server without a readable
    /// record is simply absent, which the deferral decision reads as unknown
    /// cost. Listing only the asked-for servers keeps a server removed from
    /// config from contributing a stale figure.
    pub fn load(data_dir: &Path, repository_id: &str, server_ids: &[&str]) -> Self {
        let mut servers = BTreeMap::new();
        for server_id in server_ids {
            if let Some(server) = Self::load_server(data_dir, repository_id, server_id) {
                servers.insert((*server_id).to_string(), server);
            }
        }
        Self {
            repository_id: repository_id.to_string(),
            servers,
        }
    }

    /// One server's record, or `None` when it is missing, unreadable, or
    /// recorded under another id.
    pub fn load_server(
        data_dir: &Path,
        repository_id: &str,
        server_id: &str,
    ) -> Option<McpServerInventory> {
        let text =
            fs::read_to_string(Self::server_path(data_dir, repository_id, server_id)).ok()?;
        let server: McpServerInventory = serde_json::from_str(&text).ok()?;
        (server.server_id == server_id).then_some(server)
    }

    /// Records a successful `tools/list` for one server, keeping each
    /// still-listed tool's `first_seen_at_ms`, and writes it atomically
    /// (temporary file, then rename) so a concurrent reader never sees half a
    /// file. Returns what was written.
    pub fn record_listing(
        data_dir: &Path,
        repository_id: &str,
        server_id: &str,
        tools: &[McpTool],
    ) -> Result<McpServerInventory> {
        let now = now_millis();
        let previous = Self::load_server(data_dir, repository_id, server_id);
        let first_seen: BTreeMap<&str, u128> = previous
            .as_ref()
            .map(|server| {
                server
                    .tools
                    .iter()
                    .map(|tool| (tool.name.as_str(), tool.first_seen_at_ms))
                    .collect()
            })
            .unwrap_or_default();
        let server = McpServerInventory {
            server_id: server_id.to_string(),
            refreshed_at_ms: now,
            tools: tools
                .iter()
                .map(|tool| {
                    let seen = first_seen.get(tool.name.as_str()).copied().unwrap_or(now);
                    McpInventoryTool::from_tool(tool, seen)
                })
                .collect(),
        };
        let directory = Self::directory(data_dir, repository_id);
        fs::create_dir_all(&directory)?;
        let path = Self::server_path(data_dir, repository_id, server_id);
        let temporary = directory.join(format!(".{server_id}.json.{}.tmp", std::process::id()));
        let bytes = serde_json::to_vec_pretty(&server).map_err(|error| {
            ClientError::Io(format!("could not serialise the MCP inventory: {error}"))
        })?;
        fs::write(&temporary, bytes)?;
        fs::rename(&temporary, &path)?;
        Ok(server)
    }
}

/// D7: spec 26's structured-rate estimate over the tool's description and
/// serialized input schema.
pub fn estimate_mcp_tool_tokens(tool: &McpTool) -> usize {
    TokenEstimate::Structured.tokens(&tool.description)
        + TokenEstimate::Structured.tokens(&tool.input_schema_json)
}

fn mcp_tool_content_hash(tool: &McpTool) -> String {
    let hint = match tool.read_only_hint {
        Some(true) => "true",
        Some(false) => "false",
        None => "none",
    };
    sha256(format!(
        "{}\0{}\0{}\0{hint}",
        tool.name, tool.description, tool.input_schema_json
    ))
}

/// Whether a server's tools go to the model as schemas or wait behind search.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum McpToolSurface {
    /// Under the ceiling: every enabled, offered tool's schema is sent.
    Direct,
    /// Over the ceiling, or of unknown cost: reachable through search only.
    Deferred,
}

/// The direct-versus-deferred decision for one turn, made from the inventory
/// alone (spec 33 §5.1, D2, D4).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpDeferralDecision {
    /// `mcp_deferral_ceiling_tokens` at the time of the decision.
    pub ceiling_tokens: usize,
    /// The summed estimate of every enabled, offered tool on servers with an
    /// inventory.
    pub known_tokens: usize,
    /// The surface for servers with an inventory: `Direct` only when
    /// `known_tokens` is at or under the ceiling.
    pub known_surface: McpToolSurface,
    /// Per-server estimate of its enabled, offered tools, for the UI's
    /// context-cost figure (§5.9).
    pub server_tokens: BTreeMap<String, usize>,
    /// Servers with no inventory: unknown cost, always deferred.
    pub unknown_servers: Vec<String>,
}

impl McpDeferralDecision {
    /// The surface for one server. A server the decision did not consider —
    /// no longer configured, say — is deferred, the safe direction.
    pub fn surface_for(&self, server_id: &str) -> McpToolSurface {
        if self.server_tokens.contains_key(server_id) {
            self.known_surface
        } else {
            McpToolSurface::Deferred
        }
    }
}

/// Decides the surface for `server_ids` from `inventory`, counting only tools
/// `offered(server_id, tool)` accepts — the caller's intersection of the
/// per-tool `enabled` setting and the mode and profile layers. Connects to
/// nothing.
///
/// Known servers are decided together against one ceiling, because the ceiling
/// bounds the whole request's schema cost, not one server's.
pub fn decide_deferral(
    inventory: &McpToolInventory,
    server_ids: &[&str],
    ceiling_tokens: usize,
    mut offered: impl FnMut(&str, &McpInventoryTool) -> bool,
) -> McpDeferralDecision {
    let mut server_tokens = BTreeMap::new();
    let mut unknown_servers = Vec::new();
    for server_id in server_ids {
        match inventory.servers.get(*server_id) {
            Some(server) => {
                let tokens = server
                    .tools
                    .iter()
                    .filter(|tool| offered(server_id, tool))
                    .map(|tool| tool.estimated_tokens)
                    .sum::<usize>();
                server_tokens.insert((*server_id).to_string(), tokens);
            }
            None => unknown_servers.push((*server_id).to_string()),
        }
    }
    let known_tokens = server_tokens.values().sum();
    let known_surface = if known_tokens <= ceiling_tokens {
        McpToolSurface::Direct
    } else {
        McpToolSurface::Deferred
    };
    McpDeferralDecision {
        ceiling_tokens,
        known_tokens,
        known_surface,
        server_tokens,
        unknown_servers,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tool(name: &str, description: &str, schema: &str) -> McpTool {
        McpTool {
            name: name.to_string(),
            description: description.to_string(),
            input_schema_json: schema.to_string(),
            read_only_hint: None,
        }
    }

    fn temp_data_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "damaian-mcp-inventory-{name}-{}-{}",
            std::process::id(),
            now_millis()
        ));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn estimate_uses_the_structured_rate_over_description_and_schema() {
        // 12 bytes of description and 24 of schema at 2.4 B/token: 5 + 10.
        let echo = tool("echo", "Echoes text.", r#"{"type":"object","x":1}"#);
        assert_eq!(echo.input_schema_json.len(), 23);
        assert_eq!(
            estimate_mcp_tool_tokens(&echo),
            TokenEstimate::Structured.tokens("Echoes text.")
                + TokenEstimate::Structured.tokens(r#"{"type":"object","x":1}"#)
        );
        assert_eq!(estimate_mcp_tool_tokens(&echo), 5 + 10);
    }

    #[test]
    fn a_listing_round_trips_and_keeps_first_seen_across_refreshes() {
        let data_dir = temp_data_dir("round-trip");
        let first = McpToolInventory::record_listing(
            &data_dir,
            "repo_a",
            "fake",
            &[tool("echo", "Echoes", "{}")],
        )
        .unwrap();
        let echo_seen = first.tools[0].first_seen_at_ms;
        std::thread::sleep(std::time::Duration::from_millis(2));
        let second = McpToolInventory::record_listing(
            &data_dir,
            "repo_a",
            "fake",
            &[tool("echo", "Echoes", "{}"), tool("ping", "Pings", "{}")],
        )
        .unwrap();
        assert_eq!(second.tools[0].first_seen_at_ms, echo_seen);
        assert!(second.tools[1].first_seen_at_ms > echo_seen);
        assert_eq!(second.tools[0].content_hash, first.tools[0].content_hash);

        let loaded = McpToolInventory::load(&data_dir, "repo_a", &["fake", "absent"]);
        assert_eq!(loaded.servers.get("fake"), Some(&second));
        assert!(!loaded.servers.contains_key("absent"));
        // Keyed by repository: another checkout sees nothing.
        assert!(
            McpToolInventory::load(&data_dir, "repo_b", &["fake"])
                .servers
                .is_empty()
        );
        fs::remove_dir_all(data_dir).unwrap();
    }

    #[test]
    fn a_changed_definition_changes_the_content_hash() {
        let a = McpInventoryTool::from_tool(&tool("echo", "Echoes", "{}"), 0);
        let b = McpInventoryTool::from_tool(&tool("echo", "Echoes loudly", "{}"), 0);
        let mut claimed = tool("echo", "Echoes", "{}");
        claimed.read_only_hint = Some(true);
        let c = McpInventoryTool::from_tool(&claimed, 0);
        assert_ne!(a.content_hash, b.content_hash);
        assert_ne!(a.content_hash, c.content_hash);
    }

    #[test]
    fn a_corrupt_record_reads_as_no_inventory() {
        let data_dir = temp_data_dir("corrupt");
        let directory = McpToolInventory::directory(&data_dir, "repo_a");
        fs::create_dir_all(&directory).unwrap();
        fs::write(directory.join("fake.json"), "{not json").unwrap();
        assert!(McpToolInventory::load_server(&data_dir, "repo_a", "fake").is_none());
        fs::remove_dir_all(data_dir).unwrap();
    }

    fn inventory_with(servers: &[(&str, &[usize])]) -> McpToolInventory {
        let mut inventory = McpToolInventory {
            repository_id: "repo".to_string(),
            servers: BTreeMap::new(),
        };
        for (server_id, costs) in servers {
            let tools = costs
                .iter()
                .enumerate()
                .map(|(index, cost)| McpInventoryTool {
                    name: format!("t{index}"),
                    description: String::new(),
                    schema_bytes: 0,
                    estimated_tokens: *cost,
                    read_only_hint: None,
                    content_hash: String::new(),
                    first_seen_at_ms: 0,
                })
                .collect();
            inventory.servers.insert(
                (*server_id).to_string(),
                McpServerInventory {
                    server_id: (*server_id).to_string(),
                    refreshed_at_ms: 0,
                    tools,
                },
            );
        }
        inventory
    }

    #[test]
    fn under_the_ceiling_is_direct_and_over_it_is_deferred() {
        let inventory = inventory_with(&[("a", &[300, 200]), ("b", &[400])]);
        let under = decide_deferral(&inventory, &["a", "b"], 900, |_, _| true);
        assert_eq!(under.known_tokens, 900);
        assert_eq!(under.known_surface, McpToolSurface::Direct);
        assert_eq!(under.surface_for("a"), McpToolSurface::Direct);

        let over = decide_deferral(&inventory, &["a", "b"], 899, |_, _| true);
        assert_eq!(over.known_surface, McpToolSurface::Deferred);
        assert_eq!(over.surface_for("b"), McpToolSurface::Deferred);
    }

    #[test]
    fn a_server_with_no_inventory_is_unknown_cost_and_deferred() {
        let inventory = inventory_with(&[("a", &[100])]);
        let decision = decide_deferral(&inventory, &["a", "new"], 8_000, |_, _| true);
        assert_eq!(decision.unknown_servers, vec!["new".to_string()]);
        assert_eq!(decision.surface_for("a"), McpToolSurface::Direct);
        assert_eq!(decision.surface_for("new"), McpToolSurface::Deferred);
        assert_eq!(
            decision.surface_for("unconfigured"),
            McpToolSurface::Deferred
        );
    }

    #[test]
    fn only_offered_tools_count_against_the_ceiling() {
        let inventory = inventory_with(&[("a", &[5_000, 5_000])]);
        let all = decide_deferral(&inventory, &["a"], 8_000, |_, _| true);
        assert_eq!(all.known_surface, McpToolSurface::Deferred);
        let one = decide_deferral(&inventory, &["a"], 8_000, |_, tool| tool.name == "t0");
        assert_eq!(one.known_tokens, 5_000);
        assert_eq!(one.known_surface, McpToolSurface::Direct);
    }
}
