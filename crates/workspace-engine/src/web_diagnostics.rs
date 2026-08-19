use crate::error::{ClientError, Result};
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
}

impl WebDiagnosticReport {
    pub fn from_text(text: impl Into<String>, is_error: bool) -> Self {
        let text = text.into();
        let artifacts = extract_artifacts_from_text(&text);
        Self {
            text,
            artifacts,
            is_error,
        }
    }
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

fn extract_artifacts_from_text(text: &str) -> Vec<WebDiagnosticArtifact> {
    let Ok(value) = serde_json::from_str::<Value>(text.trim()) else {
        return Vec::new();
    };
    let Some(artifacts) = value.get("artifacts").and_then(Value::as_array) else {
        return Vec::new();
    };
    artifacts
        .iter()
        .filter_map(|artifact| {
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
        })
        .collect()
}
