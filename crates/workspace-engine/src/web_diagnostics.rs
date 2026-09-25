use crate::error::{ClientError, Result};
use crate::secret_scanner::SecretScanner;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::Arc;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WebDiagnosticKind {
    Inspect,
    Scenario,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WebDiagnosticCall {
    pub kind: WebDiagnosticKind,
    pub url: String,
    pub arguments_json: String,
    #[serde(default)]
    pub session_id: Option<String>,
    #[serde(default)]
    pub task_id: Option<String>,
}

impl WebDiagnosticCall {
    pub fn from_tool_call(name: &str, arguments_json: &str) -> Result<Option<Self>> {
        let kind = match name {
            "inspect_web_page" => WebDiagnosticKind::Inspect,
            "run_web_scenario" => WebDiagnosticKind::Scenario,
            _ => return Ok(None),
        };
        let arguments: Value = serde_json::from_str(arguments_json).map_err(|error| {
            ClientError::InvalidInput(format!("Invalid {name} arguments JSON: {error}"))
        })?;
        let url = arguments
            .get("url")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| ClientError::InvalidInput(format!("{name} requires a non-empty url")))?
            .to_string();

        if kind == WebDiagnosticKind::Scenario {
            validate_scenario_actions(&arguments)?;
        }

        Ok(Some(Self {
            kind,
            url,
            arguments_json: arguments.to_string(),
            session_id: None,
            task_id: None,
        }))
    }

    pub fn with_context(mut self, session_id: &str, task_id: &str) -> Self {
        self.session_id = Some(session_id.to_string());
        self.task_id = Some(task_id.to_string());
        self
    }

    pub fn name(&self) -> &'static str {
        match self.kind {
            WebDiagnosticKind::Inspect => "inspect_web_page",
            WebDiagnosticKind::Scenario => "run_web_scenario",
        }
    }

    pub fn is_low_risk(&self) -> bool {
        self.kind == WebDiagnosticKind::Inspect && is_loopback_url(&self.url)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WebDiagnosticArtifact {
    pub kind: String,
    pub path: String,
    pub mime_type: Option<String>,
    pub width: Option<u32>,
    pub height: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WebDiagnosticReport {
    pub text: String,
    pub artifacts: Vec<WebDiagnosticArtifact>,
    pub is_error: bool,
    /// Present when `text` was a companion-shaped JSON object
    /// (spec 12 `context.md` §1.1). `None` for any other runner, whose text
    /// keeps the raw path.
    #[serde(default)]
    pub details: Option<WebDiagnosticDetails>,
    /// Which runner produced this, e.g. "MCP server `x` tool `inspect_page`".
    #[serde(default)]
    pub via: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct WebDiagnosticDetails {
    pub url: Option<String>,
    pub final_url: Option<String>,
    pub title: Option<String>,
    pub status: Option<u16>,
    pub page_errors: Vec<String>,
    pub console: Vec<WebConsoleEntry>,
    pub failed_requests: Vec<WebFailedRequest>,
    pub dom_summary: Option<WebDomSummary>,
    /// Scenario step results; empty for an inspection.
    pub steps: Vec<WebScenarioStep>,
    /// The runner's own failure message, when the call itself failed
    /// (companion `"error": true`) — not a problem found on the page.
    pub tool_error: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct WebConsoleEntry {
    /// As the browser reported it: `error`, `warning`, `log`, `assert`, ...
    pub level: String,
    pub text: String,
    pub location: Option<WebSourceLocation>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct WebSourceLocation {
    pub url: String,
    /// 1-based. Playwright reports 0-based; converted at parse.
    pub line: Option<u32>,
    /// 1-based, like `line`.
    pub column: Option<u32>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct WebFailedRequest {
    pub url: String,
    pub method: Option<String>,
    pub resource_type: Option<String>,
    /// HTTP status for a response ≥ 400; `None` for a request that never
    /// completed.
    pub status: Option<u16>,
    /// `status_text` for a response, the network error for a failed request.
    pub failure: Option<String>,
}

// `fields` from the companion's `dom_summary` is left out on purpose: up to 40
// records of form metadata that neither the model header nor the card uses.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct WebDomSummary {
    pub forms: Option<u32>,
    pub buttons: Vec<String>,
    pub status_text: Option<String>,
    pub visible_text_excerpt: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct WebScenarioStep {
    pub step: u32,
    pub action: String,
    pub success: bool,
    pub error: Option<String>,
}

impl WebDiagnosticReport {
    pub fn from_text(text: impl Into<String>, is_error: bool) -> Self {
        let text = text.into();
        let value = serde_json::from_str::<Value>(text.trim()).ok();
        let artifacts = value.as_ref().map(artifacts_from_value).unwrap_or_default();
        let details = value.as_ref().and_then(details_from_value);
        Self {
            text,
            artifacts,
            is_error,
            details,
            via: None,
        }
    }

    /// True when the runner itself failed. A page that throws is a
    /// successful diagnostic with problems in it, not a failed call
    /// (spec 12 `context.md` §3.2).
    pub fn tool_failed(&self) -> bool {
        self.is_error
            || self
                .details
                .as_ref()
                .is_some_and(|details| details.tool_error.is_some())
    }

    /// Every captured string run through the scanner. `SessionStore` is
    /// deliberately unredacted, so anything persisted or streamed must be
    /// built from this.
    pub fn redacted(&self, scanner: &SecretScanner) -> Self {
        Self {
            text: redact(scanner, &self.text),
            artifacts: self
                .artifacts
                .iter()
                .map(|artifact| artifact.redacted(scanner))
                .collect(),
            is_error: self.is_error,
            details: self
                .details
                .as_ref()
                .map(|details| details.redacted(scanner)),
            via: redact_option(scanner, &self.via),
        }
    }
}

impl WebDiagnosticArtifact {
    fn redacted(&self, scanner: &SecretScanner) -> Self {
        Self {
            kind: redact(scanner, &self.kind),
            path: redact(scanner, &self.path),
            mime_type: redact_option(scanner, &self.mime_type),
            width: self.width,
            height: self.height,
        }
    }
}

impl WebDiagnosticDetails {
    /// Page errors, console errors and warnings, failed requests, and failed
    /// scenario steps: what the model header counts.
    pub fn problem_count(&self) -> usize {
        self.page_errors.len()
            + self
                .console
                .iter()
                .filter(|entry| entry.is_problem())
                .count()
            + self.failed_requests.len()
            + self.steps.iter().filter(|step| !step.success).count()
    }

    fn redacted(&self, scanner: &SecretScanner) -> Self {
        Self {
            url: redact_option(scanner, &self.url),
            final_url: redact_option(scanner, &self.final_url),
            title: redact_option(scanner, &self.title),
            status: self.status,
            page_errors: self
                .page_errors
                .iter()
                .map(|error| redact(scanner, error))
                .collect(),
            console: self
                .console
                .iter()
                .map(|entry| entry.redacted(scanner))
                .collect(),
            failed_requests: self
                .failed_requests
                .iter()
                .map(|request| request.redacted(scanner))
                .collect(),
            dom_summary: self
                .dom_summary
                .as_ref()
                .map(|summary| summary.redacted(scanner)),
            steps: self
                .steps
                .iter()
                .map(|step| step.redacted(scanner))
                .collect(),
            tool_error: redact_option(scanner, &self.tool_error),
        }
    }
}

impl WebConsoleEntry {
    pub fn is_problem(&self) -> bool {
        matches!(
            self.level.to_ascii_lowercase().as_str(),
            "error" | "warning" | "warn" | "assert"
        )
    }

    fn redacted(&self, scanner: &SecretScanner) -> Self {
        Self {
            level: redact(scanner, &self.level),
            text: redact(scanner, &self.text),
            location: self
                .location
                .as_ref()
                .map(|location| location.redacted(scanner)),
        }
    }
}

impl WebSourceLocation {
    fn redacted(&self, scanner: &SecretScanner) -> Self {
        Self {
            url: redact(scanner, &self.url),
            line: self.line,
            column: self.column,
        }
    }
}

impl WebFailedRequest {
    fn redacted(&self, scanner: &SecretScanner) -> Self {
        Self {
            url: redact(scanner, &self.url),
            method: redact_option(scanner, &self.method),
            resource_type: redact_option(scanner, &self.resource_type),
            status: self.status,
            failure: redact_option(scanner, &self.failure),
        }
    }
}

impl WebDomSummary {
    fn redacted(&self, scanner: &SecretScanner) -> Self {
        Self {
            forms: self.forms,
            buttons: self
                .buttons
                .iter()
                .map(|button| redact(scanner, button))
                .collect(),
            status_text: redact_option(scanner, &self.status_text),
            visible_text_excerpt: redact_option(scanner, &self.visible_text_excerpt),
        }
    }
}

impl WebScenarioStep {
    fn redacted(&self, scanner: &SecretScanner) -> Self {
        Self {
            step: self.step,
            action: redact(scanner, &self.action),
            success: self.success,
            error: redact_option(scanner, &self.error),
        }
    }
}

fn redact(scanner: &SecretScanner, text: &str) -> String {
    scanner.redact(text).text
}

fn redact_option(scanner: &SecretScanner, text: &Option<String>) -> Option<String> {
    text.as_deref().map(|text| redact(scanner, text))
}

pub trait WebDiagnosticsRunner: Send + Sync {
    fn inspect(&self, call: &WebDiagnosticCall) -> Result<WebDiagnosticReport>;
    fn run_scenario(&self, call: &WebDiagnosticCall) -> Result<WebDiagnosticReport>;
}

#[derive(Clone)]
pub struct WebDiagnosticsRunnerHandle(Arc<dyn WebDiagnosticsRunner>);

impl WebDiagnosticsRunnerHandle {
    pub fn new(runner: impl WebDiagnosticsRunner + 'static) -> Self {
        Self(Arc::new(runner))
    }

    pub fn inspect(&self, call: &WebDiagnosticCall) -> Result<WebDiagnosticReport> {
        self.0.inspect(call)
    }

    pub fn run_scenario(&self, call: &WebDiagnosticCall) -> Result<WebDiagnosticReport> {
        self.0.run_scenario(call)
    }
}

impl std::fmt::Debug for WebDiagnosticsRunnerHandle {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("WebDiagnosticsRunnerHandle(..)")
    }
}

pub const WEB_SCENARIO_ACTIONS: [&str; 11] = [
    "goto",
    "fill",
    "click",
    "press",
    "select",
    "submit",
    "wait",
    "wait_for_selector",
    "expect_text",
    "expect_selector",
    "screenshot",
];

fn validate_scenario_actions(arguments: &Value) -> Result<()> {
    let Some(actions) = arguments.get("actions") else {
        return Ok(());
    };
    let Some(actions) = actions.as_array() else {
        return Err(ClientError::InvalidInput(
            "run_web_scenario actions must be an array".to_string(),
        ));
    };
    for action in actions {
        let Some(action_name) = action.get("action").and_then(Value::as_str) else {
            return Err(ClientError::InvalidInput(
                "run_web_scenario actions must include an action string".to_string(),
            ));
        };
        if !WEB_SCENARIO_ACTIONS.contains(&action_name) {
            return Err(ClientError::InvalidInput(format!(
                "Unsupported web scenario action `{action_name}`. Use one of: {}",
                WEB_SCENARIO_ACTIONS.join(", ")
            )));
        }
    }
    Ok(())
}

fn is_loopback_url(url: &str) -> bool {
    let lower = url.trim().to_ascii_lowercase();
    if !lower.starts_with("http://") && !lower.starts_with("https://") {
        return false;
    }
    let Some(after_scheme) = lower.split_once("://").map(|(_, rest)| rest) else {
        return false;
    };
    let host_port = after_scheme
        .split(['/', '?', '#'])
        .next()
        .unwrap_or_default();
    let host = if let Some(rest) = host_port.strip_prefix('[') {
        rest.split(']').next().unwrap_or_default()
    } else {
        host_port.split(':').next().unwrap_or_default()
    };
    matches!(host, "localhost" | "127.0.0.1" | "::1")
}

/// `artifact_metadata` carries kind and dimensions; `artifacts` is bare
/// paths. Metadata wins, and any path only in `artifacts` is kept too.
fn artifacts_from_value(value: &Value) -> Vec<WebDiagnosticArtifact> {
    let mut artifacts: Vec<WebDiagnosticArtifact> = value
        .get("artifact_metadata")
        .and_then(Value::as_array)
        .map(|records| records.iter().filter_map(artifact_record).collect())
        .unwrap_or_default();
    for artifact in value
        .get("artifacts")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(artifact_record)
    {
        if !artifacts.iter().any(|known| known.path == artifact.path) {
            artifacts.push(artifact);
        }
    }
    artifacts
}

fn artifact_record(artifact: &Value) -> Option<WebDiagnosticArtifact> {
    if let Some(path) = artifact.as_str() {
        return Some(WebDiagnosticArtifact {
            kind: "artifact".to_string(),
            path: path.to_string(),
            mime_type: None,
            width: None,
            height: None,
        });
    }
    let path = artifact.get("path")?.as_str()?.to_string();
    Some(WebDiagnosticArtifact {
        kind: artifact
            .get("kind")
            .and_then(Value::as_str)
            .unwrap_or("artifact")
            .to_string(),
        path,
        mime_type: artifact
            .get("mime_type")
            .or_else(|| artifact.get("mimeType"))
            .and_then(Value::as_str)
            .map(str::to_string),
        width: artifact
            .get("width")
            .and_then(Value::as_u64)
            .map(|value| value as u32),
        height: artifact
            .get("height")
            .and_then(Value::as_u64)
            .map(|value| value as u32),
    })
}

/// The keys that make a JSON object companion-shaped. Any one is enough; an
/// object with none of them (`{"unrelated": true}`) is some other server's
/// output and keeps the raw path.
const COMPANION_KEYS: [&str; 6] = [
    "final_url",
    "page_errors",
    "console",
    "failed_requests",
    "dom_summary",
    "results",
];

fn details_from_value(value: &Value) -> Option<WebDiagnosticDetails> {
    let object = value.as_object()?;
    if !COMPANION_KEYS.iter().any(|key| object.contains_key(*key)) {
        return None;
    }
    let list = |key: &str| {
        object
            .get(key)
            .and_then(Value::as_array)
            .map(Vec::as_slice)
            .unwrap_or_default()
    };
    Some(WebDiagnosticDetails {
        url: string_field(value, "url"),
        final_url: string_field(value, "final_url"),
        title: string_field(value, "title"),
        status: u16_field(value, "status"),
        page_errors: list("page_errors")
            .iter()
            .filter_map(|item| item.as_str().map(str::to_string))
            .collect(),
        console: list("console").iter().filter_map(console_entry).collect(),
        failed_requests: list("failed_requests")
            .iter()
            .filter_map(failed_request)
            .collect(),
        dom_summary: object.get("dom_summary").and_then(dom_summary),
        steps: list("results").iter().filter_map(scenario_step).collect(),
        tool_error: (object.get("error").and_then(Value::as_bool) == Some(true)).then(|| {
            string_field(value, "message")
                .unwrap_or_else(|| "the diagnostic tool reported an error".to_string())
        }),
    })
}

fn string_field(value: &Value, key: &str) -> Option<String> {
    value.get(key).and_then(Value::as_str).map(str::to_string)
}

fn u16_field(value: &Value, key: &str) -> Option<u16> {
    value
        .get(key)
        .and_then(Value::as_u64)
        .and_then(|number| u16::try_from(number).ok())
}

/// Playwright's `lineNumber`/`columnNumber` are 0-based; stored 1-based.
fn one_based(value: &Value, key: &str) -> Option<u32> {
    value
        .get(key)
        .and_then(Value::as_u64)
        .and_then(|number| u32::try_from(number + 1).ok())
}

fn console_entry(value: &Value) -> Option<WebConsoleEntry> {
    Some(WebConsoleEntry {
        level: string_field(value, "type").unwrap_or_default(),
        text: string_field(value, "text")?,
        location: value.get("location").and_then(|location| {
            Some(WebSourceLocation {
                url: string_field(location, "url")?,
                line: one_based(location, "lineNumber"),
                column: one_based(location, "columnNumber"),
            })
        }),
    })
}

fn failed_request(value: &Value) -> Option<WebFailedRequest> {
    Some(WebFailedRequest {
        url: string_field(value, "url")?,
        method: string_field(value, "method"),
        resource_type: string_field(value, "resource_type"),
        status: u16_field(value, "status"),
        failure: string_field(value, "failure").or_else(|| string_field(value, "status_text")),
    })
}

fn dom_summary(value: &Value) -> Option<WebDomSummary> {
    let summary = WebDomSummary {
        forms: value
            .get("forms")
            .and_then(Value::as_u64)
            .and_then(|forms| u32::try_from(forms).ok()),
        buttons: value
            .get("buttons")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|button| button.as_str().map(str::to_string))
            .collect(),
        status_text: string_field(value, "status_text"),
        visible_text_excerpt: string_field(value, "visible_text_excerpt"),
    };
    // `{"error": …}` has none of the known keys; no summary, so the card
    // shows no empty section.
    (summary != WebDomSummary::default()).then_some(summary)
}

fn scenario_step(value: &Value) -> Option<WebScenarioStep> {
    Some(WebScenarioStep {
        step: value
            .get("step")
            .and_then(Value::as_u64)
            .and_then(|step| u32::try_from(step).ok())
            .unwrap_or_default(),
        action: string_field(value, "action")?,
        success: value
            .get("success")
            .and_then(Value::as_bool)
            .unwrap_or_default(),
        error: string_field(value, "error"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::secret_scanner::SecretScanner;

    /// Trimmed from a real `inspect_page` result of the companion server
    /// (`context.md` §1.1). Keys and nesting are the companion's.
    const COMPANION_REPORT: &str = r#"{
      "success": false, "diagnostic_ok": false, "run_id": "20260925-1",
      "url": "http://localhost:5001/", "final_url": "http://localhost:5001/",
      "title": "Snake Game", "status": 200,
      "page_errors": ["ReferenceError: Cannot access 'game' before initialization"],
      "console": [
        {"type": "error", "text": "Failed to load resource: 404",
         "location": {"url": "http://localhost:5001/js/app.js", "lineNumber": 41, "columnNumber": 7}},
        {"type": "log", "text": "booting"}
      ],
      "failed_requests": [
        {"kind": "response", "url": "http://localhost:5001/api/me", "method": "GET",
         "resource_type": "fetch", "status": 404, "status_text": "Not Found"},
        {"kind": "requestfailed", "url": "http://localhost:5001/ws", "method": "GET",
         "resource_type": "websocket", "failure": "net::ERR_CONNECTION_REFUSED"}
      ],
      "dom_summary": {"forms": 1, "buttons": ["Log in", "Register"], "fields": [],
        "status_text": "", "visible_text_excerpt": "Snake Log in Register Score: 0"},
      "artifacts": ["/tmp/runs/20260925-1-page.png"],
      "artifact_metadata": [{"kind": "screenshot", "path": "/tmp/runs/20260925-1-page.png",
        "mime_type": "image/png", "width": 1280, "height": 720, "caption": "page"}],
      "text_report": "ignored by Damaian"
    }"#;

    #[test]
    fn a_companion_report_parses_into_typed_details() {
        let report = WebDiagnosticReport::from_text(COMPANION_REPORT, false);
        let details = report.details.expect("companion JSON has details");
        assert_eq!(details.final_url.as_deref(), Some("http://localhost:5001/"));
        assert_eq!(details.title.as_deref(), Some("Snake Game"));
        assert_eq!(details.status, Some(200));
        assert_eq!(
            details.page_errors,
            vec!["ReferenceError: Cannot access 'game' before initialization"]
        );
        assert_eq!(details.console.len(), 2);
        assert_eq!(details.console[0].level, "error");
        // Playwright's console location is 0-based; stored 1-based.
        assert_eq!(
            details.console[0].location,
            Some(WebSourceLocation {
                url: "http://localhost:5001/js/app.js".to_string(),
                line: Some(42),
                column: Some(8),
            })
        );
        assert_eq!(details.failed_requests[0].status, Some(404));
        assert_eq!(
            details.failed_requests[0].failure.as_deref(),
            Some("Not Found")
        );
        assert_eq!(
            details.failed_requests[1].failure.as_deref(),
            Some("net::ERR_CONNECTION_REFUSED")
        );
        let dom = details.dom_summary.clone().expect("dom summary");
        assert_eq!(dom.buttons, vec!["Log in", "Register"]);
        assert_eq!(details.tool_error, None);
        // 1 page error + 1 console error (the `log` line is not a problem)
        // + 2 failed requests.
        assert_eq!(details.problem_count(), 4);
    }

    #[test]
    fn artifact_metadata_supplies_kind_and_dimensions() {
        let report = WebDiagnosticReport::from_text(COMPANION_REPORT, false);
        assert_eq!(
            report.artifacts,
            vec![WebDiagnosticArtifact {
                kind: "screenshot".to_string(),
                path: "/tmp/runs/20260925-1-page.png".to_string(),
                mime_type: Some("image/png".to_string()),
                width: Some(1280),
                height: Some(720),
            }]
        );
    }

    #[test]
    fn text_that_is_not_companion_json_has_no_details() {
        for text in [
            "Browser diagnostic failed: 1 page error.\n- pageerror: boom",
            "{\"unrelated\": true}",
            "[1, 2, 3]",
            "",
        ] {
            let report = WebDiagnosticReport::from_text(text, false);
            assert_eq!(report.details, None, "{text:?}");
            assert_eq!(report.text, text);
        }
    }

    #[test]
    fn a_field_of_the_wrong_type_is_dropped_and_the_rest_kept() {
        let report = WebDiagnosticReport::from_text(
            r#"{"page_errors": "not a list", "title": 7, "final_url": "http://localhost:1/",
                "console": [{"type": "error"}, 5, {"type": "warning", "text": "slow"}]}"#,
            false,
        );
        let details = report.details.expect("final_url makes it companion-shaped");
        assert!(details.page_errors.is_empty());
        assert_eq!(details.title, None);
        assert_eq!(details.console.len(), 1, "entries without text are dropped");
        assert_eq!(details.console[0].text, "slow");
    }

    #[test]
    fn tool_failed_is_the_runner_failing_not_the_page() {
        let page_broken = WebDiagnosticReport::from_text(COMPANION_REPORT, false);
        assert!(!page_broken.tool_failed());

        let companion_error = WebDiagnosticReport::from_text(
            r#"{"error": true, "success": false, "message": "Timeout 30000ms exceeded",
                "final_url": "http://localhost:5001/"}"#,
            false,
        );
        assert!(companion_error.tool_failed());
        assert_eq!(
            companion_error.details.unwrap().tool_error.as_deref(),
            Some("Timeout 30000ms exceeded")
        );

        assert!(WebDiagnosticReport::from_text("anything", true).tool_failed());
    }

    #[test]
    fn scenario_steps_are_parsed() {
        let report = WebDiagnosticReport::from_text(
            // `r##` because the selector contains `"#`, which ends an `r#` string.
            r##"{"final_url": "http://localhost:5001/", "results": [
                {"step": 0, "action": "fill", "success": true, "selector": "#u"},
                {"step": 1, "action": "click", "success": false, "error": "not visible"}]}"##,
            false,
        );
        let steps = report.details.unwrap().steps;
        assert_eq!(steps.len(), 2);
        assert_eq!(
            steps[1],
            WebScenarioStep {
                step: 1,
                action: "click".to_string(),
                success: false,
                error: Some("not visible".to_string()),
            }
        );
    }

    #[test]
    fn redacted_scrubs_every_captured_string() {
        // The scanner's generic-token rule matches `ghp` + 20 token bytes, but
        // not when a token byte such as `/` precedes it — so inside URLs the
        // secret sits in a query value, where a leaked token actually appears.
        let secret = "ghp_abcdefghijklmnopqrstuvwxyz0123456789";
        let raw = format!(
            r#"{{"final_url": "http://localhost:5001/?t={secret}", "title": "{secret}",
                "url": "http://localhost:5001/?t={secret}",
                "page_errors": ["token {secret}"],
                "console": [{{"type": "error", "text": "{secret}",
                  "location": {{"url": "http://localhost:5001/app.js?t={secret}", "lineNumber": 0}}}}],
                "failed_requests": [{{"url": "http://localhost:5001/api?t={secret}",
                  "method": "{secret}", "resource_type": "{secret}", "failure": "{secret}"}}],
                "dom_summary": {{"buttons": ["{secret}"], "status_text": "{secret}",
                  "visible_text_excerpt": "{secret}"}},
                "results": [{{"step": 0, "action": "{secret}", "success": false, "error": "{secret}"}}],
                "artifacts": ["/tmp/runs/shot.png?t={secret}"],
                "error": true, "message": "{secret}"}}"#
        );
        let mut report = WebDiagnosticReport::from_text(raw, false);
        report.via = Some(format!("MCP server `{secret}`"));
        let redacted = report.redacted(&SecretScanner::new(Vec::new()));
        let serialized = serde_json::to_string(&redacted).unwrap();
        assert!(!serialized.contains(secret), "{serialized}");
        assert!(redacted.details.is_some(), "redaction keeps the structure");
    }
}
