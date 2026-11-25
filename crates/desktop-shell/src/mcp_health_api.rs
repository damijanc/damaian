//! `GET /api/mcp-health` and the manual retry on `/api/mcp-test` (spec 33
//! Task 7, `context.md` D5).
//!
//! MCP health is kept in memory by whichever process contacted the server:
//! each session host for its own session's turns, each worker for its task,
//! and the shell for what it contacted itself (a settings retry). The shell
//! cannot read another process's memory, so the route asks every live
//! session host and worker for its snapshot over the same socket `status`
//! uses, adds its own, and keeps the latest observation per server. A
//! configured server nobody has contacted is reported `unknown` — never
//! contacted is not the same as healthy.
//!
//! The retry has two halves. A `Failed` latch lives in the process whose
//! turns tripped it, so the shell asks every host and worker to forget that
//! server's health (`mcp_retry`) — the next turn that wants it contacts it
//! again — and then contacts the server itself, at once, so the user sees
//! the outcome. Neither half ever carries a token value: the route reports
//! the auth *reference* and the outcome.

use serde_json::{Map, Value, json};
use workspace_engine::{
    Config, McpHealthObservation, McpRuntime, McpServerConfig, McpServerHealth, McpServerRuntime,
    McpTimeouts, McpTransport, SecretScanner, clear_mcp_health, mcp_health_snapshot,
    mcp_server_health, merge_mcp_health, repository_id_for_root,
};

use std::collections::HashMap;
use std::io::Write;
use std::time::Duration;

use crate::http::{Request, required_param, write_response};

/// A host's or worker's answer to `mcp_health`.
pub(crate) fn snapshot_answer() -> Value {
    json!({"ok": true, "observations": mcp_health_snapshot()})
}

/// A host's or worker's answer to `mcp_retry`: what it knew before forgetting.
pub(crate) fn retry_answer(repository_id: &str, server_id: &str) -> Value {
    json!({"ok": true, "was": clear_mcp_health(repository_id, server_id)})
}

/// The observations in one `mcp_health` answer; anything malformed is
/// skipped, since health is advisory.
fn observations_in(answer: &Value) -> Vec<McpHealthObservation> {
    answer["observations"]
        .as_array()
        .map(|rows| {
            rows.iter()
                .filter_map(|row| serde_json::from_value(row.clone()).ok())
                .collect()
        })
        .unwrap_or_default()
}

/// Every process's observations, merged: the shell's own, every live session
/// host's and every live worker's. A process that does not answer in time
/// contributes nothing, so its servers read as what the others saw.
fn gathered_health() -> Vec<McpHealthObservation> {
    let mut all = mcp_health_snapshot();
    for answer in crate::session_hosts::ask_every_host("mcp_health", Map::new())
        .iter()
        .chain(crate::workers::ask_every_worker("mcp_health", Map::new()).iter())
    {
        all.extend(observations_in(answer));
    }
    merge_mcp_health(all)
}

fn transport_name(transport: McpTransport) -> &'static str {
    match transport {
        McpTransport::Stdio => "stdio",
        McpTransport::Http => "http",
    }
}

/// The server's auth reference as the API shows it: the reference itself
/// (`keychain:github-token`, an environment variable name), through the
/// secret scanner in case a raw token was put where a reference belongs.
fn auth_reference(server: &McpServerConfig, scanner: &SecretScanner) -> Value {
    let reference = server.auth_token_env.trim();
    if reference.is_empty() {
        Value::Null
    } else {
        Value::from(scanner.redact(reference).text)
    }
}

/// The JSON `/api/mcp-health` serves for one repository: every configured
/// server, with the latest health any process observed, or `unknown`.
pub(crate) fn mcp_health_json(
    servers: &[McpServerConfig],
    repository_id: &str,
    observations: &[McpHealthObservation],
    scanner: &SecretScanner,
) -> Value {
    let rows = servers
        .iter()
        .map(|server| {
            let seen = observations.iter().find(|observation| {
                observation.repository_id == repository_id && observation.server_id == server.id
            });
            json!({
                "id": server.id,
                "label": server.label,
                "transport": transport_name(server.transport),
                "enabled": server.enabled,
                "authReference": auth_reference(server, scanner),
                "health": seen.map_or(McpServerHealth::Unknown, |seen| seen.health),
                "observedAtMs": seen.map(|seen| seen.observed_at_ms),
                "sessionId": seen
                    .map(|seen| seen.session_id.as_str())
                    .filter(|session| !session.is_empty()),
                "lastFailure": seen.and_then(|seen| seen.last_failure),
                "consecutiveFailures": seen.map_or(0, |seen| seen.consecutive_failures),
                "reason": seen.and_then(McpHealthObservation::failed_reason),
            })
        })
        .collect::<Vec<_>>();
    json!({"repositoryId": repository_id, "servers": rows})
}

pub(crate) fn handle_mcp_health(stream: &mut dyn Write, request: &Request) -> Result<(), String> {
    let repo = required_param(request, "repo")?;
    let root = crate::validate_working_folder(&repo)?;
    let config = crate::config_for_repo(&repo)?;
    let scanner = SecretScanner::new(config.secret_patterns.clone());
    let body = mcp_health_json(
        &config.mcp_servers,
        &repository_id_for_root(&root),
        &gathered_health(),
        &scanner,
    );
    write_response(stream, request, 200, "application/json", &body.to_string())
}

fn timeouts(config: &Config) -> McpTimeouts {
    McpTimeouts {
        connect: Duration::from_millis(config.mcp_connect_timeout_ms),
        list: Duration::from_millis(config.mcp_list_timeout_ms),
        call: Duration::from_millis(config.mcp_call_timeout_ms),
    }
}

/// The manual retry (`/api/mcp-test` with `retry=true`, `repo` and `id`):
/// forgets the server's health in every process — the `Failed` latch
/// included — then contacts the saved server once from the shell and reports
/// what happened. The server is the one configured for the repository, not
/// the editor's unsaved fields, so the outcome is about what sessions use.
pub(crate) fn mcp_retry_json(form: &HashMap<String, String>) -> Result<Value, String> {
    let repo = crate::required_form(form, "repo")?;
    let server_id = crate::required_form(form, "id")?;
    let root = crate::validate_working_folder(&repo)?;
    let config = crate::config_for_repo(&repo)?;
    let server = config
        .mcp_servers
        .iter()
        .find(|server| server.id == server_id)
        .cloned()
        .ok_or_else(|| format!("No MCP server `{server_id}` is configured for this repository."))?;
    let repository_id = repository_id_for_root(&root);

    let mut arguments = Map::new();
    arguments.insert(
        "repositoryId".to_string(),
        Value::from(repository_id.clone()),
    );
    arguments.insert("serverId".to_string(), Value::from(server_id.clone()));
    let was_failed_in = crate::session_hosts::ask_every_host("mcp_retry", arguments.clone())
        .into_iter()
        .chain(crate::workers::ask_every_worker("mcp_retry", arguments))
        .filter(|answer| answer["was"] == "failed")
        .count()
        + usize::from(mcp_server_health(&repository_id, &server_id) == McpServerHealth::Failed);

    let scanner = SecretScanner::new(config.secret_patterns.clone());
    let reference = auth_reference(&server, &scanner);
    // As a turn does (`build_mcp_runtime`): only a remote server takes a
    // token, resolved here and never written anywhere.
    let auth_token = (server.transport == McpTransport::Http
        && !server.auth_token_env.trim().is_empty())
    .then(|| crate::resolve_mcp_token(&server.auth_token_env))
    .flatten();
    let mut runtime = McpRuntime::new(
        vec![McpServerRuntime {
            config: server,
            auth_token,
        }],
        crate::host::host_audit_log(&config),
        config.data_dir.clone(),
        // No session: the user is on the settings screen, not in a turn.
        "",
        &repository_id,
        timeouts(&config),
    );
    let outcome = runtime.retry_server(&server_id);
    drop(runtime);
    let health = mcp_server_health(&repository_id, &server_id);
    Ok(match outcome {
        Ok(tools) => json!({
            "ok": true,
            "retried": true,
            "serverId": server_id,
            "health": health,
            "authReference": reference,
            "wasFailedIn": was_failed_in,
            "toolCount": tools.len(),
            "tools": tools,
        }),
        Err(error) => {
            let error = scanner.redact(&error).text;
            let error = match (health, reference.as_str()) {
                (McpServerHealth::Unauthenticated, Some(reference)) => {
                    format!("Authentication failed for `{reference}`: {error}")
                }
                _ => error,
            };
            json!({
                "ok": false,
                "retried": true,
                "serverId": server_id,
                "health": health,
                "authReference": reference,
                "wasFailedIn": was_failed_in,
                "error": error,
            })
        }
    })
}
