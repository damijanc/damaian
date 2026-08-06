use crate::audit::escape_json as audit_escape_json;
use crate::cancel::CancelToken;
use crate::error::{ClientError, Result};
use crate::hash::{create_id, now_millis};
use serde::{Deserialize, Serialize};
use std::io::{Read, Write};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

/// How long the reader may go without data before the cancellation flag is
/// re-checked. The blocking read itself gives no such opportunity, which is why
/// it runs on its own thread.
const CANCEL_POLL_INTERVAL: Duration = Duration::from_millis(250);

/// How long the transport may spend reaching the provider before giving up.
const CONNECT_TIMEOUT_SECS: u64 = 30;
/// How long a connected stream may deliver effectively nothing (see
/// `speed-limit = 1`, i.e. under one byte per second) before it is treated as
/// wedged. A completion can legitimately run for minutes, so the guard is on
/// *progress* rather than total duration: a slow-but-advancing generation
/// survives, a silent socket does not. Note that
/// [`OpenAICompatibleAdapter::stream_response`] retries a stall that happens
/// before the first token, so the worst-case wait is a small multiple of this.
const STALL_TIMEOUT_SECS: u64 = 90;
/// Backstop for the pathological case where a provider dribbles bytes forever,
/// staying just above the stall threshold without ever finishing.
const MAX_TIME_SECS: u64 = 900;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelMessage {
    pub role: String,
    pub content: String,
    /// Set on a `tool` role message to link it back to the assistant's
    /// tool call, per the OpenAI function-calling contract.
    pub tool_call_id: Option<String>,
    /// Set on an `assistant` role message that requested tool calls.
    pub tool_calls: Vec<ToolCall>,
    /// The hidden reasoning a thinking-mode model produced for this assistant
    /// turn. Must be replayed verbatim on any assistant message that carries
    /// `tool_calls`: DeepSeek's thinking mode rejects the next request outright
    /// (`The `reasoning_content` in the thinking mode must be passed back to
    /// the API.`) when it's missing. `#[serde(default)]` keeps pending chat
    /// turns written before this field existed loadable.
    #[serde(default)]
    pub reasoning_content: Option<String>,
}

impl ModelMessage {
    pub fn system(content: impl Into<String>) -> Self {
        Self {
            role: "system".to_string(),
            content: content.into(),
            tool_call_id: None,
            tool_calls: Vec::new(),
            reasoning_content: None,
        }
    }

    pub fn user(content: impl Into<String>) -> Self {
        Self {
            role: "user".to_string(),
            content: content.into(),
            tool_call_id: None,
            tool_calls: Vec::new(),
            reasoning_content: None,
        }
    }

    pub fn assistant(content: impl Into<String>) -> Self {
        Self {
            role: "assistant".to_string(),
            content: content.into(),
            tool_call_id: None,
            tool_calls: Vec::new(),
            reasoning_content: None,
        }
    }

    /// An assistant turn that requested one or more native tool calls,
    /// carried alongside any content the model emitted before/instead of
    /// the call so both sides of the exchange round-trip back to the
    /// provider on the next request.
    ///
    /// `reasoning_content` is the thinking-mode reasoning behind the call, and
    /// is mandatory for DeepSeek reasoning models — see
    /// [`ModelMessage::reasoning_content`]. Pass the originating
    /// [`ModelRun::reasoning_content`] straight through; `None` is correct only
    /// when the provider returned none.
    pub fn assistant_with_tool_calls(
        content: impl Into<String>,
        tool_calls: Vec<ToolCall>,
        reasoning_content: Option<String>,
    ) -> Self {
        Self {
            role: "assistant".to_string(),
            content: content.into(),
            tool_call_id: None,
            tool_calls,
            reasoning_content,
        }
    }

    /// A `tool` role message carrying the result of a specific tool call
    /// back to the model, keyed by `tool_call_id`.
    pub fn tool(tool_call_id: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            role: "tool".to_string(),
            content: content.into(),
            tool_call_id: Some(tool_call_id.into()),
            tool_calls: Vec::new(),
            reasoning_content: None,
        }
    }
}

/// An OpenAI-style function tool definition. `parameters_json` is a raw JSON
/// object string (e.g. `{"type":"object","properties":{...}}`) embedded
/// verbatim into the request rather than re-parsed, since callers already
/// have it in that shape.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolDefinition {
    pub name: String,
    pub description: String,
    pub parameters_json: String,
}

/// A tool call the model asked to make, extracted from either a
/// non-streaming response or a streamed one (fragmented `arguments` deltas
/// are concatenated by tool-call index before being surfaced here).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub arguments_json: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelRequest {
    pub provider: String,
    pub model: String,
    pub messages: Vec<ModelMessage>,
    pub temperature: Option<String>,
    pub reasoning_level: Option<String>,
    pub stream: bool,
    /// Native tool/function definitions to offer the model. Only meaningful
    /// when the active provider is configured with
    /// `ModelProviderConfig::supports_native_tools`; otherwise callers
    /// should leave this `None` and rely on the `DAMAIAN_COMMAND_V1` text
    /// envelope instead.
    pub tools: Option<Vec<ToolDefinition>>,
    /// Explicit output-token ceiling. `None` omits `max_tokens` and lets the
    /// provider apply its own default.
    pub max_tokens: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelRun {
    pub run_id: String,
    pub provider: String,
    pub model: String,
    pub started_at_ms: u128,
    pub completed_at_ms: u128,
    pub content: String,
    pub incomplete: bool,
    pub retry_count: u32,
    pub tool_calls: Vec<ToolCall>,
    /// The provider stopped because it hit the output-token ceiling
    /// (`finish_reason: "length"`) rather than finishing its answer. Anything
    /// structured in this run — most importantly a tool call's `arguments`
    /// JSON — may be cut off mid-token and fail to parse.
    pub truncated: bool,
    /// Hidden thinking-mode reasoning, when the provider returned any. Not for
    /// display: its only use is being replayed on the assistant message that
    /// carries [`Self::tool_calls`] — see [`ModelMessage::reasoning_content`].
    pub reasoning_content: Option<String>,
}

impl ModelRun {
    /// Stands in for the run that never happened when a turn is stopped before
    /// the provider is called. [`ChatTurnResult`](crate::ChatTurnResult) always
    /// carries a run, and a cancelled turn still needs an id to audit against.
    pub fn cancelled_before_start(provider: &str, model: &str) -> Self {
        let now = now_millis();
        Self {
            run_id: create_id("modelrun"),
            provider: provider.to_string(),
            model: model.to_string(),
            started_at_ms: now,
            completed_at_ms: now,
            content: String::new(),
            incomplete: true,
            retry_count: 0,
            tool_calls: Vec::new(),
            truncated: false,
            reasoning_content: None,
        }
    }
}

pub trait ModelAdapter {
    fn stream_response(
        &mut self,
        request: &ModelRequest,
        cancel: &CancelToken,
        on_token: &mut dyn FnMut(&str),
    ) -> Result<ModelRun>;

    fn estimate_tokens(&self, payload: &str) -> usize {
        payload.len().div_ceil(4)
    }
}

#[derive(Debug, Clone)]
pub struct MockModelAdapter {
    responses: Vec<String>,
    tool_calls: Vec<Vec<ToolCall>>,
    /// Per-response `finish_reason: "length"` simulation, matched by index.
    /// Empty (the default) means no response is truncated.
    truncated: Vec<bool>,
    /// Per-response thinking-mode reasoning, matched by index. Empty (the
    /// default) means no response carries reasoning.
    reasoning_content: Vec<Option<String>>,
    next_response: usize,
    /// Every request the adapter was handed, in order, so tests can assert on
    /// what a later round actually replayed back to the provider.
    pub requests: Vec<ModelRequest>,
}

impl MockModelAdapter {
    pub fn new(response: impl Into<String>) -> Self {
        Self {
            responses: vec![response.into()],
            tool_calls: vec![Vec::new()],
            truncated: Vec::new(),
            reasoning_content: Vec::new(),
            next_response: 0,
            requests: Vec::new(),
        }
    }

    pub fn new_sequence(responses: Vec<String>) -> Self {
        let tool_calls = responses.iter().map(|_| Vec::new()).collect();
        Self {
            responses,
            tool_calls,
            truncated: Vec::new(),
            reasoning_content: Vec::new(),
            next_response: 0,
            requests: Vec::new(),
        }
    }

    /// Like `new_sequence`, but also returns the given tool calls alongside
    /// each response (matched by index), for testing native tool-calling
    /// dispatch without a real provider.
    pub fn new_sequence_with_tool_calls(
        responses: Vec<String>,
        tool_calls: Vec<Vec<ToolCall>>,
    ) -> Self {
        Self {
            responses,
            tool_calls,
            truncated: Vec::new(),
            reasoning_content: Vec::new(),
            next_response: 0,
            requests: Vec::new(),
        }
    }

    /// Marks responses (by index) as having stopped at the provider's
    /// output-token ceiling, for testing truncation handling.
    pub fn with_truncated(mut self, truncated: Vec<bool>) -> Self {
        self.truncated = truncated;
        self
    }

    /// Attaches thinking-mode reasoning to responses (by index), for testing
    /// that it is replayed on the assistant's tool-call message.
    pub fn with_reasoning_content(mut self, reasoning_content: Vec<Option<String>>) -> Self {
        self.reasoning_content = reasoning_content;
        self
    }
}

impl ModelAdapter for MockModelAdapter {
    fn stream_response(
        &mut self,
        request: &ModelRequest,
        cancel: &CancelToken,
        on_token: &mut dyn FnMut(&str),
    ) -> Result<ModelRun> {
        let run_id = create_id("modelrun");
        let started_at_ms = now_millis();
        self.requests.push(request.clone());
        let mut content = String::new();
        let index = self.next_response;
        let response = self
            .responses
            .get(index)
            .or_else(|| self.responses.last())
            .cloned()
            .unwrap_or_default();
        let tool_calls = self
            .tool_calls
            .get(index)
            .or_else(|| self.tool_calls.last())
            .cloned()
            .unwrap_or_default();
        if self.next_response + 1 < self.responses.len() {
            self.next_response += 1;
        }
        for chunk in response.as_bytes().chunks(24) {
            if cancel.is_cancelled() {
                break;
            }
            let token = String::from_utf8_lossy(chunk);
            content.push_str(&token);
            on_token(&token);
        }
        Ok(ModelRun {
            run_id: run_id.clone(),
            provider: "mock".to_string(),
            model: request.model.clone(),
            started_at_ms,
            completed_at_ms: now_millis(),
            content,
            incomplete: cancel.is_cancelled(),
            retry_count: 0,
            tool_calls,
            truncated: self.truncated.get(index).copied().unwrap_or(false),
            reasoning_content: self.reasoning_content.get(index).cloned().flatten(),
        })
    }
}

pub trait ModelTransport {
    fn send(&mut self, request_body: &str) -> Result<String>;

    fn send_stream(
        &mut self,
        request_body: &str,
        cancel: &CancelToken,
        on_chunk: &mut dyn FnMut(&str),
    ) -> Result<String> {
        cancel.check()?;
        let raw = self.send(request_body)?;
        on_chunk(&raw);
        Ok(raw)
    }
}

#[derive(Debug, Clone)]
pub struct CurlModelTransport {
    pub base_url: String,
    pub api_key: String,
}

impl CurlModelTransport {
    pub fn new(base_url: impl Into<String>, api_key: impl Into<String>) -> Self {
        Self {
            base_url: base_url.into().trim_end_matches('/').to_string(),
            api_key: api_key.into(),
        }
    }

    fn curl_args() -> [&'static str; 4] {
        ["-sS", "--no-buffer", "--config", "-"]
    }

    fn chat_completions_url(&self) -> String {
        format!("{}/v1/chat/completions", self.base_url)
    }

    fn curl_config(&self, request_body: &str) -> String {
        format!(
            "request = \"POST\"\nurl = \"{}\"\nheader = \"content-type: application/json\"\nheader = \"authorization: Bearer {}\"\ndata-binary = \"{}\"\nconnect-timeout = {CONNECT_TIMEOUT_SECS}\nspeed-limit = 1\nspeed-time = {STALL_TIMEOUT_SECS}\nmax-time = {MAX_TIME_SECS}\n",
            escape_curl_config_value(&self.chat_completions_url()),
            escape_curl_config_value(&self.api_key),
            escape_curl_config_value(request_body)
        )
    }
}

impl ModelTransport for CurlModelTransport {
    fn send(&mut self, request_body: &str) -> Result<String> {
        self.send_stream(request_body, &CancelToken::new(), &mut |_chunk| {})
    }

    fn send_stream(
        &mut self,
        request_body: &str,
        cancel: &CancelToken,
        on_chunk: &mut dyn FnMut(&str),
    ) -> Result<String> {
        // Before spawning, so a turn stopped while queued never reaches the
        // provider and never gets billed.
        cancel.check()?;

        let mut child = KillOnDrop(
            Command::new("curl")
                .args(Self::curl_args())
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()?,
        );

        if let Some(mut stdin) = child.child().stdin.take() {
            stdin.write_all(self.curl_config(request_body).as_bytes())?;
        }

        // Taken out first so the borrow for the scrutinee ends before the
        // cancellation closure below borrows the child to kill it.
        let stdout = child.child().stdout.take();
        let raw = match stdout {
            Some(stdout) => pump_stream(stdout, cancel, on_chunk, || {
                // Closes the pipe, which lets the reader thread finish so
                // `pump_stream` can join it instead of hanging.
                let _ = child.child().kill();
            })?,
            None => String::new(),
        };

        let status = child.child().wait()?;
        let mut stderr = String::new();
        if let Some(mut stderr_pipe) = child.child().stderr.take() {
            stderr_pipe.read_to_string(&mut stderr)?;
        }
        if !status.success() {
            return Err(ClientError::Io(format!(
                "Model provider transport failed: {}",
                stderr
            )));
        }
        Ok(raw)
    }
}

/// Reads `reader` to EOF, handing each chunk to `on_chunk`, and gives up as soon
/// as `cancel` is set.
///
/// The read runs on its own thread because a blocking read offers no chance to
/// notice a cancellation — which is exactly the case that matters, since a
/// provider that has not started generating yet sends nothing at all. The
/// calling thread waits on the channel with a timeout instead, so it stays
/// responsive to the flag.
///
/// `on_cancel` runs before the reader is joined. It must make the reader finish
/// (for a child process, by killing it); otherwise the join would block for as
/// long as the read would have.
fn pump_stream<R>(
    reader: R,
    cancel: &CancelToken,
    on_chunk: &mut dyn FnMut(&str),
    on_cancel: impl FnOnce(),
) -> Result<String>
where
    R: Read + Send + 'static,
{
    let (sender, receiver) = mpsc::channel();
    let reader_thread = std::thread::spawn(move || {
        let mut reader = reader;
        let mut buffer = [0_u8; 8192];
        loop {
            match reader.read(&mut buffer) {
                Ok(0) | Err(_) => break,
                // Decoded here rather than on the calling thread to keep the
                // existing per-chunk lossy behaviour unchanged.
                Ok(read) => {
                    let chunk = String::from_utf8_lossy(&buffer[..read]).to_string();
                    if sender.send(chunk).is_err() {
                        break;
                    }
                }
            }
        }
    });

    let mut raw = String::new();
    let outcome = loop {
        if cancel.is_cancelled() {
            break Err(ClientError::Cancelled);
        }
        match receiver.recv_timeout(CANCEL_POLL_INTERVAL) {
            Ok(chunk) => {
                raw.push_str(&chunk);
                on_chunk(&chunk);
            }
            Err(mpsc::RecvTimeoutError::Timeout) => continue,
            Err(mpsc::RecvTimeoutError::Disconnected) => break Ok(()),
        }
    };

    if outcome.is_err() {
        on_cancel();
    }
    let _ = reader_thread.join();
    outcome.map(|()| raw)
}

/// Kills the child if it is still running when this is dropped, so a panic on
/// the calling thread cannot leave `curl` streaming a paid-for completion into
/// nothing for the rest of `max-time`.
struct KillOnDrop(Child);

impl KillOnDrop {
    fn child(&mut self) -> &mut Child {
        &mut self.0
    }
}

impl Drop for KillOnDrop {
    fn drop(&mut self) {
        // Already-exited is the normal case and reports an error here; either
        // way there is nothing to recover from at drop time.
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

pub(crate) fn escape_curl_config_value(value: &str) -> String {
    let mut escaped = String::new();
    for character in value.chars() {
        match character {
            '"' => escaped.push_str("\\\""),
            '\\' => escaped.push_str("\\\\"),
            '\n' => escaped.push_str("\\n"),
            '\r' => escaped.push_str("\\r"),
            '\t' => escaped.push_str("\\t"),
            character if character.is_control() => {
                escaped.push_str(&format!("\\u{:04x}", character as u32));
            }
            character => escaped.push(character),
        }
    }
    escaped
}

#[derive(Debug, Clone)]
pub struct MockModelTransport {
    pub response: String,
    pub requests: Vec<String>,
    /// Number of remaining calls that should fail with a retryable error
    /// before `response` is returned. Lets tests simulate transient
    /// transport failures without shelling out to real curl.
    pub fail_before_success: u32,
    pub failure_message: String,
}

impl MockModelTransport {
    pub fn new(response: impl Into<String>) -> Self {
        Self {
            response: response.into(),
            requests: Vec::new(),
            fail_before_success: 0,
            failure_message: "connection reset by peer".to_string(),
        }
    }

    pub fn failing(response: impl Into<String>, fail_before_success: u32) -> Self {
        Self {
            fail_before_success,
            ..Self::new(response)
        }
    }
}

impl ModelTransport for MockModelTransport {
    fn send(&mut self, request_body: &str) -> Result<String> {
        self.requests.push(request_body.to_string());
        if self.fail_before_success > 0 {
            self.fail_before_success -= 1;
            return Err(ClientError::Io(self.failure_message.clone()));
        }
        Ok(self.response.clone())
    }
}

pub struct OpenAICompatibleAdapter<T: ModelTransport> {
    provider: String,
    model: String,
    transport: T,
}

impl<T: ModelTransport> OpenAICompatibleAdapter<T> {
    pub fn new(model: impl Into<String>, transport: T) -> Self {
        Self::with_provider("openai-compatible", model, transport)
    }

    pub fn with_provider(
        provider: impl Into<String>,
        model: impl Into<String>,
        transport: T,
    ) -> Self {
        Self {
            provider: provider.into(),
            model: model.into(),
            transport,
        }
    }
}

impl<T: ModelTransport> ModelAdapter for OpenAICompatibleAdapter<T> {
    fn stream_response(
        &mut self,
        request: &ModelRequest,
        cancel: &CancelToken,
        on_token: &mut dyn FnMut(&str),
    ) -> Result<ModelRun> {
        const MAX_ATTEMPTS: u32 = 3;
        const RETRY_BACKOFF_MS: [u64; 2] = [500, 1500];

        let run_id = create_id("modelrun");
        let started_at_ms = now_millis();
        let body = model_request_json(request);
        let mut content = String::new();
        let mut emitted_any = false;
        let mut attempt: u32 = 0;

        let raw = loop {
            attempt += 1;
            let mut buffered_stream = String::new();
            let mut saw_sse_stream = false;
            let mut emit_token = |token: String| {
                if cancel.is_cancelled() {
                    return;
                }
                emitted_any = true;
                content.push_str(&token);
                on_token(&token);
            };
            let send_result = self.transport.send_stream(&body, cancel, &mut |chunk| {
                buffered_stream.push_str(chunk);
                if buffered_stream.contains("data:") || saw_sse_stream {
                    saw_sse_stream = true;
                    while let Some(line_end) = buffered_stream.find('\n') {
                        let line = buffered_stream[..line_end].to_string();
                        buffered_stream = buffered_stream[line_end + 1..].to_string();
                        for token in extract_model_tokens(&line) {
                            emit_token(token);
                        }
                    }
                }
            });

            match send_result {
                Ok(raw) => {
                    if saw_sse_stream {
                        for token in extract_model_tokens(&buffered_stream) {
                            emit_token(token);
                        }
                    } else {
                        for token in extract_model_tokens(&raw) {
                            emit_token(token);
                        }
                    }
                    break raw;
                }
                Err(error) => {
                    // Only retry connection-level failures that happened before any
                    // token reached the caller. Once output has started streaming to
                    // the UI, retrying would duplicate or blend partial content, so a
                    // mid-stream failure is propagated as-is instead.
                    if !emitted_any && attempt < MAX_ATTEMPTS && error.is_retryable() {
                        std::thread::sleep(std::time::Duration::from_millis(
                            RETRY_BACKOFF_MS[(attempt - 1) as usize],
                        ));
                        continue;
                    }
                    return Err(error);
                }
            }
        };
        let retry_count = attempt - 1;

        if let Some(message) = extract_error_message(&raw) {
            return Err(ClientError::Io(format!("Model provider error: {message}")));
        }
        let tool_calls = extract_tool_calls(&raw);
        if content.is_empty() && tool_calls.is_empty() && !cancel.is_cancelled() {
            return Err(ClientError::Io(
                "Model provider returned no assistant content".to_string(),
            ));
        }

        Ok(ModelRun {
            run_id: run_id.clone(),
            provider: self.provider.clone(),
            model: if request.model.is_empty() {
                self.model.clone()
            } else {
                request.model.clone()
            },
            started_at_ms,
            completed_at_ms: now_millis(),
            content,
            incomplete: cancel.is_cancelled(),
            retry_count,
            tool_calls,
            truncated: response_was_truncated(&raw),
            reasoning_content: extract_reasoning_content(&raw),
        })
    }
}

fn message_json(message: &ModelMessage) -> String {
    let mut object = format!("{{\"role\":\"{}\"", audit_escape_json(&message.role));
    if message.tool_calls.is_empty() {
        object.push_str(&format!(
            ",\"content\":\"{}\"",
            audit_escape_json(&message.content)
        ));
    } else {
        if message.content.is_empty() {
            object.push_str(",\"content\":null");
        } else {
            object.push_str(&format!(
                ",\"content\":\"{}\"",
                audit_escape_json(&message.content)
            ));
        }
        let tool_calls_json = message
            .tool_calls
            .iter()
            .map(|call| {
                format!(
                    "{{\"id\":\"{}\",\"type\":\"function\",\"function\":{{\"name\":\"{}\",\"arguments\":\"{}\"}}}}",
                    audit_escape_json(&call.id),
                    audit_escape_json(&call.name),
                    audit_escape_json(&call.arguments_json)
                )
            })
            .collect::<Vec<_>>()
            .join(",");
        object.push_str(&format!(",\"tool_calls\":[{tool_calls_json}]"));
    }
    if let Some(tool_call_id) = &message.tool_call_id {
        object.push_str(&format!(
            ",\"tool_call_id\":\"{}\"",
            audit_escape_json(tool_call_id)
        ));
    }
    // Replayed for the provider's benefit, not the user's: DeepSeek's thinking
    // mode requires the reasoning behind a tool call to come back with it.
    if let Some(reasoning_content) = &message.reasoning_content {
        object.push_str(&format!(
            ",\"reasoning_content\":\"{}\"",
            audit_escape_json(reasoning_content)
        ));
    }
    object.push('}');
    object
}

pub fn model_request_json(request: &ModelRequest) -> String {
    let messages = request
        .messages
        .iter()
        .map(message_json)
        .collect::<Vec<_>>()
        .join(",");
    let mut body = format!(
        "{{\"model\":\"{}\",\"messages\":[{}],\"stream\":{}",
        audit_escape_json(&request.model),
        messages,
        request.stream
    );
    if let Some(temperature) = &request.temperature {
        body.push_str(&format!(",\"temperature\":{}", temperature));
    }
    if let Some(max_tokens) = request.max_tokens {
        body.push_str(&format!(",\"max_tokens\":{max_tokens}"));
    }
    if let Some(reasoning_effort) =
        api_reasoning_effort(&request.provider, &request.reasoning_level)
    {
        body.push_str(&format!(
            ",\"reasoning_effort\":\"{}\"",
            audit_escape_json(reasoning_effort)
        ));
    }
    if let Some(tools) = &request.tools
        && !tools.is_empty()
    {
        let tools_json = tools
            .iter()
            .map(|tool| {
                format!(
                    "{{\"type\":\"function\",\"function\":{{\"name\":\"{}\",\"description\":\"{}\",\"parameters\":{}}}}}",
                    audit_escape_json(&tool.name),
                    audit_escape_json(&tool.description),
                    tool.parameters_json
                )
            })
            .collect::<Vec<_>>()
            .join(",");
        body.push_str(&format!(",\"tools\":[{tools_json}]"));
    }
    body.push('}');
    body
}

fn api_reasoning_effort<'a>(
    provider: &str,
    reasoning_level: &'a Option<String>,
) -> Option<&'a str> {
    let supports_reasoning_effort = matches!(
        provider,
        "openai" | "openai-compatible" | "open-ai-compatible"
    );
    if !supports_reasoning_effort {
        return None;
    }
    let level = reasoning_level.as_deref()?.trim();
    match level {
        "" | "default" | "auto" => None,
        "minimal" | "low" | "medium" | "high" => Some(level),
        _ => None,
    }
}

pub fn extract_model_tokens(raw: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    if raw.contains("data:") {
        for line in raw.lines() {
            let trimmed = line.trim();
            if !trimmed.starts_with("data:") {
                continue;
            }
            let payload = trimmed.trim_start_matches("data:").trim();
            if payload == "[DONE]" {
                continue;
            }
            tokens.extend(extract_content_values(payload));
        }
    } else {
        tokens.extend(extract_content_values(raw));
    }
    tokens
}

fn extract_content_values(raw: &str) -> Vec<String> {
    let mut values = Vec::new();
    let bytes = raw.as_bytes();
    let needle = b"\"content\"";
    let mut cursor = 0;
    while cursor + needle.len() <= bytes.len() {
        let Some(offset) = find_bytes(&bytes[cursor..], needle) else {
            break;
        };
        let key_start = cursor + offset;
        let mut index = key_start + needle.len();
        while index < bytes.len() && bytes[index].is_ascii_whitespace() {
            index += 1;
        }
        if bytes.get(index) != Some(&b':') {
            cursor = key_start + needle.len();
            continue;
        }
        index += 1;
        while index < bytes.len() && bytes[index].is_ascii_whitespace() {
            index += 1;
        }
        if bytes.get(index) != Some(&b'"') {
            cursor = index;
            continue;
        }
        if let Some((value, end)) = parse_json_string(raw, index) {
            values.push(value);
            cursor = end;
        } else {
            break;
        }
    }
    values
}

/// Extracts tool calls from a complete OpenAI-style response, handling both
/// a single non-streaming JSON object and an SSE stream of `data: {...}`
/// lines. Streamed `arguments` fragments are concatenated by tool-call
/// index, since providers split a single call's arguments across multiple
/// deltas. Uses `serde_json` (unlike the hand-rolled scanners above) since
/// tool-call payloads are nested objects that are awkward to byte-scan.
fn extract_tool_calls(raw: &str) -> Vec<ToolCall> {
    let mut calls: Vec<ToolCall> = Vec::new();

    let mut merge_from_value = |value: &serde_json::Value| {
        let Some(choices) = value.get("choices").and_then(|choices| choices.as_array()) else {
            return;
        };
        for choice in choices {
            let tool_calls = choice
                .get("delta")
                .and_then(|delta| delta.get("tool_calls"))
                .or_else(|| {
                    choice
                        .get("message")
                        .and_then(|message| message.get("tool_calls"))
                })
                .and_then(|tool_calls| tool_calls.as_array());
            let Some(tool_calls) = tool_calls else {
                continue;
            };
            for (position, entry) in tool_calls.iter().enumerate() {
                let index = entry
                    .get("index")
                    .and_then(|index| index.as_u64())
                    .map(|index| index as usize)
                    .unwrap_or(position);
                while calls.len() <= index {
                    calls.push(ToolCall::default());
                }
                let call = &mut calls[index];
                if let Some(id) = entry.get("id").and_then(|id| id.as_str()) {
                    call.id = id.to_string();
                }
                if let Some(function) = entry.get("function") {
                    if let Some(name) = function.get("name").and_then(|name| name.as_str()) {
                        call.name = name.to_string();
                    }
                    if let Some(arguments) =
                        function.get("arguments").and_then(|value| value.as_str())
                    {
                        call.arguments_json.push_str(arguments);
                    }
                }
            }
        }
    };

    if raw.contains("data:") {
        for line in raw.lines() {
            let trimmed = line.trim();
            let Some(payload) = trimmed.strip_prefix("data:") else {
                continue;
            };
            let payload = payload.trim();
            if payload == "[DONE]" {
                continue;
            }
            if let Ok(value) = serde_json::from_str::<serde_json::Value>(payload) {
                merge_from_value(&value);
            }
        }
    } else if let Ok(value) = serde_json::from_str::<serde_json::Value>(raw) {
        merge_from_value(&value);
    }

    calls.retain(|call| !call.name.is_empty());
    calls
}

/// The thinking-mode reasoning a response carried, or `None` if the model
/// didn't think. Handles both a non-streaming object (`choices[].message`) and
/// an SSE stream, where reasoning arrives fragmented across `delta` chunks just
/// like `content` and has to be reassembled in arrival order.
///
/// This exists purely so the reasoning can be handed back on the next request:
/// DeepSeek rejects a follow-up whose assistant tool-call message is missing
/// it. It is never shown to the user.
fn extract_reasoning_content(raw: &str) -> Option<String> {
    fn chunk_reasoning(value: &serde_json::Value) -> Option<&str> {
        value.get("choices")?.as_array()?.iter().find_map(|choice| {
            // A stream carries `delta`, a whole response `message`.
            choice
                .get("delta")
                .or_else(|| choice.get("message"))?
                .get("reasoning_content")?
                .as_str()
        })
    }

    let mut reasoning = String::new();
    if raw.contains("data:") {
        for line in raw.lines() {
            let Some(payload) = line.trim().strip_prefix("data:") else {
                continue;
            };
            let payload = payload.trim();
            if payload == "[DONE]" {
                continue;
            }
            if let Ok(value) = serde_json::from_str::<serde_json::Value>(payload)
                && let Some(fragment) = chunk_reasoning(&value)
            {
                reasoning.push_str(fragment);
            }
        }
    } else if let Ok(value) = serde_json::from_str::<serde_json::Value>(raw)
        && let Some(fragment) = chunk_reasoning(&value)
    {
        reasoning.push_str(fragment);
    }

    (!reasoning.is_empty()).then_some(reasoning)
}

/// Whether the provider stopped because it ran out of output budget. Handles
/// both a non-streaming object and an SSE stream, mirroring
/// `extract_tool_calls`; in a stream only the final chunk carries a non-null
/// `finish_reason`, so every chunk is checked and any `"length"` counts.
fn response_was_truncated(raw: &str) -> bool {
    fn any_length_finish(value: &serde_json::Value) -> bool {
        value
            .get("choices")
            .and_then(|choices| choices.as_array())
            .is_some_and(|choices| {
                choices.iter().any(|choice| {
                    choice
                        .get("finish_reason")
                        .and_then(|reason| reason.as_str())
                        == Some("length")
                })
            })
    }

    if raw.contains("data:") {
        raw.lines().any(|line| {
            let Some(payload) = line.trim().strip_prefix("data:") else {
                return false;
            };
            let payload = payload.trim();
            payload != "[DONE]"
                && serde_json::from_str::<serde_json::Value>(payload)
                    .is_ok_and(|value| any_length_finish(&value))
        })
    } else {
        serde_json::from_str::<serde_json::Value>(raw).is_ok_and(|value| any_length_finish(&value))
    }
}

fn extract_error_message(raw: &str) -> Option<String> {
    if !raw.contains("\"error\"") {
        return None;
    }
    extract_string_field(raw, "message")
}

fn extract_string_field(raw: &str, field: &str) -> Option<String> {
    let needle = format!("\"{field}\"");
    let bytes = raw.as_bytes();
    let mut cursor = 0;
    while cursor + needle.len() <= raw.len() {
        let offset = find_bytes(&bytes[cursor..], needle.as_bytes())?;
        let key_start = cursor + offset;
        let mut index = key_start + needle.len();
        while index < bytes.len() && bytes[index].is_ascii_whitespace() {
            index += 1;
        }
        if bytes.get(index) != Some(&b':') {
            cursor = key_start + needle.len();
            continue;
        }
        index += 1;
        while index < bytes.len() && bytes[index].is_ascii_whitespace() {
            index += 1;
        }
        if bytes.get(index) != Some(&b'"') {
            return None;
        }
        return parse_json_string(raw, index).map(|(value, _)| value);
    }
    None
}

fn parse_json_string(raw: &str, quote_start: usize) -> Option<(String, usize)> {
    let bytes = raw.as_bytes();
    if bytes.get(quote_start) != Some(&b'"') {
        return None;
    }
    let mut output = String::new();
    let mut index = quote_start + 1;
    let mut segment_start = index;
    while index < bytes.len() {
        match bytes[index] {
            b'"' => {
                output.push_str(raw.get(segment_start..index)?);
                return Some((output, index + 1));
            }
            b'\\' => {
                output.push_str(raw.get(segment_start..index)?);
                index += 1;
                let escaped = *bytes.get(index)?;
                match escaped {
                    b'"' => output.push('"'),
                    b'\\' => output.push('\\'),
                    b'/' => output.push('/'),
                    b'b' => output.push('\u{0008}'),
                    b'f' => output.push('\u{000c}'),
                    b'n' => output.push('\n'),
                    b'r' => output.push('\r'),
                    b't' => output.push('\t'),
                    b'u' => {
                        let hex = raw.get(index + 1..index + 5)?;
                        let codepoint = u32::from_str_radix(hex, 16).ok()?;
                        if let Some(character) = char::from_u32(codepoint) {
                            output.push(character);
                        }
                        index += 4;
                    }
                    other => output.push(other as char),
                }
                index += 1;
                segment_start = index;
                continue;
            }
            _ => {}
        }
        index += 1;
    }
    None
}

fn find_bytes(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Behaves like a provider connection that has accepted the request but
    /// sent nothing yet: `read` blocks. Returns EOF once `closed` flips, which
    /// is what happens to `child.stdout` after the child is killed.
    struct SilentPipe {
        closed: std::sync::Arc<std::sync::atomic::AtomicBool>,
    }

    impl std::io::Read for SilentPipe {
        fn read(&mut self, _buffer: &mut [u8]) -> std::io::Result<usize> {
            while !self.closed.load(std::sync::atomic::Ordering::SeqCst) {
                std::thread::sleep(Duration::from_millis(10));
            }
            Ok(0)
        }
    }

    #[test]
    fn pump_stream_returns_everything_the_provider_sent() {
        let cancel = CancelToken::new();
        let mut chunks = Vec::new();

        let raw = pump_stream(
            std::io::Cursor::new(b"hello world".to_vec()),
            &cancel,
            &mut |chunk| chunks.push(chunk.to_string()),
            || panic!("must not kill the child on the success path"),
        )
        .expect("pump");

        assert_eq!(raw, "hello world");
        assert_eq!(chunks.concat(), "hello world");
    }

    // The regression test for the 90-minute unstoppable turn: a stop arriving
    // while the provider is silent must not wait for the blocking read.
    #[test]
    fn pump_stream_stops_promptly_when_cancelled_while_the_provider_is_silent() {
        use std::sync::Arc;
        use std::sync::atomic::{AtomicBool, Ordering};

        let closed = Arc::new(AtomicBool::new(false));
        let killed = Arc::new(AtomicBool::new(false));
        let cancel = CancelToken::new();

        let stopper = cancel.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(100));
            stopper.cancel();
        });

        let kill_flag = Arc::clone(&killed);
        let pipe_closed = Arc::clone(&closed);
        let started = std::time::Instant::now();
        let result = pump_stream(
            SilentPipe {
                closed: Arc::clone(&closed),
            },
            &cancel,
            &mut |_chunk| {},
            move || {
                // Stands in for `child.kill()`, which closes the pipe and lets
                // the reader thread finish.
                kill_flag.store(true, Ordering::SeqCst);
                pipe_closed.store(true, Ordering::SeqCst);
            },
        );

        assert_eq!(result.unwrap_err(), ClientError::Cancelled);
        // Without this the process leaks and keeps billing tokens for the rest
        // of `max-time`.
        assert!(killed.load(Ordering::SeqCst), "the child must be killed");
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "took {:?}, so it waited on the blocking read instead of the token",
            started.elapsed()
        );
    }

    #[test]
    fn pump_stream_kills_the_child_when_cancelled_before_it_starts() {
        use std::sync::Arc;
        use std::sync::atomic::{AtomicBool, Ordering};

        let closed = Arc::new(AtomicBool::new(false));
        let killed = Arc::new(AtomicBool::new(false));
        let cancel = CancelToken::new();
        cancel.cancel();

        let kill_flag = Arc::clone(&killed);
        let pipe_closed = Arc::clone(&closed);
        let result = pump_stream(
            SilentPipe {
                closed: Arc::clone(&closed),
            },
            &cancel,
            &mut |_chunk| panic!("must not emit chunks after cancellation"),
            move || {
                kill_flag.store(true, Ordering::SeqCst);
                pipe_closed.store(true, Ordering::SeqCst);
            },
        );

        assert_eq!(result.unwrap_err(), ClientError::Cancelled);
        assert!(killed.load(Ordering::SeqCst));
    }

    // Cheap and offline: it must bail out before spawning anything, so no
    // request reaches the (nonexistent) host.
    #[test]
    fn curl_transport_does_not_send_a_request_for_an_already_cancelled_turn() {
        let mut transport = CurlModelTransport::new("https://api.example.test/", "sk_test");
        let cancel = CancelToken::new();
        cancel.cancel();

        let started = std::time::Instant::now();
        let result = transport.send_stream("{\"model\":\"test\"}", &cancel, &mut |_chunk| {
            panic!("must not stream anything for a cancelled turn")
        });

        assert_eq!(result.unwrap_err(), ClientError::Cancelled);
        assert!(
            started.elapsed() < Duration::from_secs(1),
            "took {:?}, so it spawned curl before checking the token",
            started.elapsed()
        );
    }

    // A completion with no bound is a hang with no bound: the desktop shell
    // serves requests on a single thread, so one wedged provider connection
    // freezes the whole UI until the app is killed.
    #[test]
    fn curl_transport_bounds_connect_stall_and_total_time() {
        let transport = CurlModelTransport::new("https://api.example.test/", "sk_test");
        let config = transport.curl_config("{\"model\":\"test\",\"messages\":[]}");

        assert!(config.contains(&format!("connect-timeout = {CONNECT_TIMEOUT_SECS}")));
        // Progress-based, not duration-based: a long generation that keeps
        // streaming must survive, while a silent stream must not.
        assert!(config.contains("speed-limit = 1"));
        assert!(config.contains(&format!("speed-time = {STALL_TIMEOUT_SECS}")));
        assert!(config.contains(&format!("max-time = {MAX_TIME_SECS}")));
    }

    #[test]
    fn curl_transport_does_not_put_api_key_in_argv() {
        let api_key = "sk_test_12345678901234567890";
        let transport = CurlModelTransport::new("https://api.example.test/", api_key);
        let args = CurlModelTransport::curl_args();

        assert!(!args.iter().any(|arg| arg.contains(api_key)));
        assert_eq!(args, ["-sS", "--no-buffer", "--config", "-"]);

        let config = transport.curl_config("{\"model\":\"test\",\"messages\":[]}");
        assert!(config.contains(&format!("authorization: Bearer {api_key}")));
        assert!(
            config.contains("data-binary = \"{\\\"model\\\":\\\"test\\\",\\\"messages\\\":[]}\"")
        );
    }

    fn test_request() -> ModelRequest {
        ModelRequest {
            provider: "openai-compatible".to_string(),
            model: "test-model".to_string(),
            messages: vec![ModelMessage::user("hello")],
            temperature: None,
            reasoning_level: None,
            stream: false,
            tools: None,
            max_tokens: None,
        }
    }

    #[test]
    fn retries_transient_failure_before_any_token_then_succeeds() {
        let transport =
            MockModelTransport::failing("{\"choices\":[{\"delta\":{\"content\":\"hi\"}}]}", 2);
        let mut adapter = OpenAICompatibleAdapter::new("test-model", transport);
        let mut tokens = Vec::new();
        let run = adapter
            .stream_response(&test_request(), &CancelToken::new(), &mut |token| {
                tokens.push(token.to_string())
            })
            .expect("should succeed after retries");

        assert_eq!(run.retry_count, 2);
        assert_eq!(run.content, "hi");
        assert_eq!(tokens.join(""), "hi");
    }

    #[test]
    fn gives_up_after_max_attempts_on_persistent_transient_failure() {
        let transport = MockModelTransport::failing("unused", 10);
        let mut adapter = OpenAICompatibleAdapter::new("test-model", transport);
        let result =
            adapter.stream_response(&test_request(), &CancelToken::new(), &mut |_token| {});

        assert!(result.is_err());
        let error = result.unwrap_err();
        assert!(error.is_retryable());
    }

    #[test]
    fn does_not_retry_non_retryable_failure() {
        let mut transport = MockModelTransport::failing("unused", 1);
        transport.failure_message = "invalid api key".to_string();
        let mut adapter = OpenAICompatibleAdapter::new("test-model", transport);
        let result =
            adapter.stream_response(&test_request(), &CancelToken::new(), &mut |_token| {});

        assert!(result.is_err());
        assert!(!result.unwrap_err().is_retryable());
        // Only the single, non-retried attempt should have reached the transport.
        assert_eq!(adapter.transport.requests.len(), 1);
    }

    #[test]
    fn does_not_retry_after_a_token_has_already_streamed() {
        struct FlakyMidStreamTransport {
            calls: u32,
        }
        impl ModelTransport for FlakyMidStreamTransport {
            fn send(&mut self, _request_body: &str) -> Result<String> {
                unreachable!("send_stream is overridden")
            }
            fn send_stream(
                &mut self,
                _request_body: &str,
                _cancel: &CancelToken,
                on_chunk: &mut dyn FnMut(&str),
            ) -> Result<String> {
                self.calls += 1;
                on_chunk("data: {\"choices\":[{\"delta\":{\"content\":\"partial\"}}]}\n");
                Err(ClientError::Io("connection reset".to_string()))
            }
        }

        let mut adapter =
            OpenAICompatibleAdapter::new("test-model", FlakyMidStreamTransport { calls: 0 });
        let mut tokens = Vec::new();
        let result = adapter.stream_response(&test_request(), &CancelToken::new(), &mut |token| {
            tokens.push(token.to_string())
        });

        assert!(result.is_err());
        assert_eq!(adapter.transport.calls, 1);
        assert_eq!(tokens.join(""), "partial");
    }

    #[test]
    fn model_request_json_includes_tools_when_present() {
        let mut request = test_request();
        request.tools = Some(vec![ToolDefinition {
            name: "run_command".to_string(),
            description: "Run a shell command".to_string(),
            parameters_json:
                "{\"type\":\"object\",\"properties\":{\"command\":{\"type\":\"string\"}}}"
                    .to_string(),
        }]);
        let body = model_request_json(&request);

        assert!(body.contains("\"tools\":[{\"type\":\"function\""));
        assert!(body.contains("\"name\":\"run_command\""));
        assert!(body.contains("\"parameters\":{\"type\":\"object\""));
    }

    #[test]
    fn model_request_json_omits_tools_when_absent() {
        let body = model_request_json(&test_request());
        assert!(!body.contains("\"tools\""));
    }

    #[test]
    fn model_request_json_includes_max_tokens_when_configured() {
        let mut request = test_request();
        request.max_tokens = Some(8192);
        assert!(model_request_json(&request).contains("\"max_tokens\":8192"));
    }

    #[test]
    fn model_request_json_omits_max_tokens_when_absent() {
        assert!(!model_request_json(&test_request()).contains("max_tokens"));
    }

    /// DeepSeek's thinking mode rejects a follow-up request with
    /// `The `reasoning_content` in the thinking mode must be passed back to
    /// the API.` unless the assistant message that made a tool call carries
    /// the reasoning it was produced with. It must therefore survive
    /// serialization.
    #[test]
    fn model_request_json_replays_reasoning_content_on_tool_call_turns() {
        let mut request = test_request();
        request.messages = vec![ModelMessage::assistant_with_tool_calls(
            String::new(),
            vec![ToolCall {
                id: "call_1".to_string(),
                name: "read_git_status".to_string(),
                arguments_json: "{}".to_string(),
            }],
            Some("I should check the working tree first.".to_string()),
        )];

        let body = model_request_json(&request);
        // Parsed rather than substring-matched: the reasoning has to sit on the
        // assistant message itself, and a malformed body would be a fresh 400.
        let parsed: serde_json::Value =
            serde_json::from_str(&body).expect("request body must be valid JSON");
        let message = &parsed["messages"][0];
        assert_eq!(
            message["reasoning_content"].as_str(),
            Some("I should check the working tree first.")
        );
        assert_eq!(message["role"].as_str(), Some("assistant"));
        assert!(message["tool_calls"].is_array());
        assert!(
            parsed["reasoning_content"].is_null(),
            "must not leak to root"
        );
    }

    #[test]
    fn model_request_json_omits_reasoning_content_when_absent() {
        assert!(!model_request_json(&test_request()).contains("reasoning_content"));
    }

    #[test]
    fn extracts_reasoning_content_from_streamed_and_whole_responses() {
        // Streamed thinking arrives fragmented across chunks, exactly like
        // `content`, and must be concatenated in order.
        let streamed = concat!(
            "data: {\"choices\":[{\"delta\":{\"reasoning_content\":\"First \"}}]}\n",
            "data: {\"choices\":[{\"delta\":{\"reasoning_content\":\"I check git.\"}}]}\n",
            "data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"call_1\",\"function\":{\"name\":\"read_git_status\",\"arguments\":\"{}\"}}]}}]}\n",
            "data: [DONE]\n",
        );
        assert_eq!(
            extract_reasoning_content(streamed).as_deref(),
            Some("First I check git.")
        );
        assert_eq!(
            extract_reasoning_content(
                r#"{"choices":[{"message":{"reasoning_content":"Thinking.","content":"hi"}}]}"#
            )
            .as_deref(),
            Some("Thinking.")
        );
    }

    #[test]
    fn reasoning_content_is_none_when_the_model_did_not_think() {
        assert!(
            extract_reasoning_content(r#"{"choices":[{"message":{"content":"hi"}}]}"#).is_none()
        );
    }

    /// Reasoning text must not leak into the visible answer — `extract_model_tokens`
    /// looks for `"content"`, which must not match `"reasoning_content"`.
    #[test]
    fn streamed_reasoning_content_is_not_emitted_as_visible_content() {
        let streamed = concat!(
            "data: {\"choices\":[{\"delta\":{\"reasoning_content\":\"hidden\"}}]}\n",
            "data: {\"choices\":[{\"delta\":{\"content\":\"visible\"}}]}\n",
        );
        assert_eq!(extract_model_tokens(streamed), vec!["visible".to_string()]);
    }

    #[test]
    fn detects_length_finish_reason_in_streamed_and_whole_responses() {
        // Only the final SSE chunk carries the finish reason.
        let streamed = concat!(
            "data: {\"choices\":[{\"delta\":{\"content\":\"partial\"},\"finish_reason\":null}]}\n",
            "data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"length\"}]}\n",
            "data: [DONE]\n",
        );
        assert!(response_was_truncated(streamed));
        assert!(response_was_truncated(
            r#"{"choices":[{"message":{"content":"hi"},"finish_reason":"length"}]}"#
        ));
    }

    #[test]
    fn normal_completion_is_not_reported_as_truncated() {
        let streamed = concat!(
            "data: {\"choices\":[{\"delta\":{\"content\":\"done\"},\"finish_reason\":null}]}\n",
            "data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}]}\n",
            "data: [DONE]\n",
        );
        assert!(!response_was_truncated(streamed));
        assert!(!response_was_truncated(
            r#"{"choices":[{"message":{"content":"hi"},"finish_reason":"tool_calls"}]}"#
        ));
    }

    #[test]
    fn extract_tool_calls_from_non_streaming_response() {
        let raw = r#"{"choices":[{"message":{"tool_calls":[{"id":"call_1","type":"function","function":{"name":"run_command","arguments":"{\"command\":\"git status\"}"}}]}}]}"#;
        let calls = extract_tool_calls(raw);

        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].id, "call_1");
        assert_eq!(calls[0].name, "run_command");
        assert_eq!(calls[0].arguments_json, "{\"command\":\"git status\"}");
    }

    #[test]
    fn extract_tool_calls_concatenates_streamed_argument_fragments() {
        let raw = concat!(
            "data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"call_1\",\"function\":{\"name\":\"run_command\",\"arguments\":\"\"}}]}}]}\n",
            "data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"function\":{\"arguments\":\"{\\\"command\\\":\"}}]}}]}\n",
            "data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"function\":{\"arguments\":\"\\\"git log\\\"}\"}}]}}]}\n",
            "data: [DONE]\n",
        );
        let calls = extract_tool_calls(raw);

        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].id, "call_1");
        assert_eq!(calls[0].name, "run_command");
        assert_eq!(calls[0].arguments_json, "{\"command\":\"git log\"}");
    }

    #[test]
    fn adapter_does_not_error_on_empty_content_when_tool_calls_present() {
        let raw = r#"{"choices":[{"message":{"tool_calls":[{"id":"call_1","function":{"name":"run_command","arguments":"{\"command\":\"pwd\"}"}}]}}]}"#;
        let transport = MockModelTransport::new(raw);
        let mut adapter = OpenAICompatibleAdapter::new("test-model", transport);

        let run = adapter
            .stream_response(&test_request(), &CancelToken::new(), &mut |_token| {})
            .expect("tool-call-only response should not be treated as empty");

        assert!(run.content.is_empty());
        assert_eq!(run.tool_calls.len(), 1);
        assert_eq!(run.tool_calls[0].name, "run_command");
    }
}
