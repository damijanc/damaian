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
use crate::cancel::CancelToken;
use crate::command_runner::kill_process_group;
use crate::config::{McpServerConfig, McpTransport};
use crate::error::{ClientError, Result};
use crate::mcp_health::{
    McpContactPhase, clear_mcp_health, health_after_failure, mcp_health_observation,
    record_mcp_health,
};
use crate::mcp_inventory::McpToolInventory;
use crate::model::{ToolDefinition, escape_curl_config_value};
use crate::process_registry::{ProcessKind, ProcessRegistry, RegistrationHandle};
use crate::secret_scanner::SecretScanner;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::thread;
use std::time::{Duration, Instant};

/// Protocol revision we advertise in `initialize`. Servers negotiate down if
/// they only speak an older one.
const PROTOCOL_VERSION: &str = "2025-06-18";

/// Health of one MCP server within this session (spec 33 §5.3, context D5).
/// Held in memory by [`crate::mcp_health`], keyed by repository and server,
/// and never persisted: the states are defined in terms of *this session*
/// (`Unknown` = "never contacted this session", `Failed` = "not retried
/// automatically this session"), so persisting them would contradict their
/// own semantics.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum McpServerHealth {
    /// Never contacted this session.
    Unknown,
    /// Reachable and handshaked successfully.
    Ready,
    /// Reachable, but authentication failed.
    Unauthenticated,
    /// Startup, `tools/list`, or `tools/call` exceeded its timeout.
    Timeout,
    /// Could not be reached (spawn, network, or handshake failure).
    Unreachable,
    /// Failed repeatedly; not retried automatically this session.
    Failed,
}

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
pub(crate) fn shell_split(input: &str) -> Vec<String> {
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
    /// The server's own claim about whether calling this tool has side
    /// effects, from `tools/list`'s optional `annotations.readOnlyHint`.
    /// `None` means the server made no claim either way — not "not
    /// read-only" and not "read-only". A hint, per the MCP spec, not a
    /// guarantee; `mode_permits` (`mode.rs`) is what turns it into an
    /// enforced boundary, and only a `Some(true)` widens what a
    /// capability-restricted mode offers.
    pub read_only_hint: Option<bool>,
}

impl McpTool {
    /// The tool with its description and schema passed through the secret
    /// scanner (spec 33 §5.6, D9). Both are server-authored text that becomes
    /// model context, so a credential a server embeds in them is redacted
    /// like any other context. The name is left alone: it is how calls are
    /// routed, and a name a scanner would match is not a callable tool.
    pub fn redacted(&self, scanner: &SecretScanner) -> McpTool {
        McpTool {
            name: self.name.clone(),
            description: scanner.redact(&self.description).text,
            input_schema_json: redact_json_text(scanner, &self.input_schema_json),
            read_only_hint: self.read_only_hint,
        }
    }

    /// The definition offered to the model. It takes the scanner so that no
    /// MCP schema can reach a request unscanned — including one served from
    /// an inventory recorded before listings were redacted.
    pub fn to_tool_definition(&self, server_id: &str, scanner: &SecretScanner) -> ToolDefinition {
        let tool = self.redacted(scanner);
        ToolDefinition {
            name: namespaced_tool_name(server_id, &tool.name),
            description: tool.description,
            parameters_json: tool.input_schema_json,
        }
    }
}

/// Redacts secrets inside a JSON document string by string, so the result
/// stays the same valid JSON: scanning the raw text could match across a
/// quote and replace it, breaking the schema. Text that is not JSON is
/// redacted as plain text.
pub fn redact_json_text(scanner: &SecretScanner, text: &str) -> String {
    if !scanner.contains_secrets(text) {
        return text.to_string();
    }
    match serde_json::from_str::<Value>(text) {
        Ok(mut value) => {
            redact_json_value(scanner, &mut value);
            value.to_string()
        }
        Err(_) => scanner.redact(text).text,
    }
}

fn redact_json_value(scanner: &SecretScanner, value: &mut Value) {
    match value {
        Value::String(text) => *text = scanner.redact(text).text,
        Value::Array(items) => {
            for item in items {
                redact_json_value(scanner, item);
            }
        }
        Value::Object(map) => {
            let entries = std::mem::take(map);
            for (key, mut item) in entries {
                redact_json_value(scanner, &mut item);
                map.insert(scanner.redact(&key).text, item);
            }
        }
        Value::Null | Value::Bool(_) | Value::Number(_) => {}
    }
}

/// How many characters of a call's arguments or result the invocation
/// history keeps (spec 33 §5.7): enough to see what was asked and answered,
/// bounded so one large result cannot swell the audit log.
pub const MCP_HISTORY_EXCERPT_CHARS: usize = 2_000;

/// The invocation-history copy of a call's arguments or result: redacted
/// first, then bounded — in that order, so the cut never leaves part of a
/// secret the scanner would no longer recognise. `AuditLog::record`
/// redacts again on the way in.
pub fn mcp_history_excerpt(scanner: &SecretScanner, text: &str) -> String {
    let redacted = scanner.redact(text).text;
    let total = redacted.chars().count();
    if total <= MCP_HISTORY_EXCERPT_CHARS {
        return redacted;
    }
    let kept: String = redacted.chars().take(MCP_HISTORY_EXCERPT_CHARS).collect();
    format!(
        "{kept}… [{} more characters not kept]",
        total - MCP_HISTORY_EXCERPT_CHARS
    )
}

/// How a call got past the approval step, for the invocation history
/// (spec 33 §5.7, D8). A declined call never reaches `call_tool`; the resume
/// path records it as `mcp_tool_call_declined`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum McpApprovalDecision {
    /// The user waived approval for this tool (or its server) in config.
    NotRequired,
    /// The user approved this call on its card.
    Approved,
}

impl McpApprovalDecision {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::NotRequired => "not_required",
            Self::Approved => "approved",
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
    pub fn connect(
        config: &McpServerConfig,
        auth_token: Option<String>,
        registry: &ProcessRegistry,
        session_id: &str,
        connect_timeout: Duration,
    ) -> Result<Self> {
        let transport: Box<dyn RpcTransport> = match config.transport {
            McpTransport::Stdio => Box::new(StdioTransport::spawn(config, registry, session_id)?),
            McpTransport::Http => Box::new(HttpTransport::new(config, auth_token)?),
        };
        let mut client = Self {
            transport,
            next_id: 0,
        };
        client.initialize(connect_timeout)?;
        Ok(client)
    }

    fn initialize(&mut self, connect_timeout: Duration) -> Result<()> {
        let params = json!({
            "protocolVersion": PROTOCOL_VERSION,
            "capabilities": {},
            "clientInfo": { "name": "damaian-client", "version": env!("CARGO_PKG_VERSION") },
        });
        self.request("initialize", params, connect_timeout)?;
        // Best-effort readiness notification. A server that rejects or ignores
        // it but is otherwise healthy should not be treated as unusable.
        let _ = self.transport.notify(
            &notification_payload("notifications/initialized"),
            connect_timeout,
        );
        Ok(())
    }

    pub fn list_tools(&mut self, list_timeout: Duration) -> Result<Vec<McpTool>> {
        let result = self.request("tools/list", json!({}), list_timeout)?;
        let tools = result
            .get("tools")
            .and_then(Value::as_array)
            .map(|items| items.iter().filter_map(parse_mcp_tool).collect())
            .unwrap_or_default();
        Ok(tools)
    }

    pub fn call_tool(
        &mut self,
        tool_name: &str,
        arguments_json: &str,
        call_timeout: Duration,
    ) -> Result<McpToolResult> {
        let arguments: Value = serde_json::from_str(arguments_json).unwrap_or_else(|_| json!({}));
        let params = json!({ "name": tool_name, "arguments": arguments });
        let result = self.request("tools/call", params, call_timeout)?;
        Ok(flatten_tool_result(&result))
    }

    fn request(&mut self, method: &str, params: Value, timeout: Duration) -> Result<Value> {
        self.next_id += 1;
        let id = self.next_id;
        let payload =
            json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }).to_string();
        let response = match self.transport.call(&payload, id, timeout) {
            Ok(response) => response,
            // Re-tag a transport-level timeout with the phase that hit it, so
            // the error names connect/list/call rather than the shared
            // transport wording.
            Err(ClientError::McpTimeout(_)) => {
                return Err(ClientError::McpTimeout(format!(
                    "Timed out waiting for MCP server response ({method})"
                )));
            }
            Err(error) => return Err(error),
        };
        if let Some(error) = response.get("error") {
            let message = error
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("unknown error");
            // `InvalidInput`, not `Io`: the server answered and refused this
            // request, which says it is up (spec 33 §5.3 health), and an
            // answered refusal is no transport failure to retry.
            return Err(ClientError::InvalidInput(format!(
                "MCP {method} failed: {message}"
            )));
        }
        Ok(response.get("result").cloned().unwrap_or(Value::Null))
    }
}

/// Parses one `tools/list` array item into an [`McpTool`], or `None` when the
/// item has no `name` (the only field this repository treats as required).
pub(crate) fn parse_mcp_tool(item: &Value) -> Option<McpTool> {
    let name = item.get("name").and_then(Value::as_str)?.to_string();
    let description = item
        .get("description")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let input_schema = item
        .get("inputSchema")
        .cloned()
        .unwrap_or_else(|| json!({ "type": "object" }));
    let read_only_hint = item
        .get("annotations")
        .and_then(|annotations| annotations.get("readOnlyHint"))
        .and_then(Value::as_bool);
    Some(McpTool {
        name,
        description,
        input_schema_json: input_schema.to_string(),
        read_only_hint,
    })
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
    if text.is_empty()
        && let Some(structured) = result.get("structuredContent")
    {
        text = structured.to_string();
    }
    if text.is_empty() {
        text = result.to_string();
    }
    McpToolResult { text, is_error }
}

// ---------------------------------------------------------------------------
// Transports
// ---------------------------------------------------------------------------

/// How often a waiting call looks at the turn's Stop.
const CANCEL_POLL_INTERVAL: Duration = Duration::from_millis(50);

/// A JSON-RPC transport. `call` sends a request and returns the parsed
/// response message whose `id` matches; `notify` fires a request-less
/// notification.
trait RpcTransport: Send {
    fn call(&mut self, payload: &str, id: u64, timeout: Duration) -> Result<Value>;
    fn notify(&mut self, payload: &str, timeout: Duration) -> Result<()>;
}

/// Local server: a child process speaking newline-delimited JSON-RPC over
/// stdin/stdout. A reader thread drains stdout into a channel so blocking
/// reads can be bounded by `recv_timeout`.
struct StdioTransport {
    child: Child,
    stdin: ChildStdin,
    lines: Receiver<String>,
    /// Removes this server's registry entry when the transport is dropped.
    /// Declared last so it is dropped after the child has been killed.
    _registration: RegistrationHandle,
}

impl StdioTransport {
    fn spawn(
        config: &McpServerConfig,
        registry: &ProcessRegistry,
        session_id: &str,
    ) -> Result<Self> {
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
            .stderr(Stdio::null())
            // Its own group, so the sweep can reach a server that exec'd a
            // worker — `npx` leaving a `node` behind is the common case.
            .process_group(0);
        let mut child = command.spawn().map_err(|error| {
            ClientError::Io(format!(
                "Failed to start MCP server '{}': {error}",
                config.command
            ))
        })?;
        // Before anything else touches the child: a server that is running and
        // unrecorded is exactly what requirement 3 forbids.
        let registration = registry.register(ProcessKind::McpServer, session_id, child.id())?;
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
            _registration: registration,
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
        // The turn's Stop (spec 109 D8). The server may be part-way through
        // the call, so stopping it ends the server's process group: what the
        // call did is unknown, and nothing re-sends it.
        let cancel = CancelToken::current();
        loop {
            if cancel.is_cancelled() {
                kill_process_group(self.child.id());
                return Err(ClientError::Cancelled);
            }
            let remaining = deadline
                .checked_duration_since(Instant::now())
                .ok_or_else(|| {
                    ClientError::McpTimeout("Timed out waiting for MCP server response".to_string())
                })?;
            let line = match self.lines.recv_timeout(remaining.min(CANCEL_POLL_INTERVAL)) {
                Ok(line) => line,
                Err(mpsc::RecvTimeoutError::Timeout) => continue,
                // The server closed its output — it exited. That is no
                // timeout (spec 33 Task 6): the server is gone, not slow, so
                // its health is `Unreachable`. The wording stays clear of
                // `is_retryable_message`'s words, so nothing re-sends a call
                // the server may have acted on before exiting.
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    return Err(ClientError::Io(
                        "The MCP server exited before answering".to_string(),
                    ));
                }
            };
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

    fn notify(&mut self, payload: &str, _timeout: Duration) -> Result<()> {
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
        // Read on its own thread so the turn's Stop (spec 109 D8) can end
        // the request while the server is still answering.
        let stdout = child.stdout.take();
        let (sender, read) = mpsc::channel();
        thread::spawn(move || {
            let mut raw = String::new();
            if let Some(mut stdout) = stdout {
                let _ = stdout.read_to_string(&mut raw);
            }
            let _ = sender.send(raw);
        });
        let cancel = CancelToken::current();
        let raw = loop {
            if cancel.is_cancelled() {
                // curl is the only process; ending it closes the reader's pipe.
                let _ = child.kill();
                let _ = child.wait();
                return Err(ClientError::Cancelled);
            }
            match read.recv_timeout(CANCEL_POLL_INTERVAL) {
                Ok(raw) => break raw,
                Err(mpsc::RecvTimeoutError::Timeout) => continue,
                Err(mpsc::RecvTimeoutError::Disconnected) => break String::new(),
            }
        };
        let status = child.wait()?;
        let mut stderr = String::new();
        if let Some(mut pipe) = child.stderr.take() {
            pipe.read_to_string(&mut stderr)?;
        }
        if !status.success() {
            // curl exit 28 is "operation timed out": the request may or may
            // not have reached the server, so it is an unknown outcome rather
            // than an ordinary transport failure.
            if status.code() == Some(28) {
                return Err(ClientError::McpTimeout(
                    "Timed out waiting for MCP server response".to_string(),
                ));
            }
            return Err(ClientError::Io(format!(
                "MCP server request failed: {}",
                stderr.trim()
            )));
        }
        let (headers, body) = split_headers_body(&raw);
        if let Some(status) = refused_credentials_status(headers) {
            // The token itself is never part of the message.
            return Err(ClientError::AccessDenied(format!(
                "MCP server refused the credentials (HTTP {status})"
            )));
        }
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

    fn notify(&mut self, payload: &str, timeout: Duration) -> Result<()> {
        let _ = self.post(payload, timeout)?;
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

/// The status of a final response that refused the request's credentials —
/// 401 or 403 — read from its status line, or `None` for any other response.
fn refused_credentials_status(headers: &str) -> Option<u16> {
    let status: u16 = headers
        .lines()
        .next()?
        .strip_prefix("HTTP/")?
        .split_whitespace()
        .nth(1)?
        .parse()
        .ok()?;
    matches!(status, 401 | 403).then_some(status)
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

/// How the audit log records a tool's `readOnlyHint`: what the server said,
/// including that it said nothing. `claimed_read_only` is the server's word,
/// not a verified property of the tool.
fn read_only_claim_label(hint: Option<bool>) -> &'static str {
    match hint {
        Some(true) => "claimed_read_only",
        Some(false) => "claimed_not_read_only",
        None => "no_claim",
    }
}

/// The three independently configured MCP budgets (spec 33 §5.4, D6).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct McpTimeouts {
    pub connect: Duration,
    pub list: Duration,
    pub call: Duration,
}

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
    /// Redacts server-authored descriptions and schemas, and the bounded
    /// invocation-history copies (spec 33 §5.6) — the audit log's own scanner.
    scanner: SecretScanner,
    /// Where to open the process registry when a stdio server is spawned. The
    /// registry is opened per connection rather than held, because opening it
    /// can fail and `build_mcp_runtime` cannot.
    data_dir: Option<PathBuf>,
    /// Attribution for any server this runtime spawns.
    session_id: String,
    /// The repository whose tool inventory a successful `tools/list`
    /// refreshes (spec 33 D4). Empty for an inert runtime.
    repository_id: String,
    connect_timeout: Duration,
    list_timeout: Duration,
    call_timeout: Duration,
}

impl McpRuntime {
    pub fn new(
        servers: Vec<McpServerRuntime>,
        audit_log: AuditLog,
        data_dir: PathBuf,
        session_id: &str,
        repository_id: &str,
        timeouts: McpTimeouts,
    ) -> Self {
        Self {
            servers,
            connections: HashMap::new(),
            tools: HashMap::new(),
            scanner: audit_log.scanner().clone(),
            audit_log: Some(audit_log),
            data_dir: Some(data_dir),
            session_id: session_id.to_string(),
            repository_id: repository_id.to_string(),
            connect_timeout: timeouts.connect,
            list_timeout: timeouts.list,
            call_timeout: timeouts.call,
        }
    }

    /// An inert runtime with no servers — used by callers that don't wire MCP
    /// (the CLI, tests) and whenever MCP is disabled by config.
    pub fn disabled() -> Self {
        Self {
            servers: Vec::new(),
            connections: HashMap::new(),
            tools: HashMap::new(),
            scanner: SecretScanner::default(),
            audit_log: None,
            data_dir: None,
            session_id: String::new(),
            repository_id: String::new(),
            connect_timeout: Duration::ZERO,
            list_timeout: Duration::ZERO,
            call_timeout: Duration::ZERO,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.servers.is_empty()
    }

    fn server(&self, server_id: &str) -> Option<&McpServerRuntime> {
        self.servers
            .iter()
            .find(|server| server.config.id == server_id)
    }

    /// The server's own read-only claim for one of its tools, from the
    /// cached `tools/list` response — `None` both when the server made no
    /// claim and when the tool or server is unknown to this runtime. Callers
    /// that need to distinguish "unknown" from "known but unclaimed" should
    /// not use this method; nothing downstream needs that distinction today.
    pub fn tool_read_only_hint(&self, server_id: &str, tool_name: &str) -> Option<bool> {
        self.tools
            .get(server_id)?
            .iter()
            .find(|tool| tool.name == tool_name)?
            .read_only_hint
    }

    fn record(&self, event: &str, fields: &[(&str, String)]) {
        if let Some(log) = &self.audit_log {
            let mut entries = vec![("actor", "system".to_string())];
            entries.extend(fields.iter().map(|(k, v)| (*k, v.clone())));
            let _ = log.record(event, &entries);
        }
    }

    /// Records what a contact with `server_id` showed in this process's
    /// health store (spec 33 §5.3, D5): `Ready` on success, otherwise what
    /// the failure implies. A stopped turn records nothing.
    fn record_health(&self, server_id: &str, phase: McpContactPhase, error: Option<&ClientError>) {
        let health = match error {
            None => Some(McpServerHealth::Ready),
            Some(error) => health_after_failure(phase, error),
        };
        if let Some(health) = health {
            record_mcp_health(&self.repository_id, server_id, &self.session_id, health);
        }
    }

    /// Why `server_id` is latched `Failed` in this process (spec 33 §5.3),
    /// or `None` when it may be contacted. A `Failed` server is not retried
    /// automatically for the rest of the session and its tools are not
    /// offered; only [`Self::retry_server`] or the settings retry clears it.
    pub fn failed_reason(&self, server_id: &str) -> Option<String> {
        if self.repository_id.is_empty() {
            return None;
        }
        mcp_health_observation(&self.repository_id, server_id)?.failed_reason()
    }

    /// Whether `server_id` is latched `Failed` in this process.
    pub fn is_failed(&self, server_id: &str) -> bool {
        self.failed_reason(server_id).is_some()
    }

    /// The manual retry (spec 33 §5.3): forgets this process's health for
    /// `server_id`, a `Failed` latch included, then connects and lists it once
    /// more, recording the outcome and refreshing the inventory as any listing
    /// does. Returns the tool names, or what went wrong — never a token value.
    pub fn retry_server(&mut self, server_id: &str) -> std::result::Result<Vec<String>, String> {
        if !self.has_server(server_id) {
            return Err(format!("Unknown MCP server: {server_id}"));
        }
        if !self.repository_id.is_empty() {
            clear_mcp_health(&self.repository_id, server_id);
        }
        self.tools.remove(server_id);
        self.connections.remove(server_id);
        self.list_now(server_id)?;
        Ok(self
            .tools
            .get(server_id)
            .map(|tools| tools.iter().map(|tool| tool.name.clone()).collect())
            .unwrap_or_default())
    }

    fn connection(&mut self, server_id: &str) -> Result<&mut McpClient> {
        if !self.connections.contains_key(server_id) {
            let Some(server) = self.server(server_id) else {
                return Err(ClientError::InvalidInput(format!(
                    "Unknown MCP server: {server_id}"
                )));
            };
            // An inert runtime has no servers, so this is unreachable there —
            // but a stdio server must never be spawned without somewhere to
            // record it, so the absence is an error rather than a silent skip.
            let Some(data_dir) = &self.data_dir else {
                return Err(ClientError::InvalidInput(
                    "MCP runtime has no data directory to record server processes in".to_string(),
                ));
            };
            let registry = ProcessRegistry::open(data_dir)?;
            let client = McpClient::connect(
                &server.config,
                server.auth_token.clone(),
                &registry,
                &self.session_id,
                self.connect_timeout,
            )?;
            self.connections.insert(server_id.to_string(), client);
        }
        Ok(self.connections.get_mut(server_id).expect("just inserted"))
    }

    /// The ids of every server this runtime may contact, in config order.
    pub fn server_ids(&self) -> Vec<String> {
        self.servers
            .iter()
            .map(|server| server.config.id.clone())
            .collect()
    }

    /// Whether `server_id` is one of this runtime's active servers.
    pub fn has_server(&self, server_id: &str) -> bool {
        self.server(server_id).is_some()
    }

    /// Whether `server_id` has been listed by this runtime — successfully or
    /// not. A listed server is the only kind that has started a process.
    pub fn is_listed(&self, server_id: &str) -> bool {
        self.tools.contains_key(server_id)
    }

    /// Connects to and lists one server, once per runtime, and returns its
    /// tools. This is the only place a listing happens, so it is also the only
    /// place the inventory is refreshed (spec 33 D4): a deferred server that is
    /// never searched, called or pinned is never contacted (§5.1, §5.3). A
    /// connection or list failure is audited and reads as no tools, never
    /// fatal; the failed listing is cached for the runtime's life.
    pub fn list_server(&mut self, server_id: &str) -> &[McpTool] {
        // A `Failed` server is not retried automatically (§5.3): it reads as
        // no tools, contacts nothing, and records nothing more.
        if !self.tools.contains_key(server_id) && self.is_failed(server_id) {
            self.tools.insert(server_id.to_string(), Vec::new());
        }
        if !self.tools.contains_key(server_id) && self.has_server(server_id) {
            let _ = self.list_now(server_id);
        }
        self.tools.get(server_id).map(Vec::as_slice).unwrap_or(&[])
    }

    /// Connects to and lists one server now, records its health, and caches
    /// the result for the runtime's life — the tools, or no tools after a
    /// failure, which is audited. Returns the failure's message (never a
    /// token value).
    fn list_now(&mut self, server_id: &str) -> std::result::Result<(), String> {
        let list_timeout = self.list_timeout;
        let listing = self
            .connection(server_id)
            .and_then(|client| client.list_tools(list_timeout));
        // A server that cannot list its tools is as unusable as one that
        // cannot be reached, so both phases read as one contact.
        self.record_health(server_id, McpContactPhase::Connect, listing.as_ref().err());
        match listing {
            Ok(tools) => {
                // Redacted once here, so the cached listing, the inventory on
                // disk, search results and offered definitions never carry a
                // secret the server embedded (§5.6, D9).
                let tools: Vec<McpTool> = tools
                    .iter()
                    .map(|tool| tool.redacted(&self.scanner))
                    .collect();
                self.record(
                    "mcp_tools_listed",
                    &[
                        ("server", server_id.to_string()),
                        ("toolCount", tools.len().to_string()),
                    ],
                );
                self.refresh_inventory(server_id, &tools);
                self.tools.insert(server_id.to_string(), tools);
                Ok(())
            }
            Err(error) => {
                self.record(
                    "mcp_discovery_failed",
                    &[
                        ("server", server_id.to_string()),
                        ("error", error.to_string()),
                    ],
                );
                // Drop any half-open connection so a later call can retry.
                self.connections.remove(server_id);
                self.tools.insert(server_id.to_string(), Vec::new());
                Err(error.to_string())
            }
        }
    }

    /// Discovers every active server's tools as ready-to-offer definitions,
    /// listing each one. Connection/list failures are logged and skipped,
    /// never fatal. The chat turn no longer uses this — it lists only the
    /// servers its deferral surface needs — but a caller that wants the whole
    /// set still can.
    pub fn tool_definitions(&mut self) -> Vec<ToolDefinition> {
        let mut definitions = Vec::new();
        for server_id in self.server_ids() {
            let scanner = self.scanner.clone();
            for tool in self.list_server(&server_id) {
                definitions.push(tool.to_tool_definition(&server_id, &scanner));
            }
        }
        definitions
    }

    /// Records a successful listing in the persisted inventory (spec 33 D4),
    /// so a later turn can decide direct-versus-deferred without connecting.
    /// Best-effort: a failed write leaves the previous record, and an absent
    /// record reads as unknown cost and defers, so a failure here can only
    /// make the surface more conservative, never offer more.
    fn refresh_inventory(&self, server_id: &str, tools: &[McpTool]) {
        let Some(data_dir) = &self.data_dir else {
            return;
        };
        if self.repository_id.is_empty() {
            return;
        }
        if let Err(error) =
            McpToolInventory::record_listing(data_dir, &self.repository_id, server_id, tools)
        {
            self.record(
                "mcp_inventory_write_failed",
                &[
                    ("server", server_id.to_string()),
                    ("error", error.to_string()),
                ],
            );
        }
    }

    /// Executes a tool call, returning the flattened result. Errors are the
    /// caller's to convert into a tool-result message. Every call is written
    /// to the invocation history (spec 33 §5.7, D8) — `approval` is how it
    /// got past the approval step.
    pub fn call_tool(
        &mut self,
        server_id: &str,
        tool_name: &str,
        arguments_json: &str,
        approval: McpApprovalDecision,
    ) -> Result<McpToolResult> {
        let started = Instant::now();
        let call_timeout = self.call_timeout;
        let (phase, result) = if let Some(reason) = self.failed_reason(server_id) {
            // Not retried automatically (§5.3): nothing is contacted, and the
            // refusal is not another failure of the server's.
            (None, Err(ClientError::InvalidInput(reason)))
        } else {
            match self.connection(server_id) {
                Ok(client) => (
                    Some(McpContactPhase::Call),
                    client.call_tool(tool_name, arguments_json, call_timeout),
                ),
                Err(error) => (Some(McpContactPhase::Connect), Err(error)),
            }
        };
        if let Some(phase) = phase
            && self.has_server(server_id)
        {
            self.record_health(server_id, phase, result.as_ref().err());
        }
        let duration_ms = started.elapsed().as_millis();
        // Spec 33 §5.5: recorded as the server's claim, never as a fact.
        let claim = read_only_claim_label(self.tool_read_only_hint(server_id, tool_name));
        let mut fields = vec![
            ("server", server_id.to_string()),
            ("tool", tool_name.to_string()),
            ("sessionId", self.session_id.clone()),
            ("outcome", mcp_call_outcome(phase, &result).to_string()),
            ("durationMs", duration_ms.to_string()),
            ("approvalDecision", approval.as_str().to_string()),
            ("serverReadOnlyClaim", claim.to_string()),
            (
                "arguments",
                mcp_history_excerpt(&self.scanner, arguments_json),
            ),
        ];
        match &result {
            Ok(value) => {
                fields.push(("isError", value.is_error.to_string()));
                fields.push(("result", mcp_history_excerpt(&self.scanner, &value.text)));
                self.record("mcp_tool_called", &fields);
            }
            Err(error) => {
                fields.push((
                    "error",
                    mcp_history_excerpt(&self.scanner, &error.to_string()),
                ));
                self.record("mcp_tool_call_failed", &fields);
            }
        }
        result
    }
}

/// The invocation history's `outcome` for one call (spec 33 §5.7, D8):
/// `ok`, `tool_error` (the server answered with an error result), `failed`
/// (the server refused the request), `not_sent` (nothing reached the server:
/// it could not be started or is latched `Failed`), or `unknown` — the
/// request was sent and no answer came (a timeout, a stop, a server that
/// exited), so the server may have acted on it. `unknown` is never retried
/// automatically.
fn mcp_call_outcome(
    phase: Option<McpContactPhase>,
    result: &Result<McpToolResult>,
) -> &'static str {
    match (phase, result) {
        (_, Ok(value)) if value.is_error => "tool_error",
        (_, Ok(_)) => "ok",
        (None | Some(McpContactPhase::Connect), Err(_)) => "not_sent",
        (_, Err(ClientError::InvalidInput(_) | ClientError::AccessDenied(_))) => "failed",
        (_, Err(_)) => "unknown",
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
            vec![
                "/venv/bin/python".to_string(),
                "/path/server.py".to_string()
            ]
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
            vec![
                "/Applications/My App/bin/tool".to_string(),
                "--flag".to_string()
            ]
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
        assert_eq!(
            header_value(headers, "mcp-session-id").as_deref(),
            Some("abc123")
        );
        assert_eq!(body, "{\"result\":1}");
    }

    #[test]
    fn a_schema_is_redacted_string_by_string_and_stays_json() {
        let scanner = SecretScanner::default();
        let schema = r#"{"type":"object","properties":{"key":{"type":"string","default":"password = hunter2-very-secret","description":"Use AKIAIOSFODNN7EXAMPLE"}}}"#;
        let redacted = redact_json_text(&scanner, schema);
        assert!(!redacted.contains("AKIAIOSFODNN7EXAMPLE"), "{redacted}");
        assert!(!redacted.contains("hunter2"), "{redacted}");
        let value: Value = serde_json::from_str(&redacted).expect("still JSON");
        assert_eq!(value["properties"]["key"]["type"], "string");
        // Nothing to redact: returned byte for byte.
        let clean = r#"{"type":"object","properties":{}}"#;
        assert_eq!(redact_json_text(&scanner, clean), clean);
        // Not JSON: redacted as text.
        assert!(!redact_json_text(&scanner, "{ AKIAIOSFODNN7EXAMPLE").contains("AKIA"));
    }

    #[test]
    fn a_history_excerpt_is_redacted_before_it_is_bounded() {
        let scanner = SecretScanner::default();
        // The secret straddles the cut: bounding first would leave a prefix
        // the scanner no longer recognises.
        let text = format!(
            "{}AKIAIOSFODNN7EXAMPLE{}",
            "a ".repeat(MCP_HISTORY_EXCERPT_CHARS / 2 - 5),
            "z".repeat(3_000)
        );
        let excerpt = mcp_history_excerpt(&scanner, &text);
        assert!(!excerpt.contains("AKIA"), "{excerpt}");
        assert!(excerpt.contains("more characters not kept"));
        assert_eq!(mcp_history_excerpt(&scanner, "short"), "short");
    }

    #[test]
    fn a_call_outcome_tells_unsent_refused_and_unknown_apart() {
        let ok = |is_error| {
            Ok(McpToolResult {
                text: String::new(),
                is_error,
            })
        };
        let call = Some(McpContactPhase::Call);
        assert_eq!(mcp_call_outcome(call, &ok(false)), "ok");
        assert_eq!(mcp_call_outcome(call, &ok(true)), "tool_error");
        let timeout = Err(ClientError::McpTimeout("timed out".into()));
        assert_eq!(mcp_call_outcome(call, &timeout), "unknown");
        assert_eq!(
            mcp_call_outcome(call, &Err(ClientError::Cancelled)),
            "unknown"
        );
        let exited = Err(ClientError::Io("exited".into()));
        assert_eq!(mcp_call_outcome(call, &exited), "unknown");
        let refused = Err(ClientError::InvalidInput("bad".into()));
        assert_eq!(mcp_call_outcome(call, &refused), "failed");
        let connect = Some(McpContactPhase::Connect);
        assert_eq!(mcp_call_outcome(connect, &exited), "not_sent");
        assert_eq!(mcp_call_outcome(None, &refused), "not_sent");
    }

    #[test]
    fn only_a_401_or_403_reads_as_refused_credentials() {
        assert_eq!(
            refused_credentials_status("HTTP/1.1 401 Unauthorized\r\nwww-authenticate: Bearer"),
            Some(401)
        );
        assert_eq!(refused_credentials_status("HTTP/2 403"), Some(403));
        assert_eq!(refused_credentials_status("HTTP/1.1 200 OK"), None);
        assert_eq!(refused_credentials_status("HTTP/1.1 404 Not Found"), None);
        assert_eq!(refused_credentials_status(""), None);
    }

    #[test]
    fn parse_mcp_tool_reads_a_true_read_only_hint() {
        let item = json!({
            "name": "search_issues",
            "description": "Search issues",
            "annotations": { "readOnlyHint": true },
        });
        let tool = parse_mcp_tool(&item).unwrap();
        assert_eq!(tool.read_only_hint, Some(true));
    }

    #[test]
    fn parse_mcp_tool_reads_a_false_read_only_hint() {
        let item = json!({
            "name": "create_issue",
            "description": "Create an issue",
            "annotations": { "readOnlyHint": false },
        });
        let tool = parse_mcp_tool(&item).unwrap();
        assert_eq!(tool.read_only_hint, Some(false));
    }

    #[test]
    fn parse_mcp_tool_with_no_annotations_object_is_none() {
        let item = json!({
            "name": "search_issues",
            "description": "Search issues",
        });
        let tool = parse_mcp_tool(&item).unwrap();
        assert_eq!(tool.read_only_hint, None);
    }

    #[test]
    fn parse_mcp_tool_with_annotations_but_no_read_only_hint_is_none() {
        let item = json!({
            "name": "search_issues",
            "description": "Search issues",
            "annotations": { "someOtherHint": true },
        });
        let tool = parse_mcp_tool(&item).unwrap();
        assert_eq!(tool.read_only_hint, None);
    }
}
