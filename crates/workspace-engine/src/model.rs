use crate::audit::escape_json as audit_escape_json;
use crate::error::{ClientError, Result};
use crate::hash::{create_id, now_millis};
use std::io::{Read, Write};
use std::process::{Command, Stdio};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelMessage {
    pub role: String,
    pub content: String,
}

impl ModelMessage {
    pub fn system(content: impl Into<String>) -> Self {
        Self {
            role: "system".to_string(),
            content: content.into(),
        }
    }

    pub fn user(content: impl Into<String>) -> Self {
        Self {
            role: "user".to_string(),
            content: content.into(),
        }
    }

    pub fn assistant(content: impl Into<String>) -> Self {
        Self {
            role: "assistant".to_string(),
            content: content.into(),
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
#[derive(Debug, Clone, PartialEq, Eq, Default)]
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
}

pub trait ModelAdapter {
    fn stream_response(
        &mut self,
        request: &ModelRequest,
        on_token: &mut dyn FnMut(&str),
    ) -> Result<ModelRun>;

    fn estimate_tokens(&self, payload: &str) -> usize {
        payload.len().div_ceil(4)
    }
    fn cancel(&mut self, run_id: &str);
}

#[derive(Debug, Clone)]
pub struct MockModelAdapter {
    responses: Vec<String>,
    tool_calls: Vec<Vec<ToolCall>>,
    next_response: usize,
    cancelled: Vec<String>,
}

impl MockModelAdapter {
    pub fn new(response: impl Into<String>) -> Self {
        Self {
            responses: vec![response.into()],
            tool_calls: vec![Vec::new()],
            next_response: 0,
            cancelled: Vec::new(),
        }
    }

    pub fn new_sequence(responses: Vec<String>) -> Self {
        let tool_calls = responses.iter().map(|_| Vec::new()).collect();
        Self {
            responses,
            tool_calls,
            next_response: 0,
            cancelled: Vec::new(),
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
            next_response: 0,
            cancelled: Vec::new(),
        }
    }
}

impl ModelAdapter for MockModelAdapter {
    fn stream_response(
        &mut self,
        request: &ModelRequest,
        on_token: &mut dyn FnMut(&str),
    ) -> Result<ModelRun> {
        let run_id = create_id("modelrun");
        let started_at_ms = now_millis();
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
            if self.cancelled.contains(&run_id) {
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
            incomplete: self.cancelled.contains(&run_id),
            retry_count: 0,
            tool_calls,
        })
    }

    fn cancel(&mut self, run_id: &str) {
        self.cancelled.push(run_id.to_string());
    }
}

pub trait ModelTransport {
    fn send(&mut self, request_body: &str) -> Result<String>;

    fn send_stream(
        &mut self,
        request_body: &str,
        on_chunk: &mut dyn FnMut(&str),
    ) -> Result<String> {
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
            "request = \"POST\"\nurl = \"{}\"\nheader = \"content-type: application/json\"\nheader = \"authorization: Bearer {}\"\ndata-binary = \"{}\"\n",
            escape_curl_config_value(&self.chat_completions_url()),
            escape_curl_config_value(&self.api_key),
            escape_curl_config_value(request_body)
        )
    }
}

impl ModelTransport for CurlModelTransport {
    fn send(&mut self, request_body: &str) -> Result<String> {
        self.send_stream(request_body, &mut |_chunk| {})
    }

    fn send_stream(
        &mut self,
        request_body: &str,
        on_chunk: &mut dyn FnMut(&str),
    ) -> Result<String> {
        let mut child = Command::new("curl")
            .args(Self::curl_args())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;

        if let Some(mut stdin) = child.stdin.take() {
            stdin.write_all(self.curl_config(request_body).as_bytes())?;
        }

        let mut raw = String::new();
        if let Some(mut stdout) = child.stdout.take() {
            let mut buffer = [0_u8; 8192];
            loop {
                let read = stdout.read(&mut buffer)?;
                if read == 0 {
                    break;
                }
                let chunk = String::from_utf8_lossy(&buffer[..read]).to_string();
                raw.push_str(&chunk);
                on_chunk(&chunk);
            }
        }

        let status = child.wait()?;
        let mut stderr = String::new();
        if let Some(mut stderr_pipe) = child.stderr.take() {
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

fn escape_curl_config_value(value: &str) -> String {
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
    cancelled: Vec<String>,
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
            cancelled: Vec::new(),
        }
    }
}

impl<T: ModelTransport> ModelAdapter for OpenAICompatibleAdapter<T> {
    fn stream_response(
        &mut self,
        request: &ModelRequest,
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
                if self.cancelled.contains(&run_id) {
                    return;
                }
                emitted_any = true;
                content.push_str(&token);
                on_token(&token);
            };
            let send_result = self.transport.send_stream(&body, &mut |chunk| {
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
        if content.is_empty() && tool_calls.is_empty() && !self.cancelled.contains(&run_id) {
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
            incomplete: self.cancelled.contains(&run_id),
            retry_count,
            tool_calls,
        })
    }

    fn cancel(&mut self, run_id: &str) {
        self.cancelled.push(run_id.to_string());
    }
}

pub fn model_request_json(request: &ModelRequest) -> String {
    let messages = request
        .messages
        .iter()
        .map(|message| {
            format!(
                "{{\"role\":\"{}\",\"content\":\"{}\"}}",
                audit_escape_json(&message.role),
                audit_escape_json(&message.content)
            )
        })
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
                .or_else(|| choice.get("message").and_then(|message| message.get("tool_calls")))
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
                    if let Some(arguments) = function.get("arguments").and_then(|value| value.as_str())
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
        }
    }

    #[test]
    fn retries_transient_failure_before_any_token_then_succeeds() {
        let transport = MockModelTransport::failing("{\"choices\":[{\"delta\":{\"content\":\"hi\"}}]}", 2);
        let mut adapter = OpenAICompatibleAdapter::new("test-model", transport);
        let mut tokens = Vec::new();
        let run = adapter
            .stream_response(&test_request(), &mut |token| tokens.push(token.to_string()))
            .expect("should succeed after retries");

        assert_eq!(run.retry_count, 2);
        assert_eq!(run.content, "hi");
        assert_eq!(tokens.join(""), "hi");
    }

    #[test]
    fn gives_up_after_max_attempts_on_persistent_transient_failure() {
        let transport = MockModelTransport::failing("unused", 10);
        let mut adapter = OpenAICompatibleAdapter::new("test-model", transport);
        let result = adapter.stream_response(&test_request(), &mut |_token| {});

        assert!(result.is_err());
        let error = result.unwrap_err();
        assert!(error.is_retryable());
    }

    #[test]
    fn does_not_retry_non_retryable_failure() {
        let mut transport = MockModelTransport::failing("unused", 1);
        transport.failure_message = "invalid api key".to_string();
        let mut adapter = OpenAICompatibleAdapter::new("test-model", transport);
        let result = adapter.stream_response(&test_request(), &mut |_token| {});

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
        let result =
            adapter.stream_response(&test_request(), &mut |token| tokens.push(token.to_string()));

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
            parameters_json: "{\"type\":\"object\",\"properties\":{\"command\":{\"type\":\"string\"}}}"
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
            .stream_response(&test_request(), &mut |_token| {})
            .expect("tool-call-only response should not be treated as empty");

        assert!(run.content.is_empty());
        assert_eq!(run.tool_calls.len(), 1);
        assert_eq!(run.tool_calls[0].name, "run_command");
    }
}
