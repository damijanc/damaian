use crate::audit::escape_json as audit_escape_json;
use crate::cancel::CancelToken;
use crate::error::{ClientError, ProviderRefusal, Result};
use crate::hash::{create_id, now_millis};
use crate::process_registry::{ProcessKind, ProcessRegistry, RegistrationHandle};
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::{Read, Write};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
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

/// Refusal retries beyond the first, per spec 48 §5.3: enough to cross a
/// per-minute rate-limit window without turning the turn into a hang.
const MAX_REFUSAL_ATTEMPTS: u32 = 4;
/// The total time a single call may spend waiting on refusal retries. A
/// provider asking for more than this is refused rather than slept through.
const RETRY_WAIT_CEILING: Duration = Duration::from_secs(90);

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

    /// Attaches the thinking-mode reasoning behind this turn. Needed on every
    /// assistant message replayed into a later round — not just the ones
    /// carrying `tool_calls` — since a turn the model requested through the
    /// `DAMAIAN_COMMAND_V1` text envelope, or one whose tool call didn't
    /// decode, goes back as plain assistant text and DeepSeek's thinking mode
    /// rejects the request over any assistant message that lost its reasoning.
    /// Pass the originating [`ModelRun::reasoning_content`] straight through;
    /// `None` leaves the message unchanged.
    #[must_use]
    pub fn with_reasoning_content(mut self, reasoning_content: Option<String>) -> Self {
        self.reasoning_content = reasoning_content;
        self
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

/// Whether a token figure came from the provider or from a local
/// approximation.
///
/// Per run rather than per task, because one task mixes them: a provider that
/// reports usage on a completed call reports nothing for a call whose stream
/// was cut, and that run's figure is an estimate while its siblings are
/// measured. Spec 19 §5.1.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum UsageSource {
    /// Reported by the provider for this call.
    Measured,
    /// Derived locally from payload size. Never presented as measured.
    Estimated,
}

impl UsageSource {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Measured => "measured",
            Self::Estimated => "estimated",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "measured" => Some(Self::Measured),
            "estimated" => Some(Self::Estimated),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TokenUsage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub source: UsageSource,
}

impl TokenUsage {
    /// The only zero that is a fact rather than a guess: no request was sent,
    /// so nothing was billed.
    pub fn measured_zero() -> Self {
        Self {
            input_tokens: 0,
            output_tokens: 0,
            source: UsageSource::Measured,
        }
    }

    pub fn estimated(input_tokens: u64, output_tokens: u64) -> Self {
        Self {
            input_tokens,
            output_tokens,
            source: UsageSource::Estimated,
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
    /// Native tool/function definitions to offer the model. Only meaningful
    /// when the active provider is configured with
    /// `ModelProviderConfig::supports_native_tools`; otherwise callers
    /// should leave this `None` and rely on the `DAMAIAN_COMMAND_V1` text
    /// envelope instead.
    pub tools: Option<Vec<ToolDefinition>>,
    /// Explicit output-token ceiling. `None` omits `max_tokens` and lets the
    /// provider apply its own default.
    pub max_tokens: Option<u32>,
    /// Ask the provider to report token usage on the stream. Only meaningful
    /// together with [`Self::stream`]: an OpenAI-compatible API omits usage
    /// from a stream unless `stream_options` asks for it, and the field is
    /// both pointless and sometimes rejected on a non-streaming call.
    /// Spec 19 §5.2.
    pub request_usage: bool,
}

// No `Eq`: `reported_cost` is an `Option<f64>`. Nothing uses a run as a map key
// or in a set, and `PartialEq` is what `assert_eq!` needs.
#[derive(Debug, Clone, PartialEq)]
pub struct ModelRun {
    pub run_id: String,
    pub provider: String,
    pub model: String,
    pub started_at_ms: u128,
    pub completed_at_ms: u128,
    pub content: String,
    pub incomplete: bool,
    pub retry_count: u32,
    /// Refusal retries — attempts the provider rejected before generating
    /// anything, retried under spec 48's bounds. Kept separate from
    /// [`Self::retry_count`] because a refused attempt was billed for nothing,
    /// whereas a connection retry sent the body and may have been billed: the
    /// caller books them differently (spec 48 §5.5, context.md §3.7).
    pub refusal_retries: u32,
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
    /// What this call cost in tokens. Always populated: measured when the
    /// provider reported it, estimated when it did not. Spec 19 §5.1.
    pub usage: TokenUsage,
    /// Cost as the provider reported it. `None` is the normal case — almost no
    /// provider reports cost on a chat completion — and means "this provider
    /// did not tell us", never "free".
    pub reported_cost: Option<f64>,
    /// This call is the one that discovered the provider rejects a usage
    /// request, and was retried without it. True exactly once per provider per
    /// process, so a caller that audits on it audits once.
    ///
    /// Reported rather than audited here for the reason `SessionStore` gives
    /// for not holding an `AuditLog`: threading one through every adapter
    /// construction site is a large amount of churn for a diagnostic, and the
    /// orchestrator that owns the turn already has one.
    pub usage_reporting_unsupported: bool,
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
            refusal_retries: 0,
            tool_calls: Vec::new(),
            truncated: false,
            reasoning_content: None,
            usage: TokenUsage::measured_zero(),
            reported_cost: None,
            usage_reporting_unsupported: false,
        }
    }
}

pub trait ModelAdapter {
    fn stream_response(
        &mut self,
        request: &ModelRequest,
        cancel: &CancelToken,
        on_token: &mut dyn FnMut(&str),
        on_wait: &mut dyn FnMut(u64),
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
        _on_wait: &mut dyn FnMut(u64),
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
        // Before the struct literal moves `content` out of scope. The mock is
        // not a provider and reports no usage, so its figure is an estimate
        // like any other unreported call.
        let usage = TokenUsage::estimated(
            model_request_json(request).len().div_ceil(4) as u64,
            content.len().div_ceil(4) as u64,
        );

        Ok(ModelRun {
            run_id: run_id.clone(),
            provider: "mock".to_string(),
            model: request.model.clone(),
            started_at_ms,
            completed_at_ms: now_millis(),
            content,
            incomplete: cancel.is_cancelled(),
            retry_count: 0,
            refusal_retries: 0,
            tool_calls,
            truncated: self.truncated.get(index).copied().unwrap_or(false),
            reasoning_content: self.reasoning_content.get(index).cloned().flatten(),
            usage,
            reported_cost: None,
            usage_reporting_unsupported: false,
        })
    }
}

/// What the transport knows about the most recent response: the HTTP status and
/// the retry signal. Both are `Option` because a transport that cannot see them
/// (a mock, or curl dying before the response) must say so rather than invent a
/// 200 — a fabricated success is how a classifier would silently assert the
/// provider answered. Spec 48 §5.1.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ResponseMeta {
    /// `None` when the transport cannot report one. Never defaulted to 200.
    pub status: Option<u16>,
    /// Seconds to wait, parsed from `Retry-After` in either of its two forms.
    pub retry_after_secs: Option<u64>,
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

    /// Metadata for the most recent `send_stream`. Default: nothing known.
    fn last_response_meta(&self) -> ResponseMeta {
        ResponseMeta::default()
    }
}

#[derive(Debug, Clone)]
pub struct CurlModelTransport {
    pub base_url: String,
    pub api_key: String,
    /// So a `curl` streaming a paid-for completion is swept if this process is
    /// killed before `KillOnDrop` can run.
    pub(crate) registry: ProcessRegistry,
    /// Attribution for the spawned `curl`. Empty where the caller has no
    /// session — a settings-screen connection test, say.
    pub(crate) session_id: String,
    /// Where the per-call `dump-header` file is written, under the data
    /// directory. Read after the call and removed on every path, so a status is
    /// available to the classifier without contaminating the token stream.
    tmp_dir: PathBuf,
    /// The metadata of the most recent `send_stream`, cleared before each send
    /// so a caller reading it after a failure never sees the previous call's.
    last_meta: ResponseMeta,
}

impl CurlModelTransport {
    pub fn new(
        base_url: impl Into<String>,
        api_key: impl Into<String>,
        registry: ProcessRegistry,
        data_dir: impl AsRef<Path>,
    ) -> Self {
        let tmp_dir = data_dir.as_ref().join("tmp");
        // Best-effort: `send_stream` surfaces a transport error if curl cannot
        // write the header file, and a missing directory would be that.
        let _ = fs::create_dir_all(&tmp_dir);
        Self {
            base_url: base_url.into().trim_end_matches('/').to_string(),
            api_key: api_key.into(),
            registry,
            session_id: String::new(),
            tmp_dir,
            last_meta: ResponseMeta::default(),
        }
    }

    /// Attributes any `curl` this transport spawns to a session.
    pub fn for_session(mut self, session_id: &str) -> Self {
        self.session_id = session_id.to_string();
        self
    }

    fn curl_args() -> [&'static str; 4] {
        ["-sS", "--no-buffer", "--config", "-"]
    }

    fn chat_completions_url(&self) -> String {
        format!("{}/v1/chat/completions", self.base_url)
    }

    fn curl_config(&self, request_body: &str, header_path: &Path) -> String {
        format!(
            "request = \"POST\"\nurl = \"{}\"\nheader = \"content-type: application/json\"\nheader = \"authorization: Bearer {}\"\ndata-binary = \"{}\"\ndump-header = \"{}\"\nconnect-timeout = {CONNECT_TIMEOUT_SECS}\nspeed-limit = 1\nspeed-time = {STALL_TIMEOUT_SECS}\nmax-time = {MAX_TIME_SECS}\n",
            escape_curl_config_value(&self.chat_completions_url()),
            escape_curl_config_value(&self.api_key),
            escape_curl_config_value(request_body),
            escape_curl_config_value(&header_path.display().to_string())
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

        // Fresh per call, so two concurrent calls cannot collide on one file,
        // and cleared before the send so a caller reading the metadata after a
        // failure never sees the previous call's answer.
        self.last_meta = ResponseMeta::default();
        let header_path = self
            .tmp_dir
            .join(format!("{}.headers", create_id("headers")));
        // The file is removed on every exit, success or error, by the guard.
        let _header_guard = HeaderFileGuard {
            path: header_path.clone(),
        };

        let spawned = Command::new("curl")
            .args(Self::curl_args())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            // Its own group, so a sweep reaches the whole call rather than a
            // leader that may have exec'd.
            .process_group(0)
            .spawn()?;
        // Before the request body is written: a `curl` that is running and
        // unrecorded cannot be swept, and this one is billing.
        let registration =
            self.registry
                .register(ProcessKind::ModelCall, &self.session_id, spawned.id())?;
        let mut child = KillOnDrop {
            child: spawned,
            _registration: registration,
        };

        if let Some(mut stdin) = child.child().stdin.take() {
            stdin.write_all(self.curl_config(request_body, &header_path).as_bytes())?;
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

        // Read before the guard removes the file, so the classifier has the
        // status and `Retry-After` even once the file is gone.
        self.last_meta = read_header_file(&header_path);

        if !status.success() {
            return Err(ClientError::Io(format!(
                "Model provider transport failed: {}",
                stderr
            )));
        }
        Ok(raw)
    }

    fn last_response_meta(&self) -> ResponseMeta {
        self.last_meta.clone()
    }
}

/// Removes the `dump-header` file on every exit path, so a crashed or cancelled
/// call does not leave response headers on disk. Created only after the request
/// is sent, so a removal of a file that never existed is the normal no-op.
struct HeaderFileGuard {
    path: PathBuf,
}

impl Drop for HeaderFileGuard {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

/// The status and `Retry-After` from a curl `dump-header` file, or nothing when
/// the file is absent or unreadable. A missing file is "the transport could not
/// report", never a fabricated 200.
fn read_header_file(path: &Path) -> ResponseMeta {
    let Ok(content) = fs::read_to_string(path) else {
        return ResponseMeta::default();
    };
    ResponseMeta {
        status: parse_status_line(&content),
        retry_after_secs: parse_retry_after_header(&content),
    }
}

/// The HTTP status from a header file's first line: `HTTP/2 429` or
/// `HTTP/1.1 200 OK`.
fn parse_status_line(header: &str) -> Option<u16> {
    let first = header.lines().next()?;
    let mut fields = first.split_whitespace();
    let _version = fields.next()?;
    fields.next()?.parse().ok()
}

/// The `retry-after` header, case-insensitive, parsed into seconds.
fn parse_retry_after_header(header: &str) -> Option<u64> {
    header.lines().skip(1).find_map(|line| {
        let (name, value) = line.split_once(':')?;
        name.trim()
            .eq_ignore_ascii_case("retry-after")
            .then_some(parse_retry_after(value.trim()))?
    })
}

/// Parses a `Retry-After` value into seconds, in either of RFC 9110's two
/// forms: a non-negative delta-seconds integer, or an HTTP-date in
/// IMF-fixdate format. `None` for anything else, including a date already past,
/// so a caller treats it as "no guidance" rather than a negative wait.
fn parse_retry_after(value: &str) -> Option<u64> {
    let value = value.trim();
    if value.is_empty() {
        return None;
    }
    if let Ok(seconds) = value.parse::<u64>() {
        return Some(seconds);
    }
    let deadline = parse_http_date(value)?;
    let remaining = deadline - now_millis() as i64 / 1000;
    (remaining > 0).then_some(remaining as u64)
}

/// An IMF-fixdate HTTP-date (`Sun, 06 Nov 1994 08:49:37 GMT`) to epoch seconds.
fn parse_http_date(value: &str) -> Option<i64> {
    let (_, rest) = value.split_once(',')?;
    let mut fields = rest.split_whitespace();
    let day: i64 = fields.next()?.parse().ok()?;
    let month = month_number(fields.next()?)?;
    let year: i64 = fields.next()?.parse().ok()?;
    let mut time = fields.next()?.split(':');
    let hour: i64 = time.next()?.parse().ok()?;
    let minute: i64 = time.next()?.parse().ok()?;
    let second: i64 = time.next()?.parse().ok()?;
    let days = days_from_civil(year, month, day);
    Some(days * 86_400 + hour * 3_600 + minute * 60 + second)
}

fn month_number(month: &str) -> Option<i64> {
    Some(match month {
        "Jan" => 1,
        "Feb" => 2,
        "Mar" => 3,
        "Apr" => 4,
        "May" => 5,
        "Jun" => 6,
        "Jul" => 7,
        "Aug" => 8,
        "Sep" => 9,
        "Oct" => 10,
        "Nov" => 11,
        "Dec" => 12,
        _ => return None,
    })
}

/// Days since 1970-01-01 for a civil date (Howard Hinnant's algorithm).
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
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
///
/// The second field removes the registry entry on the same path. A `SIGKILL`
/// skips this entirely, which is exactly why the entry is written at spawn:
/// the next launch's sweep is what catches that case.
struct KillOnDrop {
    child: Child,
    /// Held, never read: dropping it is what removes the registry entry.
    _registration: RegistrationHandle,
}

impl KillOnDrop {
    fn child(&mut self) -> &mut Child {
        &mut self.child
    }
}

impl Drop for KillOnDrop {
    fn drop(&mut self) {
        // Already-exited is the normal case and reports an error here; either
        // way there is nothing to recover from at drop time.
        let _ = self.child.kill();
        let _ = self.child.wait();
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
    /// Responses handed out in order, one per call, the last one repeating.
    /// Empty means [`Self::response`] answers every call, which is what every
    /// caller predating this field expects.
    ///
    /// Needed because a capability probe is defined by what the *second* call
    /// returns — a provider rejecting `stream_options` and then accepting the
    /// same request without it — and one `response` cannot express that.
    pub responses: Vec<String>,
    pub next_response: usize,
    /// The HTTP status the double reports for its next response, so a test can
    /// exercise the status-first classification path without real curl.
    pub status: Option<u16>,
    /// The `Retry-After` seconds the double reports alongside `status`.
    pub retry_after_secs: Option<u64>,
    /// Per-response statuses, parallel to [`Self::responses`], set on each
    /// `send` so a sequenced double can express "429 then 200".
    statuses: Vec<Option<u16>>,
    /// Per-response `Retry-After`, parallel to [`Self::responses`].
    retry_afters: Vec<Option<u64>>,
}

impl MockModelTransport {
    pub fn new(response: impl Into<String>) -> Self {
        Self {
            response: response.into(),
            requests: Vec::new(),
            fail_before_success: 0,
            failure_message: "connection reset by peer".to_string(),
            responses: Vec::new(),
            next_response: 0,
            status: None,
            retry_after_secs: None,
            statuses: Vec::new(),
            retry_afters: Vec::new(),
        }
    }

    pub fn failing(response: impl Into<String>, fail_before_success: u32) -> Self {
        Self {
            fail_before_success,
            ..Self::new(response)
        }
    }

    /// Answers each call with the next response in order, repeating the last.
    pub fn sequence(responses: Vec<String>) -> Self {
        Self {
            responses,
            ..Self::new(String::new())
        }
    }

    /// A sequence whose responses each carry their own status and `Retry-After`,
    /// so a test can express "429 then 200" — the shape a refusal retry needs.
    pub fn sequence_with_status(responses: Vec<(String, Option<u16>, Option<u64>)>) -> Self {
        let mut bodies = Vec::with_capacity(responses.len());
        let mut statuses = Vec::with_capacity(responses.len());
        let mut retry_afters = Vec::with_capacity(responses.len());
        for (body, status, retry_after) in responses {
            bodies.push(body);
            statuses.push(status);
            retry_afters.push(retry_after);
        }
        Self {
            responses: bodies,
            statuses,
            retry_afters,
            ..Self::new(String::new())
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
        if self.responses.is_empty() {
            return Ok(self.response.clone());
        }
        let index = self.next_response.min(self.responses.len() - 1);
        self.next_response += 1;
        self.status = self.statuses.get(index).copied().flatten();
        self.retry_after_secs = self.retry_afters.get(index).copied().flatten();
        Ok(self.responses[index].clone())
    }

    fn last_response_meta(&self) -> ResponseMeta {
        ResponseMeta {
            status: self.status,
            retry_after_secs: self.retry_after_secs,
        }
    }
}

pub struct OpenAICompatibleAdapter<T: ModelTransport> {
    provider: String,
    model: String,
    transport: T,
    /// `None` until this provider has been observed, then the answer for the
    /// rest of the process, so the probe in [`Self::stream_response`] happens
    /// once rather than on every turn. Spec 19 §5.2. Phase 1 WP3's capability
    /// profile is the durable home for this observation; the shape is chosen
    /// so WP3 can adopt it rather than rediscover it.
    supports_usage: Option<bool>,
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
            supports_usage: None,
        }
    }

    /// Whether this provider is believed to accept a usage request. `true`
    /// until observed otherwise: the field is standard, and assuming it is
    /// absent would mean never measuring anything.
    pub fn probe_supports_usage(&self) -> bool {
        self.supports_usage.unwrap_or(true)
    }

    /// One send, with the existing connection-level retry policy.
    ///
    /// Extracted so the usage probe can run the same send twice — once asking
    /// for usage, once not — without a second definition of how a request is
    /// sent. Returns the whole body, the streamed content, and how many
    /// attempts beyond the first it took.
    fn send_with_retries(
        &mut self,
        body: &str,
        cancel: &CancelToken,
        on_token: &mut dyn FnMut(&str),
    ) -> Result<(String, String, u32)> {
        const MAX_ATTEMPTS: u32 = 3;
        const RETRY_BACKOFF_MS: [u64; 2] = [500, 1500];

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
            let send_result = self.transport.send_stream(body, cancel, &mut |chunk| {
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

        Ok((raw, content, attempt - 1))
    }
}

/// The provider is saying it does not understand the usage request, as opposed
/// to any of the other things a provider says no to.
fn mentions_unsupported_usage_option(message: &str) -> bool {
    let lowered = message.to_lowercase();
    lowered.contains("stream_options") || lowered.contains("include_usage")
}

/// A refusal's user-facing message: the provider's own words when it sent any,
/// else the classification's code, plus how many attempts were spent. The
/// provider's message is the only place the user learns what to fix, so it is
/// carried verbatim rather than paraphrased.
fn refusal_message(refusal: &ProviderRefusal, raw: &str, attempts: u32) -> String {
    let detail = extract_error_message(raw).unwrap_or_else(|| refusal.code().to_string());
    format!("{detail} (provider refused after {attempts} attempts)")
}

/// A refusal retry's sleep in seconds: exponential from 1s, with clock-derived
/// jitter so two Damaian windows that hit the same limit do not retry in
/// lockstep. Spec 48 §5.3.
fn refusal_backoff_secs(attempt: u32) -> u64 {
    let base_secs = 1u64 << attempt.min(4); // 1, 2, 4, 8
    let jitter_ms = (now_millis() % (base_secs * 1000) as u128) as u64;
    base_secs + jitter_ms / 1000
}

/// Sleeps `seconds`, polling the stop flag every [`CANCEL_POLL_INTERVAL`], so a
/// user watching "retrying in 47s" can stop it rather than wait the whole
/// duration out.
fn wait_cancellable(seconds: u64, cancel: &CancelToken) -> Result<()> {
    let deadline = std::time::Instant::now() + Duration::from_secs(seconds);
    loop {
        cancel.check()?;
        if std::time::Instant::now() >= deadline {
            return Ok(());
        }
        std::thread::sleep(CANCEL_POLL_INTERVAL);
    }
}

impl<T: ModelTransport> ModelAdapter for OpenAICompatibleAdapter<T> {
    fn stream_response(
        &mut self,
        request: &ModelRequest,
        cancel: &CancelToken,
        on_token: &mut dyn FnMut(&str),
        on_wait: &mut dyn FnMut(u64),
    ) -> Result<ModelRun> {
        let run_id = create_id("modelrun");
        let started_at_ms = now_millis();

        // At most two passes: one asking for usage, and — only if the provider
        // says it does not know the field — one without it. Spec 19 §5.2.
        let mut ask_for_usage = request.request_usage && self.probe_supports_usage();
        let mut usage_reporting_unsupported = false;
        let mut total_retries: u32 = 0;
        let mut refusal_attempts: u32 = 0;
        let refusal_deadline = std::time::Instant::now() + RETRY_WAIT_CEILING;
        let (raw, content, retry_count, body) = loop {
            let body = model_request_json(&ModelRequest {
                request_usage: ask_for_usage,
                ..request.clone()
            });
            let (raw, content, retry_count) = self.send_with_retries(&body, cancel, on_token)?;
            total_retries += retry_count;

            // A provider that rejects `stream_options` says so in the body of
            // an error response, not through a transport failure: `curl -sS`
            // exits zero on a 4xx, so the status never reaches the connection
            // layer. Checked *before* the refusal classification, because its
            // 400-shaped body is a capability probe, not a refusal.
            if let Some(message) = extract_error_message(&raw)
                && ask_for_usage
                && mentions_unsupported_usage_option(&message)
            {
                self.supports_usage = Some(false);
                usage_reporting_unsupported = true;
                ask_for_usage = false;
                continue;
            }

            // Classify the refusal from status first, body second (spec 48
            // §5.2). This runs only after `send_with_retries` returned `Ok`,
            // which is where a provider 429 actually arrives.
            if let Some(refusal) = classify_refusal(&self.transport.last_response_meta(), &raw) {
                // Mid-stream: tokens already reached the user, so retrying
                // would blend two responses. Treated as permanent; the caller
                // books what streamed (spec 48 §5.5).
                let retryable = refusal.is_transient() && content.is_empty();
                if !retryable {
                    let message = refusal_message(&refusal, &raw, refusal_attempts + 1);
                    return Err(ClientError::Provider(refusal, message));
                }
                if refusal_attempts >= MAX_REFUSAL_ATTEMPTS
                    || std::time::Instant::now() >= refusal_deadline
                {
                    let message = refusal_message(&refusal, &raw, refusal_attempts + 1);
                    return Err(ClientError::Provider(refusal, message));
                }

                let remaining =
                    refusal_deadline.saturating_duration_since(std::time::Instant::now());
                let wait = match refusal.retry_after_secs() {
                    Some(retry_after) => {
                        if retry_after > remaining.as_secs() {
                            // The provider's figure is not slept through: eleven
                            // minutes is indistinguishable from a hang, so the
                            // call fails carrying that figure.
                            let message = format!(
                                "Provider asked to wait {retry_after}s, beyond the \
                                 {}s retry ceiling",
                                RETRY_WAIT_CEILING.as_secs()
                            );
                            return Err(ClientError::Provider(refusal, message));
                        }
                        retry_after
                    }
                    None => refusal_backoff_secs(refusal_attempts),
                };

                on_wait(wait);
                wait_cancellable(wait, cancel)?;
                refusal_attempts += 1;
                continue;
            }

            // Not a refusal: any other error object is a plain provider error.
            if let Some(message) = extract_error_message(&raw) {
                return Err(ClientError::Io(format!("Model provider error: {message}")));
            }
            if ask_for_usage && extract_usage(&raw).is_some() {
                self.supports_usage = Some(true);
            }
            break (raw, content, total_retries, body);
        };

        let tool_calls = extract_tool_calls(&raw);
        if content.is_empty() && tool_calls.is_empty() && !cancel.is_cancelled() {
            return Err(ClientError::Io(
                "Model provider returned no assistant content".to_string(),
            ));
        }

        // Before the struct literal moves `content` out of scope.
        let reported = extract_usage(&raw);
        let usage = match reported {
            Some((input_tokens, output_tokens, _)) => TokenUsage {
                input_tokens,
                output_tokens,
                source: UsageSource::Measured,
            },
            None => TokenUsage::estimated(
                self.estimate_tokens(&body) as u64,
                self.estimate_tokens(&content) as u64,
            ),
        };

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
            refusal_retries: refusal_attempts,
            tool_calls,
            truncated: response_was_truncated(&raw),
            reasoning_content: extract_reasoning_content(&raw),
            usage,
            reported_cost: reported.and_then(|(_, _, cost)| cost),
            usage_reporting_unsupported,
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
    // Streaming only: `stream_options` is meaningless on a non-streaming call
    // and rejected outright by some providers.
    if request.request_usage && request.stream {
        body.push_str(",\"stream_options\":{\"include_usage\":true}");
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

/// The provider's own token figures, when it reported any: input, output, and
/// the cost it charged if it says.
///
/// Reads the whole body rather than hooking the incremental reader. Usage
/// arrives on a final chunk whose `choices` array is empty, which
/// [`extract_model_tokens`] already passes over, and `raw` holds the complete
/// stream by the time this is called — the same way `extract_tool_calls` and
/// `response_was_truncated` read it. The last `usage` object wins, so a
/// provider that repeats it per chunk reports its final total rather than its
/// first partial one.
///
/// `prompt_tokens`/`completion_tokens` is the OpenAI naming;
/// `input_tokens`/`output_tokens` is accepted as an alias, because providers
/// differ and a missed alias silently downgrades a measured figure to an
/// estimate.
pub fn extract_usage(raw: &str) -> Option<(u64, u64, Option<f64>)> {
    let mut found = None;
    for payload in usage_payloads(raw) {
        let Ok(value) = serde_json::from_str::<serde_json::Value>(&payload) else {
            continue;
        };
        let Some(usage) = value.get("usage").filter(|usage| !usage.is_null()) else {
            continue;
        };
        let input = usage
            .get("prompt_tokens")
            .or_else(|| usage.get("input_tokens"))
            .and_then(serde_json::Value::as_u64);
        let output = usage
            .get("completion_tokens")
            .or_else(|| usage.get("output_tokens"))
            .and_then(serde_json::Value::as_u64);
        // Both or neither. A half-read usage object would become a measured
        // figure with a fabricated zero in it.
        if let (Some(input), Some(output)) = (input, output) {
            let cost = usage.get("cost").and_then(serde_json::Value::as_f64);
            found = Some((input, output, cost));
        }
    }
    found
}

/// The JSON payloads of a response body, whether it is an SSE stream or a
/// single non-streaming object.
fn usage_payloads(raw: &str) -> Vec<String> {
    if !raw.contains("data:") {
        return vec![raw.to_string()];
    }
    raw.lines()
        .map(str::trim)
        .filter(|line| line.starts_with("data:"))
        .map(|line| line.trim_start_matches("data:").trim().to_string())
        .filter(|payload| payload != "[DONE]")
        .collect()
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

/// Classifies a provider's refusal, status first and body second — never by
/// substring search over prose. `None` means "not a refusal": a 2xx with no
/// error object, or a transport that reported neither a status nor an error
/// body. Spec 48 §5.2.
pub fn classify_refusal(meta: &ResponseMeta, raw: &str) -> Option<ProviderRefusal> {
    if let Some(status) = meta.status {
        match status {
            429 => {
                return Some(ProviderRefusal::RateLimited {
                    retry_after_secs: meta.retry_after_secs,
                });
            }
            500 | 502 | 503 | 504 => {
                return Some(ProviderRefusal::Overloaded {
                    retry_after_secs: meta.retry_after_secs,
                });
            }
            402 => return Some(ProviderRefusal::QuotaExhausted),
            401 | 403 => return Some(ProviderRefusal::AuthFailed),
            400 | 404 | 422 => return Some(ProviderRefusal::BadRequest),
            // Any other 4xx is a refusal we cannot classify from its status
            // alone; the body may name a quota, otherwise it is permanent.
            400..=499 => {
                return Some(if body_mentions_quota(raw) {
                    ProviderRefusal::QuotaExhausted
                } else {
                    ProviderRefusal::Unknown
                });
            }
            // A 5xx outside the overload list is still a refusal, just not one
            // we can promise is transient.
            500..=599 => return Some(ProviderRefusal::Unknown),
            // 1xx, 2xx, 3xx: not a refusal by status. Fall through to the body
            // check, for a provider that returns 200 with an error object.
            _ => {}
        }
    }
    classify_refusal_from_body(raw)
}

/// The body-only fallback: a provider error object's structured `code`/`type`
/// field, examined only when no status classified the response. Free prose is
/// never the signal — a message mentioning "connection" or a request id
/// carrying "429" must not drive a refusal, which is the point of reading the
/// `code`/`type` field rather than the `message`.
fn classify_refusal_from_body(raw: &str) -> Option<ProviderRefusal> {
    if !raw.contains("\"error\"") {
        return None;
    }
    let code = extract_string_field(raw, "code")
        .or_else(|| extract_string_field(raw, "type"))
        .unwrap_or_default()
        .to_lowercase();
    Some(
        if code.contains("rate_limit") || code.contains("too_many") {
            ProviderRefusal::RateLimited {
                retry_after_secs: None,
            }
        } else if code.contains("quota") || code.contains("billing") || code.contains("balance") {
            ProviderRefusal::QuotaExhausted
        } else if code.contains("auth")
            || code.contains("api_key")
            || code.contains("unauthorized")
            || code.contains("forbidden")
        {
            ProviderRefusal::AuthFailed
        } else if code.contains("server")
            || code.contains("overload")
            || code.contains("unavailable")
            || code.contains("timeout")
        {
            ProviderRefusal::Overloaded {
                retry_after_secs: None,
            }
        } else if code.contains("invalid") || code.contains("not_found") {
            ProviderRefusal::BadRequest
        } else {
            ProviderRefusal::Unknown
        },
    )
}

/// Whether an error object's structured field names an exhausted balance or
/// quota, for the "4xx whose body says quota" case in §5.2.
fn body_mentions_quota(raw: &str) -> bool {
    extract_string_field(raw, "code")
        .or_else(|| extract_string_field(raw, "type"))
        .is_some_and(|value| {
            let lowered = value.to_lowercase();
            lowered.contains("quota") || lowered.contains("billing") || lowered.contains("balance")
        })
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

    /// A registry over a scratch directory, so a transport can be built in a
    /// test without writing to the user's real data directory. None of these
    /// tests spawn `curl`, so nothing is ever recorded in it.
    fn test_registry() -> ProcessRegistry {
        let dir = std::env::temp_dir().join(format!(
            "damaian-model-registry-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        ProcessRegistry::open(dir).expect("scratch registry")
    }

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
        let mut transport = CurlModelTransport::new(
            "https://api.example.test/",
            "sk_test",
            test_registry(),
            test_data_dir(),
        );
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
        let transport = CurlModelTransport::new(
            "https://api.example.test/",
            "sk_test",
            test_registry(),
            test_data_dir(),
        );
        let config = transport.curl_config(
            "{\"model\":\"test\",\"messages\":[]}",
            Path::new("h.headers"),
        );

        assert!(config.contains(&format!("connect-timeout = {CONNECT_TIMEOUT_SECS}")));
        // Progress-based, not duration-based: a long generation that keeps
        // streaming must survive, while a silent stream must not.
        assert!(config.contains("speed-limit = 1"));
        assert!(config.contains(&format!("speed-time = {STALL_TIMEOUT_SECS}")));
        assert!(config.contains(&format!("max-time = {MAX_TIME_SECS}")));
        assert!(config.contains("dump-header = \"h.headers\""));
    }

    #[test]
    fn curl_transport_does_not_put_api_key_in_argv() {
        let api_key = "sk_test_12345678901234567890";
        let transport = CurlModelTransport::new(
            "https://api.example.test/",
            api_key,
            test_registry(),
            test_data_dir(),
        );
        let args = CurlModelTransport::curl_args();

        assert!(!args.iter().any(|arg| arg.contains(api_key)));
        assert_eq!(args, ["-sS", "--no-buffer", "--config", "-"]);

        let config = transport.curl_config(
            "{\"model\":\"test\",\"messages\":[]}",
            Path::new("h.headers"),
        );
        assert!(config.contains(&format!("authorization: Bearer {api_key}")));
        assert!(
            config.contains("data-binary = \"{\\\"model\\\":\\\"test\\\",\\\"messages\\\":[]}\"")
        );
    }

    /// A scratch data directory, so constructing a transport never writes into
    /// the user's real `~/Library/Application Support/DamaianClient`.
    fn test_data_dir() -> PathBuf {
        std::env::temp_dir().join(format!(
            "damaian-model-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ))
    }

    #[test]
    fn a_transport_with_no_metadata_reports_none_by_default() {
        // The default must be "unknown", never "200": a fabricated success is
        // how a mock would silently assert the provider answered.
        let transport = MockModelTransport::new("data: [DONE]\n");
        assert_eq!(transport.last_response_meta().status, None);
        assert_eq!(transport.last_response_meta().retry_after_secs, None);
    }

    #[test]
    fn a_mock_can_report_a_status_and_a_retry_after() {
        let mut transport = MockModelTransport::new("data: [DONE]\n");
        transport.status = Some(429);
        transport.retry_after_secs = Some(3);
        // `send` must leave the metadata readable afterwards.
        transport.send("{}").expect("send");
        assert_eq!(transport.last_response_meta().status, Some(429));
        assert_eq!(transport.last_response_meta().retry_after_secs, Some(3));
    }

    #[test]
    fn the_status_and_retry_after_are_read_from_a_header_file() {
        let header = "HTTP/2 429\nretry-after: 3\ncontent-type: application/json\n";
        assert_eq!(parse_status_line(header), Some(429));
        assert_eq!(parse_retry_after_header(header), Some(3));
    }

    #[test]
    fn the_retry_after_header_is_case_insensitive() {
        let header = "HTTP/1.1 503 Service Unavailable\nRetry-After: 7\n";
        assert_eq!(parse_retry_after_header(header), Some(7));
    }

    #[test]
    fn retry_after_delta_seconds_parse_directly() {
        assert_eq!(parse_retry_after("3"), Some(3));
        assert_eq!(parse_retry_after(" 17 "), Some(17));
    }

    #[test]
    fn an_http_date_parses_to_epoch_seconds() {
        // The RFC 7231 example; its epoch value is a fixed, well-known number.
        assert_eq!(
            parse_http_date("Sun, 06 Nov 1994 08:49:37 GMT"),
            Some(784_111_777)
        );
    }

    #[test]
    fn retry_after_accepts_a_future_http_date_and_rejects_a_past_one() {
        // 2100 is far in the future; the day name before the comma is ignored.
        let seconds = parse_retry_after("Mon, 01 Jan 2100 00:00:00 GMT").unwrap();
        assert!(seconds > 0);
        assert_eq!(parse_retry_after("Sun, 06 Nov 1994 08:49:37 GMT"), None);
    }

    #[test]
    fn a_429_status_classifies_as_rate_limited_regardless_of_prose() {
        // The message says "connection", the request id carries "429": status
        // must win over both.
        let meta = ResponseMeta {
            status: Some(429),
            retry_after_secs: Some(3),
        };
        let raw = "{\"error\":{\"message\":\"connection failed\",\"request_id\":\"req_429ab\"}}";
        assert_eq!(
            classify_refusal(&meta, raw),
            Some(ProviderRefusal::RateLimited {
                retry_after_secs: Some(3)
            })
        );
    }

    #[test]
    fn a_permanent_status_is_not_retried_even_when_prose_names_a_rate_limit() {
        let meta = ResponseMeta {
            status: Some(401),
            retry_after_secs: None,
        };
        let raw = "{\"error\":{\"message\":\"connection failed\",\"request_id\":\"req_429ab\"}}";
        let refusal = classify_refusal(&meta, raw).unwrap();
        assert_eq!(refusal, ProviderRefusal::AuthFailed);
        assert!(!refusal.is_transient());
    }

    #[test]
    fn a_503_status_classifies_as_overloaded_and_is_transient() {
        let meta = ResponseMeta {
            status: Some(503),
            retry_after_secs: Some(5),
        };
        assert_eq!(
            classify_refusal(&meta, "{}").unwrap(),
            ProviderRefusal::Overloaded {
                retry_after_secs: Some(5)
            }
        );
    }

    #[test]
    fn a_402_status_is_quota_exhausted_even_without_a_body() {
        let meta = ResponseMeta {
            status: Some(402),
            retry_after_secs: None,
        };
        assert_eq!(
            classify_refusal(&meta, "{}"),
            Some(ProviderRefusal::QuotaExhausted)
        );
    }

    #[test]
    fn a_2xx_with_no_error_object_is_not_a_refusal() {
        let meta = ResponseMeta {
            status: Some(200),
            retry_after_secs: None,
        };
        assert_eq!(classify_refusal(&meta, "data: [DONE]\n"), None);
    }

    #[test]
    fn a_2xx_with_an_error_object_is_classified_from_the_code_field() {
        // Some providers return 200 with an error body; the `code` field is
        // the signal, never the `message`.
        let meta = ResponseMeta {
            status: Some(200),
            retry_after_secs: None,
        };
        let raw = "{\"error\":{\"code\":\"rate_limit_exceeded\",\"message\":\"slow down\"}}";
        assert_eq!(
            classify_refusal(&meta, raw),
            Some(ProviderRefusal::RateLimited {
                retry_after_secs: None
            })
        );
    }

    #[test]
    fn a_body_naming_quota_is_exhausted_only_when_no_status_classified() {
        let meta = ResponseMeta::default();
        let raw = "{\"error\":{\"code\":\"insufficient_quota\",\"message\":\"out of credit\"}}";
        assert_eq!(
            classify_refusal(&meta, raw),
            Some(ProviderRefusal::QuotaExhausted)
        );
    }

    #[test]
    fn an_unclassified_4xx_naming_quota_is_quota_exhausted() {
        let meta = ResponseMeta {
            status: Some(428),
            retry_after_secs: None,
        };
        let raw = "{\"error\":{\"code\":\"billing_not_active\"}}";
        assert_eq!(
            classify_refusal(&meta, raw),
            Some(ProviderRefusal::QuotaExhausted)
        );
    }

    #[test]
    fn an_unclassified_4xx_without_a_quota_body_is_unknown_and_permanent() {
        let meta = ResponseMeta {
            status: Some(428),
            retry_after_secs: None,
        };
        let refusal = classify_refusal(&meta, "{\"error\":{\"message\":\"weird\"}}").unwrap();
        assert_eq!(refusal, ProviderRefusal::Unknown);
        assert!(!refusal.is_transient());
    }

    #[test]
    fn a_429_then_200_is_retried_and_succeeds() {
        // `retry_after: 0` keeps the test from sleeping through the backoff.
        let transport = MockModelTransport::sequence_with_status(vec![
            (
                "{\"error\":{\"message\":\"rate limited\"}}".to_string(),
                Some(429),
                Some(0),
            ),
            (
                "data: {\"choices\":[{\"delta\":{\"content\":\"ok\"}}]}\n\ndata: [DONE]\n"
                    .to_string(),
                Some(200),
                None,
            ),
        ]);
        let mut adapter = OpenAICompatibleAdapter::new("test-model", transport);
        let run = adapter
            .stream_response(
                &test_request(),
                &CancelToken::new(),
                &mut |_| {},
                &mut |_| {},
            )
            .expect("the retry should succeed");
        assert_eq!(run.content, "ok");
        assert_eq!(run.refusal_retries, 1, "one refusal retry beyond the first");
        assert_eq!(
            adapter.transport.requests.len(),
            2,
            "the provider was asked twice"
        );
    }

    #[test]
    fn a_retry_after_beyond_the_ceiling_fails_with_the_providers_figure() {
        let transport = MockModelTransport::sequence_with_status(vec![(
            "{\"error\":{\"message\":\"rate limited\"}}".to_string(),
            Some(429),
            Some(10_000),
        )]);
        let mut adapter = OpenAICompatibleAdapter::new("test-model", transport);
        let error = adapter
            .stream_response(
                &test_request(),
                &CancelToken::new(),
                &mut |_| {},
                &mut |_| {},
            )
            .expect_err("a wait beyond the ceiling is not slept through");
        assert_eq!(error.code(), "provider_rate_limited");
        assert!(
            format!("{error}").contains("10000"),
            "the message carries the provider's own figure"
        );
        assert_eq!(
            adapter.transport.requests.len(),
            1,
            "no retry was attempted"
        );
    }

    #[test]
    fn a_permanent_refusal_is_not_retried() {
        let transport = MockModelTransport::sequence_with_status(vec![
            (
                "{\"error\":{\"message\":\"bad key\"}}".to_string(),
                Some(401),
                None,
            ),
            ("data: [DONE]\n".to_string(), Some(200), None),
        ]);
        let mut adapter = OpenAICompatibleAdapter::new("test-model", transport);
        let error = adapter
            .stream_response(
                &test_request(),
                &CancelToken::new(),
                &mut |_| {},
                &mut |_| {},
            )
            .expect_err("a 401 must not be retried");
        assert_eq!(error.code(), "provider_auth_failed");
        assert_eq!(
            adapter.transport.requests.len(),
            1,
            "the second response was never asked for"
        );
    }

    #[test]
    fn a_rate_limit_wait_is_interrupted_by_a_stop() {
        let transport = MockModelTransport::sequence_with_status(vec![
            (
                "{\"error\":{\"message\":\"rate limited\"}}".to_string(),
                Some(429),
                Some(30),
            ),
            ("data: [DONE]\n".to_string(), Some(200), None),
        ]);
        let mut adapter = OpenAICompatibleAdapter::new("test-model", transport);
        let cancel = CancelToken::new();
        let stopper = cancel.clone();
        let started = std::time::Instant::now();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(100));
            stopper.cancel();
        });

        let result = adapter.stream_response(&test_request(), &cancel, &mut |_| {}, &mut |_| {});
        assert_eq!(result.unwrap_err(), ClientError::Cancelled);
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "the stop must interrupt the wait, not sleep it out: {:?}",
            started.elapsed()
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
            request_usage: false,
        }
    }

    #[test]
    fn a_run_with_no_reported_usage_is_estimated_from_the_request_and_the_content() {
        let transport =
            MockModelTransport::new("{\"choices\":[{\"delta\":{\"content\":\"hello\"}}]}");
        let mut adapter = OpenAICompatibleAdapter::new("test-model", transport);
        let request = test_request();
        let run = adapter
            .stream_response(&request, &CancelToken::new(), &mut |_token| {}, &mut |_| {})
            .expect("the mock stream should produce a run");

        assert_eq!(run.usage.source, UsageSource::Estimated);
        // Over the serialised request rather than the prompt alone, so it is
        // larger than the user's text and never zero.
        let body_estimate = model_request_json(&request).len().div_ceil(4) as u64;
        assert_eq!(run.usage.input_tokens, body_estimate);
        assert_eq!(run.usage.output_tokens, "hello".len().div_ceil(4) as u64);
        assert_eq!(run.reported_cost, None);
    }

    fn usage_request() -> ModelRequest {
        ModelRequest {
            stream: true,
            request_usage: true,
            ..test_request()
        }
    }

    #[test]
    fn a_streaming_request_asks_for_usage_when_the_provider_supports_it() {
        let body = model_request_json(&usage_request());
        assert!(body.contains("\"stream_options\":{\"include_usage\":true}"));
    }

    #[test]
    fn a_non_streaming_request_never_asks_for_usage() {
        // `stream_options` is meaningless without a stream and is rejected
        // outright by some providers, so the flag alone must not emit it.
        let request = ModelRequest {
            stream: false,
            request_usage: true,
            ..test_request()
        };
        assert!(!model_request_json(&request).contains("stream_options"));
    }

    #[test]
    fn a_request_that_does_not_ask_for_usage_omits_the_option() {
        assert!(!model_request_json(&test_request()).contains("stream_options"));
    }

    #[test]
    fn a_provider_that_rejects_stream_options_is_retried_once_without_it() {
        let transport = MockModelTransport::sequence(vec![
            "{\"error\":{\"message\":\"Unrecognized request argument supplied: stream_options\"}}"
                .to_string(),
            "data: {\"choices\":[{\"delta\":{\"content\":\"hello\"}}]}\n\ndata: [DONE]\n"
                .to_string(),
        ]);
        let mut adapter = OpenAICompatibleAdapter::new("test-model", transport);
        let run = adapter
            .stream_response(
                &usage_request(),
                &CancelToken::new(),
                &mut |_token| {},
                &mut |_| {},
            )
            .expect("the second attempt should succeed");

        assert_eq!(run.content, "hello");
        // A probe is not a failed call. Counting it would inflate the retry
        // figure and, once usage is recorded per attempt, bill the user for a
        // request that never ran.
        assert_eq!(run.retry_count, 0);
        assert_eq!(run.usage.source, UsageSource::Estimated);
        assert!(run.usage_reporting_unsupported);
        assert!(!adapter.probe_supports_usage());
    }

    #[test]
    fn the_probe_happens_once_rather_than_on_every_turn() {
        let transport = MockModelTransport::sequence(vec![
            "{\"error\":{\"message\":\"Unrecognized request argument supplied: stream_options\"}}"
                .to_string(),
            "data: {\"choices\":[{\"delta\":{\"content\":\"a\"}}]}\n\ndata: [DONE]\n".to_string(),
            "data: {\"choices\":[{\"delta\":{\"content\":\"b\"}}]}\n\ndata: [DONE]\n".to_string(),
        ]);
        let mut adapter = OpenAICompatibleAdapter::new("test-model", transport);
        adapter
            .stream_response(
                &usage_request(),
                &CancelToken::new(),
                &mut |_token| {},
                &mut |_| {},
            )
            .expect("the first call succeeds after the probe");
        let second = adapter
            .stream_response(
                &usage_request(),
                &CancelToken::new(),
                &mut |_token| {},
                &mut |_| {},
            )
            .expect("the second call succeeds directly");

        // A fourth body would mean the second turn probed again.
        let bodies = &adapter.transport.requests;
        assert_eq!(bodies.len(), 3);
        assert!(bodies[0].contains("stream_options"));
        assert!(!bodies[1].contains("stream_options"));
        assert!(!bodies[2].contains("stream_options"));
        // Audited once, on the run that observed it — not on every later call.
        assert!(!second.usage_reporting_unsupported);
    }

    #[test]
    fn a_provider_error_that_is_not_about_usage_is_still_an_error() {
        // The probe must not swallow real failures by retrying everything.
        let transport = MockModelTransport::sequence(vec![
            "{\"error\":{\"message\":\"Insufficient balance\"}}".to_string(),
            "data: {\"choices\":[{\"delta\":{\"content\":\"hello\"}}]}\n\ndata: [DONE]\n"
                .to_string(),
        ]);
        let mut adapter = OpenAICompatibleAdapter::new("test-model", transport);
        let error = adapter
            .stream_response(
                &usage_request(),
                &CancelToken::new(),
                &mut |_token| {},
                &mut |_| {},
            )
            .expect_err("a balance error must not be retried as a capability probe");

        assert!(format!("{error}").contains("Insufficient balance"));
        assert_eq!(adapter.transport.requests.len(), 1);
    }

    #[test]
    fn usage_is_read_from_the_final_chunk_of_a_stream() {
        let raw = concat!(
            "data: {\"choices\":[{\"delta\":{\"content\":\"hi\"}}]}\n\n",
            "data: {\"choices\":[],\"usage\":{\"prompt_tokens\":11902,\"completion_tokens\":812}}\n\n",
            "data: [DONE]\n"
        );
        assert_eq!(extract_usage(raw), Some((11902, 812, None)));
    }

    #[test]
    fn usage_accepts_the_input_output_naming_some_providers_use() {
        let raw = "data: {\"choices\":[],\"usage\":{\"input_tokens\":7,\"output_tokens\":3}}\n";
        assert_eq!(extract_usage(raw), Some((7, 3, None)));
    }

    #[test]
    fn a_reported_cost_is_carried_when_the_provider_sends_one() {
        let raw = "data: {\"choices\":[],\"usage\":{\"prompt_tokens\":5,\"completion_tokens\":2,\"cost\":0.00031}}\n";
        assert_eq!(extract_usage(raw), Some((5, 2, Some(0.00031))));
    }

    #[test]
    fn a_stream_without_a_usage_object_reports_nothing_rather_than_zero() {
        // The distinction requirement 4 rests on: absent is not the same as
        // zero, and a zero here would be presented as measured.
        let raw = "data: {\"choices\":[{\"delta\":{\"content\":\"hi\"}}]}\n\ndata: [DONE]\n";
        assert_eq!(extract_usage(raw), None);
    }

    #[test]
    fn a_half_reported_usage_object_is_not_treated_as_measured() {
        // A missing half would otherwise become a fabricated zero wearing a
        // measured label.
        let raw = "data: {\"choices\":[],\"usage\":{\"prompt_tokens\":5}}\n";
        assert_eq!(extract_usage(raw), None);
    }

    #[test]
    fn a_measured_run_carries_the_providers_figures_not_the_estimate() {
        let transport = MockModelTransport::new(concat!(
            "data: {\"choices\":[{\"delta\":{\"content\":\"hello\"}}]}\n\n",
            "data: {\"choices\":[],\"usage\":{\"prompt_tokens\":41,\"completion_tokens\":9}}\n\n",
            "data: [DONE]\n"
        ));
        let mut adapter = OpenAICompatibleAdapter::new("test-model", transport);
        let run = adapter
            .stream_response(
                &test_request(),
                &CancelToken::new(),
                &mut |_token| {},
                &mut |_| {},
            )
            .expect("the mock stream should produce a run");

        assert_eq!(run.usage.source, UsageSource::Measured);
        assert_eq!(run.usage.input_tokens, 41);
        assert_eq!(run.usage.output_tokens, 9);
    }

    #[test]
    fn a_turn_cancelled_before_the_provider_was_called_is_a_measured_zero() {
        // The one genuinely free case (spec 19 §5.5): nothing was sent, so
        // nothing was billed, and that is a fact rather than an estimate.
        let run = ModelRun::cancelled_before_start("openai-compatible", "test-model");

        assert_eq!(run.usage.source, UsageSource::Measured);
        assert_eq!(run.usage.input_tokens, 0);
        assert_eq!(run.usage.output_tokens, 0);
        assert_eq!(run.reported_cost, None);
    }

    #[test]
    fn retries_transient_failure_before_any_token_then_succeeds() {
        let transport =
            MockModelTransport::failing("{\"choices\":[{\"delta\":{\"content\":\"hi\"}}]}", 2);
        let mut adapter = OpenAICompatibleAdapter::new("test-model", transport);
        let mut tokens = Vec::new();
        let run = adapter
            .stream_response(
                &test_request(),
                &CancelToken::new(),
                &mut |token| tokens.push(token.to_string()),
                &mut |_| {},
            )
            .expect("should succeed after retries");

        assert_eq!(run.retry_count, 2);
        assert_eq!(run.content, "hi");
        assert_eq!(tokens.join(""), "hi");
    }

    #[test]
    fn gives_up_after_max_attempts_on_persistent_transient_failure() {
        let transport = MockModelTransport::failing("unused", 10);
        let mut adapter = OpenAICompatibleAdapter::new("test-model", transport);
        let result = adapter.stream_response(
            &test_request(),
            &CancelToken::new(),
            &mut |_token| {},
            &mut |_| {},
        );

        assert!(result.is_err());
        let error = result.unwrap_err();
        assert!(error.is_retryable());
    }

    #[test]
    fn does_not_retry_non_retryable_failure() {
        let mut transport = MockModelTransport::failing("unused", 1);
        transport.failure_message = "invalid api key".to_string();
        let mut adapter = OpenAICompatibleAdapter::new("test-model", transport);
        let result = adapter.stream_response(
            &test_request(),
            &CancelToken::new(),
            &mut |_token| {},
            &mut |_| {},
        );

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
        let result = adapter.stream_response(
            &test_request(),
            &CancelToken::new(),
            &mut |token| tokens.push(token.to_string()),
            &mut |_| {},
        );

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
            .stream_response(
                &test_request(),
                &CancelToken::new(),
                &mut |_token| {},
                &mut |_| {},
            )
            .expect("tool-call-only response should not be treated as empty");

        assert!(run.content.is_empty());
        assert_eq!(run.tool_calls.len(), 1);
        assert_eq!(run.tool_calls[0].name, "run_command");
    }
}
