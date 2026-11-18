//! Browser diagnostics through an MCP server, and the artifacts a
//! diagnostic run leaves for the UI.

use super::*;

pub(crate) fn handle_reveal_web_diagnostic_artifact(
    stream: &mut TcpStream,
    request: &Request,
) -> Result<(), String> {
    let form = parse_form(&request.body);
    let repo = required_form(&form, "repo")?;
    let relative_path = required_form(&form, "path")?;
    let config = config_for_repo(&repo)?;
    let path = reveal_web_diagnostic_artifact(&config, &relative_path)?;
    write_response(
        stream,
        request,
        200,
        "application/json",
        &format!("{{\"path\":\"{}\"}}", escape_json(&path.to_string_lossy())),
    )
}

pub(crate) fn handle_web_diagnostic_artifact(
    stream: &mut TcpStream,
    request: &Request,
) -> Result<(), String> {
    let repo = request.param("repo").unwrap_or_default();
    let relative_path = request
        .param("path")
        .ok_or_else(|| "path is required".to_string())?;
    let engine = engine_for_repo(&repo)?;
    let path = web_diagnostic_artifact_path(&engine.config, &relative_path)?;
    let bytes = fs::read(&path).map_err(|error| error.to_string())?;
    write_binary_response(stream, request, 200, content_type_for_path(&path), &bytes)
}

#[derive(Clone)]
struct McpBrowserDiagnosticsRunner {
    pub(crate) servers: Vec<BrowserDiagnosticsMcpServer>,
    pub(crate) data_dir: PathBuf,
}

#[derive(Clone)]
struct BrowserDiagnosticsMcpServer {
    pub(crate) config: McpServerConfig,
    pub(crate) auth_token: Option<String>,
}

impl std::fmt::Debug for McpBrowserDiagnosticsRunner {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("McpBrowserDiagnosticsRunner")
            .field(
                "servers",
                &self
                    .servers
                    .iter()
                    .map(|server| server.config.id.as_str())
                    .collect::<Vec<_>>(),
            )
            .finish()
    }
}

impl WebDiagnosticsRunner for McpBrowserDiagnosticsRunner {
    fn inspect(&self, call: &WebDiagnosticCall) -> workspace_engine::Result<WebDiagnosticReport> {
        self.call_compatible_tool(&["inspect_page", "inspect_web_page"], call)
    }

    fn run_scenario(
        &self,
        call: &WebDiagnosticCall,
    ) -> workspace_engine::Result<WebDiagnosticReport> {
        self.call_compatible_tool(&["run_web_scenario", "run_scenario"], call)
    }
}

impl McpBrowserDiagnosticsRunner {
    fn call_compatible_tool(
        &self,
        preferred_tools: &[&str],
        call: &WebDiagnosticCall,
    ) -> workspace_engine::Result<WebDiagnosticReport> {
        let mut last_error = None;
        // Diagnostics run outside any turn, so there is no session to attribute
        // these servers to — the entry's kind still identifies them.
        let registry = ProcessRegistry::open(&self.data_dir)?;
        for server in &self.servers {
            let mut client = match McpClient::connect(
                &server.config,
                server.auth_token.clone(),
                &registry,
                "",
                Duration::from_secs(15),
            ) {
                Ok(client) => client,
                Err(error) => {
                    last_error = Some(format!("{}: {error}", server.config.id));
                    continue;
                }
            };
            let tools = match client.list_tools(Duration::from_secs(15)) {
                Ok(tools) => tools,
                Err(error) => {
                    last_error = Some(format!("{}: {error}", server.config.id));
                    continue;
                }
            };
            let Some(tool_name) = preferred_tools
                .iter()
                .find(|candidate| tools.iter().any(|tool| tool.name == **candidate))
            else {
                continue;
            };
            let arguments = mcp_browser_arguments(tool_name, call)?;
            match client.call_tool(tool_name, &arguments, Duration::from_secs(60)) {
                Ok(result) => {
                    return Ok(browser_report_from_tool_result(
                        WebDiagnosticReport::from_text(result.text, result.is_error),
                        call,
                        &self.data_dir,
                        &format!("MCP server `{}` tool `{}`", server.config.id, tool_name),
                    ));
                }
                Err(error) => {
                    last_error = Some(format!("{} {tool_name}: {error}", server.config.id));
                }
            }
        }
        Err(workspace_engine::ClientError::InvalidInput(format!(
            "No compatible browser diagnostic MCP tool found. Expected one of: {}.{}",
            preferred_tools.join(", "),
            last_error
                .map(|error| format!(" Last error: {error}"))
                .unwrap_or_default()
        )))
    }
}

pub(crate) fn browser_diagnostics_runner_for_config(
    config: &Config,
) -> Option<WebDiagnosticsRunnerHandle> {
    let servers = config
        .active_mcp_servers()
        .into_iter()
        .filter(|server| looks_like_browser_diagnostics_server(server))
        .map(|server| BrowserDiagnosticsMcpServer {
            config: server.clone(),
            auth_token: if server.transport == McpTransport::Http {
                resolve_mcp_token(&server.auth_token_env)
            } else {
                None
            },
        })
        .collect::<Vec<_>>();
    (!servers.is_empty()).then(|| {
        WebDiagnosticsRunnerHandle::new(McpBrowserDiagnosticsRunner {
            servers,
            data_dir: config.data_dir.clone(),
        })
    })
}

/// Copies artifacts into the data dir and records which server answered.
/// A structured report carries that in `via` and is rendered by the engine;
/// only unstructured output gets the prose prefix.
pub(crate) fn browser_report_from_tool_result(
    mut report: WebDiagnosticReport,
    call: &WebDiagnosticCall,
    data_dir: &Path,
    via: &str,
) -> WebDiagnosticReport {
    materialize_browser_artifacts(&mut report, call, data_dir);
    if report.details.is_none() {
        // Unstructured output: keep the old prose prefix, which is the only
        // place the model learns which server answered.
        report.text = format!(
            "{} via {via}:\n{}",
            if report.is_error {
                "Browser diagnostic failed"
            } else {
                "Browser diagnostic result"
            },
            report.text
        );
    }
    report.via = Some(via.to_string());
    report
}

fn materialize_browser_artifacts(
    report: &mut WebDiagnosticReport,
    call: &WebDiagnosticCall,
    data_dir: &Path,
) {
    if report.artifacts.is_empty() {
        return;
    }
    let session_id = call.session_id.as_deref().unwrap_or("unknown-session");
    let task_id = call.task_id.as_deref().unwrap_or("unknown-task");
    let run_id = browser_diagnostic_run_id();
    let relative_dir = PathBuf::from("web-diagnostics")
        .join(session_id)
        .join(task_id)
        .join(run_id);
    let target_dir = data_dir.join(&relative_dir);

    for artifact in &mut report.artifacts {
        let source = PathBuf::from(&artifact.path);
        if !source.is_absolute() || !source.is_file() {
            continue;
        }
        let Some(file_name) = source.file_name() else {
            continue;
        };
        if fs::create_dir_all(&target_dir).is_err() {
            continue;
        }
        let target = target_dir.join(file_name);
        if fs::copy(&source, &target).is_ok() {
            let relative = relative_dir.join(file_name);
            let source_text = artifact.path.clone();
            artifact.path = relative.to_string_lossy().to_string();
            report.text = report.text.replace(&source_text, &artifact.path);
        }
    }
}

fn browser_diagnostic_run_id() -> String {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or_default();
    format!("run-{millis}")
}

fn web_diagnostic_artifact_path(config: &Config, relative_path: &str) -> Result<PathBuf, String> {
    let relative = Path::new(relative_path);
    if relative.is_absolute()
        || !relative_path.starts_with("web-diagnostics/")
        || relative
            .components()
            .any(|component| matches!(component, std::path::Component::ParentDir))
    {
        return Err("artifact path must be a Damaian web-diagnostics relative path".to_string());
    }
    let candidate = config.data_dir.join(relative);
    let data_dir = config
        .data_dir
        .canonicalize()
        .map_err(|error| error.to_string())?;
    let canonical = candidate
        .canonicalize()
        .map_err(|error| error.to_string())?;
    if !canonical.starts_with(&data_dir) {
        return Err("artifact path escapes the Damaian data directory".to_string());
    }
    Ok(canonical)
}

/// Where `POST /api/reveal-web-diagnostic-artifact` may point Finder: exactly
/// what `GET /api/web-diagnostic-artifact` may read (spec 12 `context.md`
/// §3.4), so nothing outside `<data-dir>/web-diagnostics/` can be revealed.
pub(crate) fn web_diagnostic_reveal_target(config: &Config, path: &str) -> Result<PathBuf, String> {
    web_diagnostic_artifact_path(config, path)
}

fn reveal_web_diagnostic_artifact(config: &Config, path: &str) -> Result<PathBuf, String> {
    let file = web_diagnostic_reveal_target(config, path)?;
    let status = Command::new("open")
        .arg("-R")
        .arg(&file)
        .status()
        .map_err(|error| format!("failed to open Finder: {error}"))?;
    if status.success() {
        Ok(file)
    } else {
        Err(format!("Finder launch failed with status {status}"))
    }
}

/// A recorded diagnostic as the card reads it, live or on reload. The record
/// is camelCase at the envelope and snake_case inside `report`.
pub(crate) fn web_diagnostic_json(record: &WebDiagnosticRecord) -> String {
    serde_json::to_string(record).unwrap_or_else(|_| "{}".to_string())
}

fn content_type_for_path(path: &Path) -> &'static str {
    match path
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase()
        .as_str()
    {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "webp" => "image/webp",
        "gif" => "image/gif",
        "json" => "application/json; charset=utf-8",
        "txt" | "log" => "text/plain; charset=utf-8",
        _ => "application/octet-stream",
    }
}

fn looks_like_browser_diagnostics_server(server: &McpServerConfig) -> bool {
    let haystack = format!(
        "{} {} {} {} {}",
        server.id,
        server.label,
        server.command,
        server.args.join(" "),
        server.url
    )
    .to_ascii_lowercase();
    haystack.contains("playwright")
        || haystack.contains("browser")
        || haystack.contains("web-diagnostic")
}

pub(crate) fn mcp_browser_arguments(
    tool_name: &str,
    call: &WebDiagnosticCall,
) -> workspace_engine::Result<String> {
    if tool_name != "run_scenario" || call.kind != WebDiagnosticKind::Scenario {
        return Ok(call.arguments_json.clone());
    }
    let mut value = serde_json::from_str::<serde_json::Value>(&call.arguments_json)
        .map_err(|error| workspace_engine::ClientError::InvalidInput(error.to_string()))?;
    let Some(object) = value.as_object_mut() else {
        return Ok(call.arguments_json.clone());
    };
    let mut steps = Vec::new();
    steps.push(serde_json::json!({"action": "goto", "url": call.url}));
    if let Some(serde_json::Value::Array(actions)) = object.remove("actions") {
        steps.extend(actions);
    }
    object.insert("steps".to_string(), serde_json::Value::Array(steps));
    Ok(value.to_string())
}
