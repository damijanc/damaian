use crate::audit::escape_json as audit_escape_json;
use crate::error::{ClientError, Result};
use crate::hash::{create_id, now_millis};
use std::io::Write;
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
    response: String,
    cancelled: Vec<String>,
}

impl MockModelAdapter {
    pub fn new(response: impl Into<String>) -> Self {
        Self {
            response: response.into(),
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
        for chunk in self.response.as_bytes().chunks(24) {
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
}

impl ModelTransport for CurlModelTransport {
    fn send(&mut self, request_body: &str) -> Result<String> {
        let url = format!("{}/v1/chat/completions", self.base_url);
        let mut child = Command::new("curl")
            .arg("-sS")
            .arg("--no-buffer")
            .arg("-X")
            .arg("POST")
            .arg(url)
            .arg("-H")
            .arg("content-type: application/json")
            .arg("-H")
            .arg(format!("authorization: Bearer {}", self.api_key))
            .arg("-d")
            .arg("@-")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;

        if let Some(mut stdin) = child.stdin.take() {
            stdin.write_all(request_body.as_bytes())?;
        }

        let output = child.wait_with_output()?;
        if !output.status.success() {
            return Err(ClientError::Io(format!(
                "Model provider transport failed: {}",
                String::from_utf8_lossy(&output.stderr)
            )));
        }
        Ok(String::from_utf8_lossy(&output.stdout).to_string())
    }
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
        Self {
            provider: "openai-compatible".to_string(),
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
        let raw = self.transport.send(&body)?;
        let tokens = extract_model_tokens(&raw);
        let mut content = String::new();
        for token in tokens {
            if self.cancelled.contains(&run_id) {
                break;
            }
            content.push_str(&token);
            on_token(&token);
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
    body.push('}');
    body
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
