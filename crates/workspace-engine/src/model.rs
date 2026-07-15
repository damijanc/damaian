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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelRequest {
    pub provider: String,
    pub model: String,
    pub messages: Vec<ModelMessage>,
    pub temperature: Option<String>,
    pub reasoning_level: Option<String>,
    pub stream: bool,
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
    next_response: usize,
    cancelled: Vec<String>,
}

impl MockModelAdapter {
    pub fn new(response: impl Into<String>) -> Self {
        Self {
            responses: vec![response.into()],
            next_response: 0,
            cancelled: Vec::new(),
        }
    }

    pub fn new_sequence(responses: Vec<String>) -> Self {
        Self {
            responses,
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
        let response = self
            .responses
            .get(self.next_response)
            .or_else(|| self.responses.last())
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
}

impl MockModelTransport {
    pub fn new(response: impl Into<String>) -> Self {
        Self {
            response: response.into(),
            requests: Vec::new(),
        }
    }
}

impl ModelTransport for MockModelTransport {
    fn send(&mut self, request_body: &str) -> Result<String> {
        self.requests.push(request_body.to_string());
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
        let run_id = create_id("modelrun");
        let started_at_ms = now_millis();
        let body = model_request_json(request);
        let mut content = String::new();
        let mut buffered_stream = String::new();
        let mut saw_sse_stream = false;
        let mut emit_token = |token: String| {
            if self.cancelled.contains(&run_id) {
                return;
            }
            content.push_str(&token);
            on_token(&token);
        };
        let raw = self.transport.send_stream(&body, &mut |chunk| {
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
        })?;
        if let Some(message) = extract_error_message(&raw) {
            return Err(ClientError::Io(format!("Model provider error: {message}")));
        }
        if saw_sse_stream {
            for token in extract_model_tokens(&buffered_stream) {
                emit_token(token);
            }
        } else {
            for token in extract_model_tokens(&raw) {
                emit_token(token);
            }
        }
        if content.is_empty() && !self.cancelled.contains(&run_id) {
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
}
