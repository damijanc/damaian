//! A minimal, synchronous [Model Context Protocol](https://modelcontextprotocol.io)
//! client. Deliberately avoids an async runtime to match the rest of
//! `workspace-engine` (HTTP is done by shelling out to `curl`, exactly as the
//! model transport does; local servers are ordinary child processes).
//!
//! Scope is intentionally narrow: the JSON-RPC `initialize` handshake plus
//! `tools/list` and `tools/call`. Resources, prompts, and sampling are not
//! implemented. Every operation is bounded by a timeout so a slow or wedged
//! server can never hang a chat turn.

use crate::audit::AuditLog;
use crate::config::{McpServerConfig, McpTransport};
use crate::error::{ClientError, Result};
use crate::model::{ToolDefinition, escape_curl_config_value};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::thread;
use std::time::{Duration, Instant};

/// Protocol revision we advertise in `initialize`. Servers negotiate down if
/// they only speak an older one.
const PROTOCOL_VERSION: &str = "2025-06-18";
const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
const CALL_TIMEOUT: Duration = Duration::from_secs(60);

/// The `mcp__` namespace prefix. Every MCP tool offered to the model is named
/// `mcp__<server_id>__<tool_name>` so it can't collide with a built-in tool or
/// a tool from another server.
pub const MCP_TOOL_PREFIX: &str = "mcp__";

pub fn namespaced_tool_name(server_id: &str, tool_name: &str) -> String {
    format!("{MCP_TOOL_PREFIX}{server_id}__{tool_name}")
}

/// Splits `mcp__<server_id>__<tool_name>` back into its parts. Server ids
/// forbid `_` (see [`crate::config::normalize_mcp_server_id`]), so the first
/// `__` after the prefix unambiguously ends the server id even when the tool
/// name itself contains underscores.
pub fn parse_namespaced_tool_name(name: &str) -> Option<(String, String)> {
    let rest = name.strip_prefix(MCP_TOOL_PREFIX)?;
    let (server_id, tool_name) = rest.split_once("__")?;
    if server_id.is_empty() || tool_name.is_empty() {
        return None;
    }
    Some((server_id.to_string(), tool_name.to_string()))
}

/// Splits a command line into tokens the way a shell would for the simple
/// cases: whitespace separates arguments, and single or double quotes (plus a
/// backslash escape) group a token so paths with spaces survive. Not a full
/// POSIX parser — just enough to let a pasted `python /path/server.py` line be
/// launched correctly.
fn shell_split(input: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut has_token = false;
    let mut in_single = false;
    let mut in_double = false;
    let mut chars = input.chars();
    while let Some(character) = chars.next() {
        match character {
            '\\' if !in_single => {
                if let Some(escaped) = chars.next() {
                    current.push(escaped);
                    has_token = true;
                }
            }
            '\'' if !in_double => {
                in_single = !in_single;
                has_token = true;
            }
            '"' if !in_single => {
                in_double = !in_double;
                has_token = true;
            }
            character if character.is_whitespace() && !in_single && !in_double => {
                if has_token {
                    tokens.push(std::mem::take(&mut current));
                    has_token = false;
                }
            }
            character => {
                current.push(character);
                has_token = true;
            }
        }
    }
    if has_token {
        tokens.push(current);
    }
    tokens
}

/// A tool discovered from a server's `tools/list`.
#[derive(Debug, Clone)]
pub struct McpTool {
    pub name: String,
    pub description: String,
    /// The tool's JSON-Schema `inputSchema`, serialized — maps directly onto
    /// [`ToolDefinition::parameters_json`].
    pub input_schema_json: String,
}

impl McpTool {
    pub fn to_tool_definition(&self, server_id: &str) -> ToolDefinition {
        ToolDefinition {
            name: namespaced_tool_name(server_id, &self.name),
            description: self.description.clone(),
            parameters_json: self.input_schema_json.clone(),
        }
    }
}

/// The flattened result of a `tools/call`.
#[derive(Debug, Clone)]
pub struct McpToolResult {
    pub text: String,
    /// True when the server marked the result as a tool-level error. The
    /// caller still feeds the text back to the model (so it can react), but
    /// can label it as a failure.
    pub is_error: bool,
}

// ---------------------------------------------------------------------------
// Client
// ---------------------------------------------------------------------------

/// A live connection to one MCP server. Dropping it tears down the underlying
/// transport (killing the child process for stdio servers).
pub struct McpClient {
    transport: Box<dyn RpcTransport>,
    next_id: u64,
}

impl McpClient {
    /// Connects and completes the `initialize` handshake. `auth_token` is the
    /// already-resolved bearer token for HTTP servers (the engine never reads
    /// the keychain itself — the caller resolves it and passes the value in).
    pub fn connect(config: &McpServerConfig, auth_token: Option<String>) -> Result<Self> {
        let transport: Box<dyn RpcTransport> = match config.transport {
            McpTransport::Stdio => Box::new(StdioTransport::spawn(config)?),
            McpTransport::Http => Box::new(HttpTransport::new(config, auth_token)?),
        };
        let mut client = Self {
            transport,
            next_id: 0,
        };
        client.initialize()?;
        Ok(client)
    }

    fn initialize(&mut self) -> Result<()> {
        let params = json!({
            "protocolVersion": PROTOCOL_VERSION,
            "capabilities": {},
            "clientInfo": { "name": "damaian-client", "version": env!("CARGO_PKG_VERSION") },
        });
        self.request("initialize", params, CONNECT_TIMEOUT)?;
        // Best-effort readiness notification. A server that rejects or ignores
        // it but is otherwise healthy should not be treated as unusable.
        let _ = self
            .transport
            .notify(&notification_payload("notifications/initialized"));
        Ok(())
    }

    pub fn list_tools(&mut self) -> Result<Vec<McpTool>> {
        let result = self.request("tools/list", json!({}), CONNECT_TIMEOUT)?;
        let mut tools = Vec::new();
        if let Some(items) = result.get("tools").and_then(Value::as_array) {
            for item in items {
                let Some(name) = item.get("name").and_then(Value::as_str) else {
                    continue;
                };
                let description = item
                    .get("description")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
                let input_schema = item
                    .get("inputSchema")
                    .cloned()
                    .unwrap_or_else(|| json!({ "type": "object" }));
                tools.push(McpTool {
                    name: name.to_string(),
                    description,
                    input_schema_json: input_schema.to_string(),
                });
            }
        }
        Ok(tools)
    }

    pub fn call_tool(&mut self, tool_name: &str, arguments_json: &str) -> Result<McpToolResult> {
        let arguments: Value =
            serde_json::from_str(arguments_json).unwrap_or_else(|_| json!({}));
        let params = json!({ "name": tool_name, "arguments": arguments });
        let result = self.request("tools/call", params, CALL_TIMEOUT)?;
        Ok(flatten_tool_result(&result))
    }

    fn request(&mut self, method: &str, params: Value, timeout: Duration) -> Result<Value> {
        self.next_id += 1;
        let id = self.next_id;
        let payload =
            json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }).to_string();
        let response = self.transport.call(&payload, id, timeout)?;
        if let Some(error) = response.get("error") {
            let message = error
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("unknown error");
            return Err(ClientError::Io(format!("MCP {method} failed: {message}")));
        }
        Ok(response.get("result").cloned().unwrap_or(Value::Null))
    }
}

fn notification_payload(method: &str) -> String {
    json!({ "jsonrpc": "2.0", "method": method, "params": {} }).to_string()
}

/// Collapses a `tools/call` result's content blocks into a single text blob
/// suitable for a tool-result message. Non-text blocks are noted rather than
/// dropped silently; a structured-only result falls back to its JSON.
fn flatten_tool_result(result: &Value) -> McpToolResult {
    let is_error = result
        .get("isError")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let mut text = String::new();
    if let Some(blocks) = result.get("content").and_then(Value::as_array) {
        for block in blocks {
            match block.get("type").and_then(Value::as_str) {
                Some("text") => {
                    if let Some(chunk) = block.get("text").and_then(Value::as_str) {
                        if !text.is_empty() {
                            text.push('\n');
                        }
                        text.push_str(chunk);
                    }
                }
                Some(other) => {
                    if !text.is_empty() {
                        text.push('\n');
                    }
                    text.push_str(&format!("[{other} content omitted]"));
                }
                None => {}
            }
        }
    }
    if text.is_empty() {
        if let Some(structured) = result.get("structuredContent") {
            text = structured.to_string();
        }
    }
    if text.is_empty() {
        text = result.to_string();
    }
    McpToolResult { text, is_error }
}

// ---------------------------------------------------------------------------
// Transports
// ---------------------------------------------------------------------------

/// A JSON-RPC transport. `call` sends a request and returns the parsed
/// response message whose `id` matches; `notify` fires a request-less
/// notification.
trait RpcTransport: Send {
    fn call(&mut self, payload: &str, id: u64, timeout: Duration) -> Result<Value>;
    fn notify(&mut self, payload: &str) -> Result<()>;
}

/// Local server: a child process speaking newline-delimited JSON-RPC over
/// stdin/stdout. A reader thread drains stdout into a channel so blocking
/// reads can be bounded by `recv_timeout`.
struct StdioTransport {
    child: Child,
    stdin: ChildStdin,
    lines: Receiver<String>,
}

impl StdioTransport {
    fn spawn(config: &McpServerConfig) -> Result<Self> {
        if config.command.trim().is_empty() {
            return Err(ClientError::InvalidInput(
                "MCP stdio server requires a command".to_string(),
            ));
        }
        // Tolerate a whole command line pasted into `command` (e.g. copied from
        // an opencode-style `"command": ["python", "server.py"]` array). When
        // no separate args are configured, shell-split `command` into the
        // executable plus its arguments; quotes protect paths with spaces.
        let (program, args) = if config.args.is_empty() {
            let mut tokens = shell_split(&config.command).into_iter();
            let Some(program) = tokens.next() else {
                return Err(ClientError::InvalidInput(
                    "MCP stdio server requires a command".to_string(),
                ));
            };
            (program, tokens.collect::<Vec<_>>())
        } else {
            (config.command.clone(), config.args.clone())
        };
        let mut command = Command::new(&program);
        command.args(&args);
        // Only the explicitly configured environment reaches the child — no
        // ambient secrets are inherited beyond what the user opted in to.
        for (key, value) in &config.env {
            command.env(key, value);
        }
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            // stderr is the MCP logging channel (stdout is reserved for the
            // protocol). Discard it so a chatty server can't fill the pipe and
            // block, and so it never pollutes the app's own stderr.
            .stderr(Stdio::null());
        let mut child = command.spawn().map_err(|error| {
            ClientError::Io(format!(
                "Failed to start MCP server '{}': {error}",
                config.command
            ))
        })?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| ClientError::Io("Failed to open MCP server stdin".to_string()))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| ClientError::Io("Failed to open MCP server stdout".to_string()))?;
        let (sender, lines) = mpsc::channel();
        thread::spawn(move || {
            let reader = BufReader::new(stdout);
            for line in reader.lines() {
                match line {
                    Ok(line) => {
                        if sender.send(line).is_err() {
                            break;
                        }
                    }
                    Err(_) => break,
                }
            }
        });
        Ok(Self {
            child,
            stdin,
            lines,
        })
    }

    fn write_line(&mut self, payload: &str) -> Result<()> {
        writeln!(self.stdin, "{payload}")
            .and_then(|_| self.stdin.flush())
            .map_err(|error| ClientError::Io(format!("Failed to write to MCP server: {error}")))
    }
}

impl RpcTransport for StdioTransport {
    fn call(&mut self, payload: &str, id: u64, timeout: Duration) -> Result<Value> {
        self.write_line(payload)?;
        let deadline = Instant::now() + timeout;
        loop {
            let remaining = deadline
                .checked_duration_since(Instant::now())
                .ok_or_else(|| ClientError::Io("Timed out waiting for MCP server".to_string()))?;
            let line = self.lines.recv_timeout(remaining).map_err(|_| {
                ClientError::Io("Timed out waiting for MCP server response".to_string())
            })?;
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            let Ok(value) = serde_json::from_str::<Value>(trimmed) else {
                continue;
            };
            // Ignore server-initiated notifications and responses to other ids.
            if value.get("id").and_then(Value::as_u64) == Some(id) {
                return Ok(value);
            }
        }
    }

    fn notify(&mut self, payload: &str) -> Result<()> {
        self.write_line(payload)
    }
}

impl Drop for StdioTransport {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Remote server: JSON-RPC over the Streamable-HTTP transport, driven by a
/// `curl` subprocess (like the model transport) so the bearer token stays out
/// of `argv`. Handles both a plain JSON response body and a `text/event-stream`
/// (SSE) body, and echoes the negotiated `Mcp-Session-Id` on later requests.
struct HttpTransport {
    url: String,
    auth_token: Option<String>,
    session_id: Option<String>,
}

impl HttpTransport {
    fn new(config: &McpServerConfig, auth_token: Option<String>) -> Result<Self> {
        if config.url.trim().is_empty() {
            return Err(ClientError::InvalidInput(
                "MCP http server requires a url".to_string(),
            ));
        }
        Ok(Self {
            url: config.url.clone(),
            auth_token: auth_token.filter(|token| !token.is_empty()),
            session_id: None,
        })
    }

    fn curl_config(&self, payload: &str, timeout: Duration) -> String {
        let mut config = String::new();
        config.push_str(&format!(
            "request = \"POST\"\nurl = \"{}\"\n",
            escape_curl_config_value(&self.url)
        ));
        config.push_str("header = \"content-type: application/json\"\n");
        config.push_str("header = \"accept: application/json, text/event-stream\"\n");
        if let Some(token) = &self.auth_token {
            config.push_str(&format!(
                "header = \"authorization: Bearer {}\"\n",
                escape_curl_config_value(token)
            ));
        }
        if let Some(session) = &self.session_id {
            config.push_str(&format!(
                "header = \"mcp-session-id: {}\"\n",
                escape_curl_config_value(session)
            ));
        }
        config.push_str(&format!(
            "data-binary = \"{}\"\n",
            escape_curl_config_value(payload)
        ));
        config.push_str(&format!("max-time = {}\n", timeout.as_secs().max(1)));
        config
    }

    /// Posts one JSON-RPC message. Returns the parsed response body, or `None`
    /// for an empty body (e.g. a notification the server accepts with 202).
    fn post(&mut self, payload: &str, timeout: Duration) -> Result<Option<Value>> {
        // `-D -` dumps response headers to stdout ahead of the body so we can
        // read the session id back out.
        let mut child = Command::new("curl")
            .args(["-sS", "-D", "-", "--config", "-"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|error| ClientError::Io(format!("Failed to run curl for MCP: {error}")))?;
        if let Some(mut stdin) = child.stdin.take() {
            stdin.write_all(self.curl_config(payload, timeout).as_bytes())?;
        }
        let mut raw = String::new();
        if let Some(mut stdout) = child.stdout.take() {
            stdout.read_to_string(&mut raw)?;
        }
        let status = child.wait()?;
        let mut stderr = String::new();
        if let Some(mut pipe) = child.stderr.take() {
            pipe.read_to_string(&mut stderr)?;
        }
        if !status.success() {
            return Err(ClientError::Io(format!(
                "MCP server request failed: {}",
                stderr.trim()
            )));
        }
        let (headers, body) = split_headers_body(&raw);
        if let Some(session) = header_value(headers, "mcp-session-id") {
            self.session_id = Some(session);
        }
        Ok(parse_rpc_body(body))
    }
}

impl RpcTransport for HttpTransport {
    fn call(&mut self, payload: &str, _id: u64, timeout: Duration) -> Result<Value> {
        self.post(payload, timeout)?
            .ok_or_else(|| ClientError::Io("Empty response from MCP server".to_string()))
    }

    fn notify(&mut self, payload: &str) -> Result<()> {
        let _ = self.post(payload, CONNECT_TIMEOUT)?;
        Ok(())
    }
}

/// Splits a raw curl `-D -` dump into (final header block, body). Skips any
/// interim/redirect header blocks so the body is what follows the last
/// `HTTP/...` status.
fn split_headers_body(raw: &str) -> (&str, &str) {
    let mut headers = "";
    let mut rest = raw;
    while rest.starts_with("HTTP/") {
        match rest.split_once("\r\n\r\n") {
            Some((head, body)) => {
                headers = head;
                rest = body;
            }
            None => {
                headers = rest;
                rest = "";
                break;
            }
        }
    }
    (headers, rest)
}

fn header_value(headers: &str, name: &str) -> Option<String> {
    headers.lines().find_map(|line| {
        let (key, value) = line.split_once(':')?;
        if key.trim().eq_ignore_ascii_case(name) {
            Some(value.trim().to_string())
        } else {
            None
        }
    })
}

/// Parses a response body that is either plain JSON or an SSE stream, returning
/// the last JSON-RPC message found.
fn parse_rpc_body(body: &str) -> Option<Value> {
    let trimmed = body.trim();
    if trimmed.is_empty() {
        return None;
    }
    if let Ok(value) = serde_json::from_str::<Value>(trimmed) {
        return Some(value);
    }
    let mut data = String::new();
    let mut found = None;
    let flush = |data: &mut String, found: &mut Option<Value>| {
        if !data.is_empty() {
            if let Ok(value) = serde_json::from_str::<Value>(data) {
                *found = Some(value);
            }
            data.clear();
        }
    };
    for line in trimmed.lines() {
        let line = line.trim_end();
        if let Some(rest) = line.strip_prefix("data:") {
            if !data.is_empty() {
                data.push('\n');
            }
            data.push_str(rest.trim_start());
        } else if line.is_empty() {
            flush(&mut data, &mut found);
        }
    }
    flush(&mut data, &mut found);
    found
}

// ---------------------------------------------------------------------------
// Runtime
// ---------------------------------------------------------------------------

/// One active server plus its already-resolved auth token.
pub struct McpServerRuntime {
    pub config: McpServerConfig,
    pub auth_token: Option<String>,
}

/// Per-turn MCP orchestration: lazily connects to the active servers, caches
/// their tool lists and connections for the life of the runtime, and tears
/// everything down on drop. Discovery and calls are best-effort — a failing
/// server is skipped (for discovery) or returns a structured error (for a
/// call) rather than aborting the chat turn.
pub struct McpRuntime {
    servers: Vec<McpServerRuntime>,
    connections: HashMap<String, McpClient>,
    tools: HashMap<String, Vec<McpTool>>,
    audit_log: Option<AuditLog>,
}

impl McpRuntime {
    pub fn new(servers: Vec<McpServerRuntime>, audit_log: AuditLog) -> Self {
        Self {
            servers,
            connections: HashMap::new(),
            tools: HashMap::new(),
            audit_log: Some(audit_log),
        }
    }

    /// An inert runtime with no servers — used by callers that don't wire MCP
    /// (the CLI, tests) and whenever MCP is disabled by config.
    pub fn disabled() -> Self {
        Self {
            servers: Vec::new(),
            connections: HashMap::new(),
            tools: HashMap::new(),
            audit_log: None,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.servers.is_empty()
    }

    fn server(&self, server_id: &str) -> Option<&McpServerRuntime> {
        self.servers.iter().find(|server| server.config.id == server_id)
    }

    pub fn requires_approval(&self, server_id: &str) -> bool {
        // Default to requiring approval for anything we can't identify.
        self.server(server_id)
            .map(|server| server.config.require_approval)
            .unwrap_or(true)
    }

    fn record(&self, event: &str, fields: &[(&str, String)]) {
        if let Some(log) = &self.audit_log {
            let mut entries = vec![("actor", "system".to_string())];
            entries.extend(fields.iter().map(|(k, v)| (*k, v.clone())));
            let _ = log.record(event, &entries);
        }
    }

    fn connection(&mut self, server_id: &str) -> Result<&mut McpClient> {
        if !self.connections.contains_key(server_id) {
            let Some(server) = self.server(server_id) else {
                return Err(ClientError::InvalidInput(format!(
                    "Unknown MCP server: {server_id}"
                )));
            };
            let client = McpClient::connect(&server.config, server.auth_token.clone())?;
            self.connections.insert(server_id.to_string(), client);
        }
        Ok(self.connections.get_mut(server_id).expect("just inserted"))
    }

    /// Discovers every active server's tools as ready-to-offer definitions.
    /// Connection/list failures are logged and skipped, never fatal.
    pub fn tool_definitions(&mut self) -> Vec<ToolDefinition> {
        let server_ids: Vec<String> = self
            .servers
            .iter()
            .map(|server| server.config.id.clone())
            .collect();
        let mut definitions = Vec::new();
        for server_id in server_ids {
            if !self.tools.contains_key(&server_id) {
                match self.connection(&server_id).and_then(McpClient::list_tools) {
                    Ok(tools) => {
                        self.record(
                            "mcp_tools_listed",
                            &[
                                ("server", server_id.clone()),
                                ("toolCount", tools.len().to_string()),
                            ],
                        );
                        self.tools.insert(server_id.clone(), tools);
                    }
                    Err(error) => {
                        self.record(
                            "mcp_discovery_failed",
                            &[("server", server_id.clone()), ("error", error.to_string())],
                        );
                        // Drop any half-open connection so a later call can retry.
                        self.connections.remove(&server_id);
                        self.tools.insert(server_id.clone(), Vec::new());
                    }
                }
            }
            if let Some(tools) = self.tools.get(&server_id) {
                for tool in tools {
                    definitions.push(tool.to_tool_definition(&server_id));
                }
            }
        }
        definitions
    }

    /// Executes a tool call, returning the flattened result. Errors are the
    /// caller's to convert into a tool-result message.
    pub fn call_tool(
        &mut self,
        server_id: &str,
        tool_name: &str,
        arguments_json: &str,
    ) -> Result<McpToolResult> {
        let result = self
            .connection(server_id)
            .and_then(|client| client.call_tool(tool_name, arguments_json));
        match &result {
            Ok(value) => self.record(
                "mcp_tool_called",
                &[
                    ("server", server_id.to_string()),
                    ("tool", tool_name.to_string()),
                    ("isError", value.is_error.to_string()),
                ],
            ),
            Err(error) => self.record(
                "mcp_tool_call_failed",
                &[
                    ("server", server_id.to_string()),
                    ("tool", tool_name.to_string()),
                    ("error", error.to_string()),
                ],
            ),
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn namespacing_round_trips() {
        let name = namespaced_tool_name("sentry", "search_issues");
        assert_eq!(name, "mcp__sentry__search_issues");
        let (server, tool) = parse_namespaced_tool_name(&name).unwrap();
        assert_eq!(server, "sentry");
        assert_eq!(tool, "search_issues");
    }

    #[test]
    fn shell_split_handles_plain_args_and_quoted_paths() {
        assert_eq!(
            shell_split("/venv/bin/python /path/server.py"),
            vec!["/venv/bin/python".to_string(), "/path/server.py".to_string()]
        );
        assert_eq!(
            shell_split("npx -y @scope/pkg /tmp"),
            vec![
                "npx".to_string(),
                "-y".to_string(),
                "@scope/pkg".to_string(),
                "/tmp".to_string()
            ]
        );
        // A quoted path with a space stays a single token.
        assert_eq!(
            shell_split("\"/Applications/My App/bin/tool\" --flag"),
            vec!["/Applications/My App/bin/tool".to_string(), "--flag".to_string()]
        );
        assert!(shell_split("   ").is_empty());
    }

    #[test]
    fn namespacing_rejects_non_mcp_names() {
        assert!(parse_namespaced_tool_name("run_command").is_none());
        assert!(parse_namespaced_tool_name("mcp__only").is_none());
    }

    #[test]
    fn flattens_text_content_blocks() {
        let result = json!({
            "content": [
                { "type": "text", "text": "line one" },
                { "type": "text", "text": "line two" },
            ],
        });
        let flattened = flatten_tool_result(&result);
        assert_eq!(flattened.text, "line one\nline two");
        assert!(!flattened.is_error);
    }

    #[test]
    fn flattens_error_and_non_text_blocks() {
        let result = json!({
            "isError": true,
            "content": [ { "type": "image", "data": "..." } ],
        });
        let flattened = flatten_tool_result(&result);
        assert!(flattened.is_error);
        assert_eq!(flattened.text, "[image content omitted]");
    }

    #[test]
    fn parses_sse_body() {
        let body = "event: message\r\ndata: {\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{\"ok\":true}}\r\n\r\n";
        let value = parse_rpc_body(body).unwrap();
        assert_eq!(value["result"]["ok"], json!(true));
    }

    #[test]
    fn parses_plain_json_body() {
        let value = parse_rpc_body("{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":42}").unwrap();
        assert_eq!(value["result"], json!(42));
    }

    #[test]
    fn splits_headers_from_body() {
        let raw = "HTTP/1.1 200 OK\r\nMcp-Session-Id: abc123\r\ncontent-type: application/json\r\n\r\n{\"result\":1}";
        let (headers, body) = split_headers_body(raw);
        assert_eq!(header_value(headers, "mcp-session-id").as_deref(), Some("abc123"));
        assert_eq!(body, "{\"result\":1}");
    }
}
