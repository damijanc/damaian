use std::collections::HashMap;
use std::env;
use std::fs;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use workspace_engine::{
    CancelToken, ChatMessage, ChatTurnOptions, ChatTurnResult, Config, CurlModelTransport,
    GeneratedSecretWarning, McpClient, McpServerConfig, McpTokenResolver, McpTransport,
    OpenAICompatibleAdapter, ProposedFilePatch, ResumeDecisionOptions, Session, TurnPhase,
    TurnProgress, TurnSink, WebDiagnosticCall, WebDiagnosticKind, WebDiagnosticReport,
    WebDiagnosticsRunner, WebDiagnosticsRunnerHandle, WorkspaceEngine, allow_always_eligible,
    command_approval_prompt, normalize_mcp_server_id, normalize_model_provider,
    normalize_model_reasoning_level, parse_hunk_selection, parse_mcp_transport, patch_diff_text,
};

mod keychain;
pub mod terminal;

const INDEX_HTML: &str = include_str!("../static/index.html");
const STYLE_CSS: &str = include_str!("../static/style.css");
const APP_JS: &str = include_str!("../static/app.js");
const XTERM_JS: &str = include_str!("../static/xterm.js");
const XTERM_CSS: &str = include_str!("../static/xterm.css");
const XTERM_ADDON_FIT_JS: &str = include_str!("../static/xterm-addon-fit.js");
// `style-src` allows 'unsafe-inline' because xterm.js's DOM renderer injects a
// <style> element to colour the cursor and size cells; without it the cursor is
// invisible. `script-src` stays strict ('self' only).
const CONTENT_SECURITY_POLICY: &str = "default-src 'self'; connect-src 'self' ipc: http://ipc.localhost; img-src 'self' data: blob:; style-src 'self' 'unsafe-inline'; script-src 'self'; object-src 'none'; base-uri 'none'; frame-ancestors 'none'";
static MODEL_API_KEY_CACHE: OnceLock<Mutex<HashMap<String, String>>> = OnceLock::new();

pub fn run_from_env() -> Result<(), String> {
    run_server(ShellOptions::from_args(env::args().skip(1).collect()))
}

pub fn run_server(options: ShellOptions) -> Result<(), String> {
    run_server_with_ready(options, |_| {})
}

pub fn run_server_with_ready<F>(options: ShellOptions, ready: F) -> Result<(), String>
where
    F: FnOnce(u16),
{
    let bind = format!("127.0.0.1:{}", options.port);
    let listener = TcpListener::bind(&bind).map_err(|error| format!("bind {bind}: {error}"))?;
    let actual_port = listener
        .local_addr()
        .map_err(|error| format!("read listener address: {error}"))?
        .port();
    println!("Damaian desktop shell listening at http://127.0.0.1:{actual_port}");
    if let Some(repo) = &options.default_repo {
        println!("Default repository: {repo}");
    }
    ready(actual_port);

    for stream in listener.incoming() {
        match stream {
            Ok(mut stream) => {
                let options = options.clone();
                if let Err(error) = handle_connection(&mut stream, &options) {
                    let _ = write_basic_response(
                        &mut stream,
                        500,
                        "application/json",
                        &json_error(&error),
                    );
                }
            }
            Err(error) => eprintln!("connection failed: {error}"),
        }
    }
    Ok(())
}

#[derive(Debug, Clone)]
pub struct ShellOptions {
    pub port: u16,
    pub default_repo: Option<String>,
    pub api_token: String,
}

impl ShellOptions {
    pub fn new(port: u16, default_repo: Option<String>) -> Self {
        Self::with_generated_token(port, default_repo)
    }

    pub fn from_args(args: Vec<String>) -> Self {
        let mut port = env::var("DAMAIAN_DESKTOP_PORT")
            .ok()
            .and_then(|value| value.parse().ok())
            .unwrap_or(4765);
        let mut default_repo = None;
        let mut index = 0;
        while index < args.len() {
            match args[index].as_str() {
                "--port" => {
                    if let Some(value) = args.get(index + 1).and_then(|value| value.parse().ok()) {
                        port = value;
                    }
                    index += 2;
                }
                "--repo" => {
                    default_repo = args.get(index + 1).cloned();
                    index += 2;
                }
                _ => index += 1,
            }
        }
        Self::with_generated_token(port, default_repo)
    }

    fn with_generated_token(port: u16, default_repo: Option<String>) -> Self {
        let api_token = generate_api_token();
        Self {
            port,
            default_repo,
            api_token,
        }
    }
}

fn handle_connection(stream: &mut TcpStream, options: &ShellOptions) -> Result<(), String> {
    let request = read_request(stream)?;
    if request.method == "OPTIONS" && request.path.starts_with("/api/") {
        return write_preflight_response(stream, &request);
    }
    if api_request_requires_token(&request.path)
        && let Err(error) = require_api_token(&request, &options.api_token)
    {
        return write_response(
            stream,
            &request,
            401,
            "application/json",
            &json_error(&error),
        );
    }
    match (request.method.as_str(), request.path.as_str()) {
        ("GET", "/") => write_response(
            stream,
            &request,
            200,
            "text/html; charset=utf-8",
            &index_html(),
        ),
        ("GET", "/style.css") | ("GET", "/assets/style.css") => {
            write_response(stream, &request, 200, "text/css; charset=utf-8", STYLE_CSS)
        }
        ("GET", "/app.js") | ("GET", "/assets/app.js") => write_response(
            stream,
            &request,
            200,
            "application/javascript; charset=utf-8",
            APP_JS,
        ),
        ("GET", "/xterm.js") | ("GET", "/assets/xterm.js") => write_response(
            stream,
            &request,
            200,
            "application/javascript; charset=utf-8",
            XTERM_JS,
        ),
        ("GET", "/xterm-addon-fit.js") | ("GET", "/assets/xterm-addon-fit.js") => write_response(
            stream,
            &request,
            200,
            "application/javascript; charset=utf-8",
            XTERM_ADDON_FIT_JS,
        ),
        ("GET", "/xterm.css") | ("GET", "/assets/xterm.css") => {
            write_response(stream, &request, 200, "text/css; charset=utf-8", XTERM_CSS)
        }
        ("GET", "/api/bootstrap") => {
            let repo = options.default_repo.clone().unwrap_or_default();
            write_response(
                stream,
                &request,
                200,
                "application/json",
                &format!("{{\"defaultRepo\":\"{}\"}}", escape_json(&repo)),
            )
        }
        ("GET", "/api/web-diagnostic-artifact") => {
            let repo = request.param("repo").unwrap_or_default();
            let relative_path = request
                .param("path")
                .ok_or_else(|| "path is required".to_string())?;
            let engine = engine_for_repo(&repo)?;
            let path = web_diagnostic_artifact_path(&engine.config, &relative_path)?;
            let bytes = fs::read(&path).map_err(|error| error.to_string())?;
            write_binary_response(stream, &request, 200, content_type_for_path(&path), &bytes)
        }
        ("GET", "/api/config") => {
            let repo = request.param("repo");
            let config = Config::load_for_repository(repo.as_deref().map(Path::new))
                .map_err(|error| error.to_string())?;
            write_response(
                stream,
                &request,
                200,
                "application/json",
                &format!(
                    "{{\"policy\":\"{}\"}}",
                    escape_json(&config.to_policy_text())
                ),
            )
        }
        ("GET", "/api/config-file") => {
            let scope = request.param("scope");
            let repo = request.param("repo").unwrap_or_default();
            let path = desktop_settings_config_path(scope.as_deref())?;
            let content = if path.exists() {
                fs::read_to_string(&path).map_err(|error| error.to_string())?
            } else {
                String::new()
            };
            let (effective_policy, effective_error) = effective_policy_for_repo(&repo);
            write_response(
                stream,
                &request,
                200,
                "application/json",
                &format!(
                    "{{\"path\":\"{}\",\"exists\":{},\"content\":\"{}\",\"effectivePolicy\":\"{}\",\"effectiveError\":\"{}\"}}",
                    escape_json(&path.to_string_lossy()),
                    path.exists(),
                    escape_json(&content),
                    escape_json(&effective_policy),
                    escape_json(&effective_error)
                ),
            )
        }
        ("GET", "/api/model-key-status") => {
            let repo = request.param("repo").unwrap_or_default();
            let model_provider = request.param("model_provider");
            write_response(
                stream,
                &request,
                200,
                "application/json",
                &model_key_status_json(&repo, model_provider.as_deref())?,
            )
        }
        ("GET", "/api/git-status") => {
            let repo = required_param(&request, "repo")?;
            let engine = engine_for_repo(&repo)?;
            let status = engine
                .git
                .status(&repo)
                .map_err(|error| error.to_string())?;
            let files = status
                .files
                .iter()
                .map(|file| {
                    format!(
                        "{{\"path\":\"{}\",\"raw\":\"{}\",\"untracked\":{},\"conflicted\":{}}}",
                        escape_json(&file.path),
                        escape_json(&file.raw),
                        file.untracked,
                        file.conflicted
                    )
                })
                .collect::<Vec<_>>()
                .join(",");
            write_response(
                stream,
                &request,
                200,
                "application/json",
                &format!(
                    "{{\"clean\":{},\"exitCode\":{},\"files\":[{}]}}",
                    status.clean, status.exit_code, files
                ),
            )
        }
        ("GET", "/api/terminal-cwd") => {
            let repo = request.param("repo").unwrap_or_default();
            let cwd = terminal_cwd_for_repo(&repo)?;
            write_response(
                stream,
                &request,
                200,
                "application/json",
                &format!("{{\"cwd\":\"{}\"}}", escape_json(&cwd.to_string_lossy())),
            )
        }
        ("POST", "/api/terminal-run") => {
            let form = parse_form(&request.body);
            let cwd = form.get("cwd").cloned().unwrap_or_default();
            let command = required_form(&form, "command")?;
            let result = run_terminal_command(&cwd, &command)?;
            write_response(
                stream,
                &request,
                200,
                "application/json",
                &format!(
                    "{{\"cwd\":\"{}\",\"exitCode\":{},\"stdout\":\"{}\",\"stderr\":\"{}\"}}",
                    escape_json(&result.cwd.to_string_lossy()),
                    result.exit_code,
                    escape_json(&result.stdout),
                    escape_json(&result.stderr)
                ),
            )
        }
        ("GET", "/api/sessions") => {
            let repo = required_param(&request, "repo")?;
            let engine = engine_for_repo(&repo)?;
            let repository_id = engine
                .indexer
                .repository_id_for_path(&repo)
                .map_err(|error| error.to_string())?;
            let sessions = engine
                .session_store
                .list_sessions(Some(&repository_id))
                .map_err(|error| error.to_string())?;
            write_response(
                stream,
                &request,
                200,
                "application/json",
                &format!("{{\"sessions\":[{}]}}", sessions_json(&sessions)),
            )
        }
        ("GET", "/api/session") => {
            let session_id = required_param(&request, "session_id")?;
            let engine = default_engine()?;
            let Some(session) = engine
                .session_store
                .read_session(&session_id)
                .map_err(|error| error.to_string())?
            else {
                return Err(format!("Unknown session: {session_id}"));
            };
            let messages = engine
                .session_store
                .read_messages(&session_id)
                .map_err(|error| error.to_string())?;
            // Message roles alone cannot say whether a turn was stopped, so the
            // task statuses ride along and the UI joins them by `taskId`.
            let task_statuses = engine
                .session_store
                .read_task_statuses(&session_id)
                .map_err(|error| error.to_string())?;
            write_response(
                stream,
                &request,
                200,
                "application/json",
                &format!(
                    "{{\"session\":{},\"messages\":[{}],\"tasks\":[{}]}}",
                    session_json(&session),
                    messages_json(&messages),
                    task_statuses_json(&task_statuses)
                ),
            )
        }
        ("POST", "/api/session-create") => {
            let form = parse_form(&request.body);
            let repo = required_form(&form, "repo")?;
            let title = form
                .get("title")
                .filter(|value| !value.trim().is_empty())
                .cloned()
                .unwrap_or_else(|| "New session".to_string());
            let engine = engine_for_repo(&repo)?;
            let repository_id = engine
                .indexer
                .repository_id_for_path(&repo)
                .map_err(|error| error.to_string())?;
            let session = engine
                .session_store
                .create_session(&repository_id, &title)
                .map_err(|error| error.to_string())?;
            write_response(
                stream,
                &request,
                200,
                "application/json",
                &format!("{{\"session\":{}}}", session_json(&session)),
            )
        }
        ("POST", "/api/session-rename") => {
            let form = parse_form(&request.body);
            let session_id = required_form(&form, "session_id")?;
            let title = required_form(&form, "title")?;
            let engine = default_engine()?;
            let session = engine
                .session_store
                .rename_session(&session_id, &title)
                .map_err(|error| error.to_string())?;
            write_response(
                stream,
                &request,
                200,
                "application/json",
                &format!("{{\"session\":{}}}", session_json(&session)),
            )
        }
        ("POST", "/api/session-delete") => {
            let form = parse_form(&request.body);
            let session_id = required_form(&form, "session_id")?;
            let engine = default_engine()?;
            engine
                .session_store
                .delete_session(&session_id)
                .map_err(|error| error.to_string())?;
            write_response(
                stream,
                &request,
                200,
                "application/json",
                &format!(
                    "{{\"sessionId\":\"{}\",\"status\":\"deleted\"}}",
                    escape_json(&session_id)
                ),
            )
        }
        ("POST", "/api/open-vscode") => {
            let form = parse_form(&request.body);
            let repo = required_form(&form, "repo")?;
            let path = open_in_vscode(&repo)?;
            write_response(
                stream,
                &request,
                200,
                "application/json",
                &format!("{{\"path\":\"{}\"}}", escape_json(&path.to_string_lossy())),
            )
        }
        ("POST", "/api/reveal-in-finder") => {
            let form = parse_form(&request.body);
            let repo = required_form(&form, "repo")?;
            let path = reveal_in_finder(&repo)?;
            write_response(
                stream,
                &request,
                200,
                "application/json",
                &format!("{{\"path\":\"{}\"}}", escape_json(&path.to_string_lossy())),
            )
        }
        ("POST", "/api/context-file") => {
            let form = parse_form(&request.body);
            let repo = required_form(&form, "repo")?;
            let path = required_form(&form, "path")?;
            let engine = engine_for_repo(&repo)?;
            let files = validate_context_files(&engine, &repo, &path)?;
            let Some(path) = files.first() else {
                return Err("context file is required".to_string());
            };
            write_response(
                stream,
                &request,
                200,
                "application/json",
                &format!("{{\"path\":\"{}\"}}", escape_json(path)),
            )
        }
        ("POST", "/api/open-vscode-file") => {
            let form = parse_form(&request.body);
            let repo = required_form(&form, "repo")?;
            let path = required_form(&form, "path")?;
            let line = form.get("line").and_then(|value| value.parse::<u32>().ok());
            let col = form.get("col").and_then(|value| value.parse::<u32>().ok());
            let opened_path = open_workspace_path_in_vscode(&repo, &path, line, col)?;
            write_response(
                stream,
                &request,
                200,
                "application/json",
                &format!(
                    "{{\"path\":\"{}\"}}",
                    escape_json(&opened_path.to_string_lossy())
                ),
            )
        }
        ("POST", "/api/render-markdown") => {
            let form = parse_form(&request.body);
            let content = required_form(&form, "content")?;
            let html = render_markdown_with_optional_file_links(&content, form.get("repo"));
            write_response(
                stream,
                &request,
                200,
                "application/json",
                &format!("{{\"html\":\"{}\"}}", escape_json(&html)),
            )
        }
        ("POST", "/api/ask-stream") => handle_ask_stream(stream, &request),
        ("POST", "/api/resume-command-stream") => handle_resume_command_stream(stream, &request),
        ("POST", "/api/ask") => {
            let form = parse_form(&request.body);
            // The non-streaming fallback: there is no stream for a client to
            // abort, so nothing here can be stopped. The events go nowhere.
            let (events, _discard) = std::sync::mpsc::channel();
            let result = run_chat_request(&form, &CancelToken::new(), &events)?;
            write_response(
                stream,
                &request,
                200,
                "application/json",
                &chat_result_json(&result),
            )
        }
        ("POST", "/api/propose-edit") => {
            let form = parse_form(&request.body);
            let repo = required_form(&form, "repo")?;
            let prompt = required_form(&form, "prompt")?;
            let engine = engine_for_repo_with_model_options(&repo, &form)?;
            let context_files = form
                .get("context_files")
                .map(|value| validate_context_files(&engine, &repo, value))
                .transpose()?
                .unwrap_or_default();
            let api_key = resolve_model_api_key(&engine.config.model_api_key_env)?;
            let transport = CurlModelTransport::new(&engine.config.model_base_url, api_key);
            let mut adapter = OpenAICompatibleAdapter::with_provider(
                &engine.config.model_provider,
                &engine.config.model_name,
                transport,
            );
            let result = engine
                .edit_orchestrator
                .propose_edit(&repo, &prompt, &context_files, &mut adapter)
                .map_err(|error| error.to_string())?;
            write_response(
                stream,
                &request,
                200,
                "application/json",
                &format!(
                    "{{\"patchId\":\"{}\",\"summary\":\"{}\",\"diff\":\"{}\",\"files\":[{}],\"contextFiles\":[{}]}}",
                    escape_json(&result.patch.id),
                    escape_json(&result.patch.summary),
                    escape_json(&patch_diff_text(&result.patch)),
                    patch_files_json(&result.patch.files),
                    json_string_array(&result.context_files)
                ),
            )
        }
        ("POST", "/api/apply-patch") => {
            let form = parse_form(&request.body);
            let repo = required_form(&form, "repo")?;
            let patch_id = required_form(&form, "patch_id")?;
            let approved_paths = form
                .get("paths")
                .map(|value| parse_path_list(value))
                .transpose()?;
            let hunk_selection = form
                .get("hunk_selection")
                .map(|value| parse_hunk_selection(value).map_err(|error| error.to_string()))
                .transpose()?;
            // Explicit per-apply user decision, sent only after the UI has
            // shown what the scanner found. Absent on the first attempt.
            let allow_generated_secrets = form
                .get("allow_secrets")
                .is_some_and(|value| value == "1" || value == "true");
            let engine = engine_for_repo(&repo)?;
            // Without the override, report what would be blocked instead of
            // failing: the user needs to see which file tripped the check to
            // decide whether to accept it.
            if !allow_generated_secrets && engine.config.block_generated_secrets {
                let flagged = engine
                    .edit_orchestrator
                    .preview_stored_patch_secrets(
                        &repo,
                        &patch_id,
                        approved_paths.as_deref(),
                        hunk_selection.as_ref(),
                    )
                    .map_err(|error| error.to_string())?;
                if !flagged.is_empty() {
                    return write_response(
                        stream,
                        &request,
                        200,
                        "application/json",
                        &format!(
                            "{{\"patchId\":\"{}\",\"appliedFiles\":[],\"warningCount\":{},\"blockedBySecrets\":[{}]}}",
                            escape_json(&patch_id),
                            flagged.len(),
                            generated_secret_warnings_json(&flagged)
                        ),
                    );
                }
            }
            let result = engine
                .edit_orchestrator
                .apply_stored_patch(
                    &repo,
                    &patch_id,
                    approved_paths.as_deref(),
                    hunk_selection.as_ref(),
                    "desktop_user",
                    allow_generated_secrets,
                )
                .map_err(|error| error.to_string())?;
            write_response(
                stream,
                &request,
                200,
                "application/json",
                &format!(
                    "{{\"patchId\":\"{}\",\"appliedFiles\":[{}],\"warningCount\":{}}}",
                    escape_json(&result.patch_id),
                    json_string_array(&result.applied_files),
                    result.warnings.len()
                ),
            )
        }
        ("POST", "/api/rollback-patch") => {
            let form = parse_form(&request.body);
            let repo = required_form(&form, "repo")?;
            let patch_id = required_form(&form, "patch_id")?;
            let selected_paths = form
                .get("paths")
                .map(|value| parse_path_list(value))
                .transpose()?;
            let engine = engine_for_repo(&repo)?;
            let result = engine
                .edit_orchestrator
                .rollback_stored_patch(&repo, &patch_id, selected_paths.as_deref(), "desktop_user")
                .map_err(|error| error.to_string())?;
            write_response(
                stream,
                &request,
                200,
                "application/json",
                &format!(
                    "{{\"patchId\":\"{}\",\"restoredFiles\":[{}],\"deletedFiles\":[{}],\"warnings\":[{}]}}",
                    escape_json(&result.patch_id),
                    json_string_array(&result.restored_files),
                    json_string_array(&result.deleted_files),
                    json_string_array(&result.warnings)
                ),
            )
        }
        ("POST", "/api/reject-patch-files") => {
            let form = parse_form(&request.body);
            let repo = required_form(&form, "repo")?;
            let patch_id = required_form(&form, "patch_id")?;
            let paths = parse_path_list(&required_form(&form, "paths")?)?;
            let engine = engine_for_repo(&repo)?;
            let path = engine
                .edit_orchestrator
                .reject_stored_patch_files(&patch_id, &paths, "desktop_user")
                .map_err(|error| error.to_string())?;
            write_response(
                stream,
                &request,
                200,
                "application/json",
                &format!(
                    "{{\"patchId\":\"{}\",\"rejectedFiles\":[{}],\"path\":\"{}\"}}",
                    escape_json(&patch_id),
                    json_string_array(&paths),
                    escape_json(&path.to_string_lossy())
                ),
            )
        }
        ("POST", "/api/reject-patch") => {
            let form = parse_form(&request.body);
            let patch_id = required_form(&form, "patch_id")?;
            let engine = engine_for_repo(form.get("repo").map(String::as_str).unwrap_or_default())?;
            let path = engine
                .edit_orchestrator
                .reject_stored_patch(&patch_id, "desktop_user")
                .map_err(|error| error.to_string())?;
            write_response(
                stream,
                &request,
                200,
                "application/json",
                &format!(
                    "{{\"patchId\":\"{}\",\"status\":\"rejected\",\"path\":\"{}\"}}",
                    escape_json(&patch_id),
                    escape_json(&path.to_string_lossy())
                ),
            )
        }
        ("POST", "/api/propose-command") => {
            let form = parse_form(&request.body);
            let repo = required_form(&form, "repo")?;
            let command = required_form(&form, "command")?;
            let engine = engine_for_repo(&repo)?;
            let proposal = engine
                .validation_orchestrator
                .propose_command(&repo, &command, "Desktop command proposal")
                .map_err(|error| error.to_string())?;
            write_response(
                stream,
                &request,
                200,
                "application/json",
                &format!(
                    "{{\"proposalId\":\"{}\",\"prompt\":\"{}\",\"risk\":\"{}\",\"requiresApproval\":{},\"blocked\":{},\"allowAlways\":{},\"allowBrowserDiagnosticsForSession\":false}}",
                    escape_json(&proposal.id),
                    escape_json(&command_approval_prompt(&proposal)),
                    proposal.risk.as_str(),
                    proposal.requires_approval,
                    proposal.blocked,
                    allow_always_eligible(&engine.config, &proposal.command, proposal.blocked)
                ),
            )
        }
        ("POST", "/api/run-command") => {
            let form = parse_form(&request.body);
            let proposal_id = required_form(&form, "proposal_id")?;
            let mut engine =
                engine_for_repo(form.get("repo").map(String::as_str).unwrap_or_default())?;

            // Persist the permanent allowance *before* running. If the write
            // fails the command must not run either: silently downgrading
            // "allow always" to a one-time approval would leave the user
            // believing they'd never be asked again.
            let allowlist_path = if form.get("always").map(String::as_str) == Some("true") {
                Some(
                    engine
                        .validation_orchestrator
                        .allow_command_always(&proposal_id, "desktop_user")
                        .map_err(|error| error.to_string())?,
                )
            } else {
                None
            };

            if engine
                .chat_orchestrator
                .has_pending_chat_command(&proposal_id)
            {
                // This proposal paused a chat turn awaiting approval — run
                // the command and feed the result back to the model so it
                // can actually answer, instead of just executing it in
                // isolation and leaving the user without a synthesized
                // response.
                let result = resume_chat_command(
                    &mut engine,
                    &proposal_id,
                    true,
                    resume_decision_options(&form),
                )?;
                write_response(
                    stream,
                    &request,
                    200,
                    "application/json",
                    &chat_result_json(&result),
                )
            } else {
                let record = engine
                    .validation_orchestrator
                    .run_proposal(&proposal_id, true, "desktop_user")
                    .map_err(|error| error.to_string())?;
                write_response(
                    stream,
                    &request,
                    200,
                    "application/json",
                    &format!(
                        "{{\"proposalId\":\"{}\",\"commandId\":\"{}\",\"exitCode\":{},\"stdout\":\"{}\",\"stderr\":\"{}\",\"allowlistPath\":{}}}",
                        escape_json(&record.proposal_id),
                        escape_json(&record.execution.id),
                        record.execution.exit_code.unwrap_or(-1),
                        escape_json(&record.execution.stdout),
                        escape_json(&record.execution.stderr),
                        json_optional_string(allowlist_path.as_deref())
                    ),
                )
            }
        }
        ("POST", "/api/reject-command") => {
            let form = parse_form(&request.body);
            let proposal_id = required_form(&form, "proposal_id")?;
            let mut engine =
                engine_for_repo(form.get("repo").map(String::as_str).unwrap_or_default())?;

            if engine
                .chat_orchestrator
                .has_pending_chat_command(&proposal_id)
            {
                let result = resume_chat_command(
                    &mut engine,
                    &proposal_id,
                    false,
                    ResumeDecisionOptions::default(),
                )?;
                write_response(
                    stream,
                    &request,
                    200,
                    "application/json",
                    &chat_result_json(&result),
                )
            } else {
                let path = engine
                    .validation_orchestrator
                    .reject_proposal(&proposal_id, "desktop_user")
                    .map_err(|error| error.to_string())?;
                write_response(
                    stream,
                    &request,
                    200,
                    "application/json",
                    &format!(
                        "{{\"proposalId\":\"{}\",\"status\":\"rejected\",\"path\":\"{}\"}}",
                        escape_json(&proposal_id),
                        escape_json(&path.to_string_lossy())
                    ),
                )
            }
        }
        ("POST", "/api/config-set") => {
            let form = parse_form(&request.body);
            let scope = form.get("scope").map(String::as_str);
            let key = required_form(&form, "key")?;
            let value = required_form(&form, "value")?;
            let path = desktop_settings_config_path(scope)?;
            let path = update_config_overlay(path, &key, &value)?;
            write_response(
                stream,
                &request,
                200,
                "application/json",
                &format!("{{\"path\":\"{}\"}}", escape_json(&path.to_string_lossy())),
            )
        }
        ("POST", "/api/config-file") => {
            let form = parse_form(&request.body);
            let scope = form.get("scope").map(String::as_str);
            let repo = form.get("repo").cloned().unwrap_or_default();
            let content = form.get("content").cloned().unwrap_or_default();
            let path = desktop_settings_config_path(scope)?;
            save_config_file(&path, &content)?;
            let (effective_policy, effective_error) = effective_policy_for_repo(&repo);
            write_response(
                stream,
                &request,
                200,
                "application/json",
                &format!(
                    "{{\"path\":\"{}\",\"effectivePolicy\":\"{}\",\"effectiveError\":\"{}\"}}",
                    escape_json(&path.to_string_lossy()),
                    escape_json(&effective_policy),
                    escape_json(&effective_error)
                ),
            )
        }
        ("POST", "/api/model-key") => {
            let form = parse_form(&request.body);
            let scope = form.get("scope").map(String::as_str);
            let repo = form.get("repo").cloned().unwrap_or_default();
            let account = required_form(&form, "account")?;
            let api_key = required_form(&form, "api_key")?;
            let reference = keychain::reference_for_account(&account)?;
            keychain::write_password(&account, &api_key)?;
            remember_model_api_key(&account, &api_key);
            let path = desktop_settings_config_path(scope)?;
            update_config_overlay(path.clone(), "model_api_key_env", &reference)?;
            let (effective_policy, effective_error) = effective_policy_for_repo(&repo);
            write_response(
                stream,
                &request,
                200,
                "application/json",
                &format!(
                    "{{\"path\":\"{}\",\"reference\":\"{}\",\"account\":\"{}\",\"configured\":true,\"effectivePolicy\":\"{}\",\"effectiveError\":\"{}\"}}",
                    escape_json(&path.to_string_lossy()),
                    escape_json(&reference),
                    escape_json(account.trim()),
                    escape_json(&effective_policy),
                    escape_json(&effective_error)
                ),
            )
        }
        ("POST", "/api/provider-key") => {
            let form = parse_form(&request.body);
            let account = required_form(&form, "account")?;
            let api_key = required_form(&form, "api_key")?;
            let reference = keychain::reference_for_account(&account)?;
            keychain::write_password(&account, &api_key)?;
            remember_model_api_key(&account, &api_key);
            write_response(
                stream,
                &request,
                200,
                "application/json",
                &format!(
                    "{{\"reference\":\"{}\",\"account\":\"{}\",\"configured\":true}}",
                    escape_json(&reference),
                    escape_json(account.trim())
                ),
            )
        }
        ("POST", "/api/model-key-delete") => {
            let form = parse_form(&request.body);
            let account = required_form(&form, "account")?;
            let deleted = keychain::delete_password(&account)?;
            forget_model_api_key(&account);
            write_response(
                stream,
                &request,
                200,
                "application/json",
                &format!(
                    "{{\"account\":\"{}\",\"deleted\":{},\"configured\":false}}",
                    escape_json(account.trim()),
                    deleted
                ),
            )
        }
        ("POST", "/api/mcp-test") => {
            let form = parse_form(&request.body);
            let body = match mcp_test_connection(&form) {
                Ok(tools) => format!(
                    "{{\"ok\":true,\"toolCount\":{},\"tools\":[{}]}}",
                    tools.len(),
                    json_string_array(&tools)
                ),
                Err(error) => {
                    format!("{{\"ok\":false,\"error\":\"{}\"}}", escape_json(&error))
                }
            };
            write_response(stream, &request, 200, "application/json", &body)
        }
        _ => write_response(
            stream,
            &request,
            404,
            "application/json",
            &json_error("not found"),
        ),
    }
}

fn handle_ask_stream(stream: &mut TcpStream, request: &Request) -> Result<(), String> {
    let form = parse_form(&request.body);
    write_event_stream_headers(stream, request)?;
    stream_turn(stream, move |cancel, events| {
        run_chat_request(&form, cancel, events)
    })
}

fn handle_resume_command_stream(stream: &mut TcpStream, request: &Request) -> Result<(), String> {
    let form = parse_form(&request.body);
    write_event_stream_headers(stream, request)?;
    stream_turn(stream, move |cancel, events| {
        run_resume_command_request(&form, cancel, events)
    })
}

/// Encode raw pty bytes for transport across the IPC boundary as UTF-8-safe
/// text. Shared with the desktop app's terminal commands.
pub fn base64_encode(input: &[u8]) -> String {
    const CHARS: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(input.len().div_ceil(3) * 4);
    for chunk in input.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = chunk.get(1).copied().unwrap_or(0) as u32;
        let b2 = chunk.get(2).copied().unwrap_or(0) as u32;
        let n = (b0 << 16) | (b1 << 8) | b2;
        out.push(CHARS[((n >> 18) & 63) as usize] as char);
        out.push(CHARS[((n >> 12) & 63) as usize] as char);
        out.push(if chunk.len() > 1 {
            CHARS[((n >> 6) & 63) as usize] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            CHARS[(n & 63) as usize] as char
        } else {
            '='
        });
    }
    out
}

fn run_resume_command_request(
    form: &HashMap<String, String>,
    cancel: &CancelToken,
    events: &std::sync::mpsc::Sender<TurnEvent>,
) -> Result<ChatTurnResult, String> {
    let repo = required_form(form, "repo")?;
    let proposal_id = required_form(form, "proposal_id")?;
    let approved = form.get("approved").map(String::as_str) == Some("true");
    let mut engine = engine_for_repo(&repo)?;
    configure_chat_integrations(&mut engine);

    if !engine
        .chat_orchestrator
        .has_pending_chat_command(&proposal_id)
    {
        return Err(format!(
            "No pending chat command for proposal: {proposal_id}"
        ));
    }

    // Record the permanent allowance before resuming the turn, and fail the
    // whole request if it can't be written — see `/api/run-command`. Only
    // meaningful alongside `approved`, since rejecting can't grant anything.
    if approved && form.get("always").map(String::as_str) == Some("true") {
        engine
            .validation_orchestrator
            .allow_command_always(&proposal_id, "desktop_user")
            .map_err(|error| error.to_string())?;
    }

    let api_key = resolve_model_api_key(&engine.config.model_api_key_env)?;
    let transport = CurlModelTransport::new(&engine.config.model_base_url, api_key);
    let mut adapter = OpenAICompatibleAdapter::with_provider(
        &engine.config.model_provider,
        &engine.config.model_name,
        transport,
    );
    let mut on_token = |token: &str| {
        let _ = events.send(TurnEvent::Token(token.to_string()));
    };
    let mut on_progress = |progress: TurnProgress| {
        let _ = events.send(turn_progress_event(progress));
    };
    let mut sink = TurnSink {
        on_token: &mut on_token,
        on_progress: &mut on_progress,
        cancel,
    };
    engine
        .chat_orchestrator
        .resume_after_command_decision_with_options(
            &proposal_id,
            approved,
            "desktop_user",
            &mut adapter,
            &mut sink,
            resume_decision_options(form),
        )
        .map_err(|error| error.to_string())
}

/// A send failure only means the relay has gone, and the cancel token is what
/// stops the turn in that case, so the result is deliberately discarded.
fn turn_progress_event(progress: TurnProgress) -> TurnEvent {
    match progress {
        TurnProgress::Session(session_id) => TurnEvent::Session(session_id),
        TurnProgress::Phase(phase) => TurnEvent::Phase(phase),
    }
}

fn run_chat_request(
    form: &HashMap<String, String>,
    cancel: &CancelToken,
    events: &std::sync::mpsc::Sender<TurnEvent>,
) -> Result<ChatTurnResult, String> {
    let repo = required_form(form, "repo")?;
    let prompt = required_form(form, "prompt")?;
    let session_id = form
        .get("session_id")
        .map(String::as_str)
        .filter(|value| !value.is_empty());
    let mut engine = engine_for_repo_with_model_options(&repo, form)?;
    configure_chat_integrations(&mut engine);
    let context_files = form
        .get("context_files")
        .map(|value| validate_context_files(&engine, &repo, value))
        .transpose()?
        .unwrap_or_default();

    let api_key = resolve_model_api_key(&engine.config.model_api_key_env)?;
    let transport = CurlModelTransport::new(&engine.config.model_base_url, api_key);
    let mut adapter = OpenAICompatibleAdapter::with_provider(
        &engine.config.model_provider,
        &engine.config.model_name,
        transport,
    );
    let mut on_token = |token: &str| {
        let _ = events.send(TurnEvent::Token(token.to_string()));
    };
    let mut on_progress = |progress: TurnProgress| {
        let _ = events.send(turn_progress_event(progress));
    };
    let mut sink = TurnSink {
        on_token: &mut on_token,
        on_progress: &mut on_progress,
        cancel,
    };
    let turn_options = ChatTurnOptions {
        continue_debugging: form.get("continue_debugging").map(String::as_str) == Some("true"),
    };
    engine
        .chat_orchestrator
        .ask_with_session_with_options(
            &repo,
            &prompt,
            &context_files,
            session_id,
            &mut adapter,
            &mut sink,
            turn_options,
        )
        .map_err(|error| error.to_string())
}

/// Continues a chat turn that paused on a command approval, once the user
/// approves or rejects it via `/api/run-command` or `/api/reject-command`.
/// Tokens aren't streamed back here (these endpoints are plain JSON POSTs,
/// not SSE), so they're discarded; the final answer still comes back in the
/// response body.
fn resume_chat_command(
    engine: &mut WorkspaceEngine,
    proposal_id: &str,
    approved: bool,
    decision_options: ResumeDecisionOptions,
) -> Result<ChatTurnResult, String> {
    configure_chat_integrations(engine);
    let api_key = resolve_model_api_key(&engine.config.model_api_key_env)?;
    let transport = CurlModelTransport::new(&engine.config.model_base_url, api_key);
    let mut adapter = OpenAICompatibleAdapter::with_provider(
        &engine.config.model_provider,
        &engine.config.model_name,
        transport,
    );
    let never_cancelled = CancelToken::new();
    let mut on_token = |_token: &str| {};
    let mut on_progress = |_progress: TurnProgress| {};
    let mut sink = TurnSink {
        on_token: &mut on_token,
        on_progress: &mut on_progress,
        cancel: &never_cancelled,
    };
    engine
        .chat_orchestrator
        .resume_after_command_decision_with_options(
            proposal_id,
            approved,
            "desktop_user",
            &mut adapter,
            &mut sink,
            decision_options,
        )
        .map_err(|error| error.to_string())
}

fn resume_decision_options(form: &HashMap<String, String>) -> ResumeDecisionOptions {
    ResumeDecisionOptions {
        allow_browser_diagnostics_for_session: form
            .get("allow_browser_diagnostics_for_session")
            .map(String::as_str)
            == Some("true"),
    }
}

fn default_engine() -> Result<WorkspaceEngine, String> {
    let config = Config::load_for_repository(None).map_err(|error| error.to_string())?;
    Ok(WorkspaceEngine::new(config))
}

fn desktop_settings_config_path(scope: Option<&str>) -> Result<PathBuf, String> {
    match scope.unwrap_or("user") {
        "user" => Ok(Config::default().user_config_path()),
        "repo" => Err(
            "desktop settings only write user config; edit repository config in .damaian/config.conf"
                .to_string(),
        ),
        _ => Err("scope must be user".to_string()),
    }
}

fn effective_policy_for_repo(repo: &str) -> (String, String) {
    match Config::load_for_repository(if repo.is_empty() {
        None
    } else {
        Some(Path::new(repo))
    }) {
        Ok(config) => (config.to_policy_text(), String::new()),
        Err(error) => (String::new(), error.to_string()),
    }
}

fn resolve_model_api_key(reference: &str) -> Result<String, String> {
    if let Some(account) = keychain::account_from_reference(reference) {
        if let Some(api_key) = cached_model_api_key(account) {
            return Ok(api_key);
        }
        let api_key = keychain::read_password(account).map_err(|error| {
            format!(
                "Keychain API key '{}' is required. Open Settings and save the model API key. {error}",
                account
            )
        })?;
        remember_model_api_key(account, &api_key);
        Ok(api_key)
    } else {
        env::var(reference).map_err(|_| format!("{reference} is required"))
    }
}

/// Resolver handed to the chat orchestrator so it can turn an MCP server's
/// `auth_token_env` reference into a bearer token via the keychain (or an
/// environment variable), without the engine ever touching the keychain
/// directly. Mirrors [`resolve_model_api_key`], but returns `None` instead of
/// erroring so a missing token just means "no auth header".
fn mcp_token_resolver() -> McpTokenResolver {
    McpTokenResolver::new(resolve_mcp_token)
}

fn configure_chat_integrations(engine: &mut WorkspaceEngine) {
    engine
        .chat_orchestrator
        .set_mcp_token_resolver(mcp_token_resolver());
    if let Some(runner) = browser_diagnostics_runner_for_config(&engine.config) {
        engine.chat_orchestrator.set_web_diagnostics_runner(runner);
    }
}

fn resolve_mcp_token(reference: &str) -> Option<String> {
    let reference = reference.trim();
    if reference.is_empty() {
        return None;
    }
    if let Some(account) = keychain::account_from_reference(reference) {
        if let Some(token) = cached_model_api_key(account) {
            return Some(token);
        }
        if let Ok(token) = keychain::read_password(account) {
            remember_model_api_key(account, &token);
            return Some(token);
        }
        return None;
    }
    env::var(reference).ok()
}

#[derive(Clone)]
struct McpBrowserDiagnosticsRunner {
    servers: Vec<BrowserDiagnosticsMcpServer>,
    data_dir: PathBuf,
}

#[derive(Clone)]
struct BrowserDiagnosticsMcpServer {
    config: McpServerConfig,
    auth_token: Option<String>,
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
        for server in &self.servers {
            let mut client = match McpClient::connect(&server.config, server.auth_token.clone()) {
                Ok(client) => client,
                Err(error) => {
                    last_error = Some(format!("{}: {error}", server.config.id));
                    continue;
                }
            };
            let tools = match client.list_tools() {
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
            match client.call_tool(tool_name, &arguments) {
                Ok(result) => {
                    let mut report = WebDiagnosticReport::from_text(result.text, result.is_error);
                    materialize_browser_artifacts(&mut report, call, &self.data_dir);
                    report.text = format!(
                        "{} via MCP server `{}` tool `{}`:\n{}",
                        if report.is_error {
                            "Browser diagnostic failed"
                        } else {
                            "Browser diagnostic result"
                        },
                        server.config.id,
                        tool_name,
                        report.text
                    );
                    return Ok(report);
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

fn browser_diagnostics_runner_for_config(config: &Config) -> Option<WebDiagnosticsRunnerHandle> {
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

fn mcp_browser_arguments(
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

/// Builds a one-off [`McpServerConfig`] (plus resolved token) from the MCP
/// editor form, for the Test-connection endpoint. Accepts either a raw
/// `auth_token` (typed but not yet saved) or an `auth_token_env` reference to
/// resolve from the keychain.
fn mcp_config_from_form(
    form: &HashMap<String, String>,
) -> Result<(McpServerConfig, Option<String>), String> {
    let raw_id = form
        .get("id")
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
        .unwrap_or("test");
    let id = normalize_mcp_server_id(raw_id).map_err(|error| error.to_string())?;
    let transport =
        parse_mcp_transport(form.get("transport").map(String::as_str).unwrap_or("stdio"))
            .map_err(|error| error.to_string())?;

    let split = |value: &str| {
        value
            .split('|')
            .map(str::trim)
            .filter(|item| !item.is_empty())
            .map(str::to_string)
            .collect::<Vec<_>>()
    };
    let env = form
        .get("env")
        .map(|value| {
            value
                .split('|')
                .map(str::trim)
                .filter(|item| !item.is_empty())
                .filter_map(|item| {
                    item.split_once('=')
                        .map(|(key, val)| (key.trim().to_string(), val.trim().to_string()))
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    let auth_token_env = form.get("auth_token_env").cloned().unwrap_or_default();
    let token = if let Some(raw) = form
        .get("auth_token")
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
    {
        Some(raw)
    } else if !auth_token_env.trim().is_empty() {
        resolve_mcp_token(&auth_token_env)
    } else {
        None
    };

    let config = McpServerConfig {
        label: form.get("label").cloned().unwrap_or_else(|| id.clone()),
        transport,
        command: form.get("command").cloned().unwrap_or_default(),
        args: form
            .get("args")
            .map(|value| split(value))
            .unwrap_or_default(),
        env,
        url: form
            .get("url")
            .map(|value| value.trim_end_matches('/').to_string())
            .unwrap_or_default(),
        auth_token_env,
        enabled: true,
        require_approval: true,
        id,
    };
    Ok((config, token))
}

/// Connects to the server described by the form and lists its tools. Returns
/// the discovered tool names on success.
fn mcp_test_connection(form: &HashMap<String, String>) -> Result<Vec<String>, String> {
    let (config, token) = mcp_config_from_form(form)?;
    if config.transport == McpTransport::Stdio && config.command.trim().is_empty() {
        return Err("A command is required for a local (stdio) server.".to_string());
    }
    if config.transport == McpTransport::Http && config.url.trim().is_empty() {
        return Err("A URL is required for a remote (http) server.".to_string());
    }
    let mut client = McpClient::connect(&config, token).map_err(|error| error.to_string())?;
    let tools = client.list_tools().map_err(|error| error.to_string())?;
    Ok(tools.into_iter().map(|tool| tool.name).collect())
}

fn model_api_key_cache() -> &'static Mutex<HashMap<String, String>> {
    MODEL_API_KEY_CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn cached_model_api_key(account: &str) -> Option<String> {
    model_api_key_cache()
        .lock()
        .ok()
        .and_then(|cache| cache.get(account.trim()).cloned())
}

fn remember_model_api_key(account: &str, api_key: &str) {
    if let Ok(mut cache) = model_api_key_cache().lock() {
        cache.insert(account.trim().to_string(), api_key.to_string());
    }
}

fn forget_model_api_key(account: &str) {
    if let Ok(mut cache) = model_api_key_cache().lock() {
        cache.remove(account.trim());
    }
}

fn model_key_status_json(repo: &str, provider: Option<&str>) -> Result<String, String> {
    let config = config_for_repo_with_provider(repo, provider)?;
    let reference = config.model_api_key_env;
    if let Some(account) = keychain::account_from_reference(&reference) {
        let status = match keychain::password_exists(account) {
            Ok(configured) => (configured, String::new()),
            Err(error) => (false, error),
        };
        return Ok(format!(
            "{{\"reference\":\"{}\",\"kind\":\"keychain\",\"account\":\"{}\",\"configured\":{},\"message\":\"{}\"}}",
            escape_json(&reference),
            escape_json(account),
            status.0,
            escape_json(&status.1)
        ));
    }

    Ok(format!(
        "{{\"reference\":\"{}\",\"kind\":\"environment\",\"account\":\"\",\"configured\":{},\"message\":\"{}\"}}",
        escape_json(&reference),
        env::var(&reference).is_ok(),
        escape_json(&format!("Environment variable {reference}"))
    ))
}

fn save_config_file(path: &Path, content: &str) -> Result<(), String> {
    workspace_engine::ConfigOverlay::parse(content).map_err(|error| error.to_string())?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    fs::write(path, content).map_err(|error| error.to_string())
}

/// Renders assistant markdown to HTML, upgrading in-text file references to
/// clickable links when a valid `repo` is supplied. Verification goes
/// through the repo's `path_policy` so restricted files (`.env`, etc.) and
/// paths outside the repo never become links. Any failure to build the
/// per-repo engine falls back to a plain (link-free) render rather than
/// erroring, so message rendering is never blocked by it.
fn render_markdown_with_optional_file_links(content: &str, repo: Option<&String>) -> String {
    let Some(repo) = repo.filter(|value| !value.is_empty()) else {
        return workspace_engine::render_markdown_to_html(content);
    };
    let Ok(engine) = engine_for_repo(repo) else {
        return workspace_engine::render_markdown_to_html(content);
    };
    let verifier = |candidate: &str| -> Option<String> {
        let target = engine
            .path_policy
            .resolve_existing(repo, candidate, false)
            .ok()?;
        engine
            .path_policy
            .assert_not_restricted(&target.relative_path, false)
            .ok()?;
        let metadata = fs::metadata(&target.absolute_path).ok()?;
        metadata.is_file().then_some(target.relative_path)
    };
    workspace_engine::render_markdown_to_html_with_file_links(content, &verifier)
}

fn open_in_vscode(repo: &str) -> Result<PathBuf, String> {
    let path = validate_working_folder(repo)?;
    launch_vscode(&path, None, None)?;
    Ok(path)
}

fn reveal_in_finder(repo: &str) -> Result<PathBuf, String> {
    let path = validate_working_folder(repo)?;
    let status = Command::new("open")
        .arg(&path)
        .status()
        .map_err(|error| format!("failed to open Finder: {error}"))?;
    if status.success() {
        Ok(path)
    } else {
        Err(format!("Finder launch failed with status {status}"))
    }
}

fn open_workspace_path_in_vscode(
    repo: &str,
    relative_path: &str,
    line: Option<u32>,
    col: Option<u32>,
) -> Result<PathBuf, String> {
    let path = validate_workspace_path(repo, relative_path)?;
    launch_vscode(&path, line, col)?;
    Ok(path)
}

fn validate_context_files(
    engine: &WorkspaceEngine,
    repo: &str,
    raw_paths: &str,
) -> Result<Vec<String>, String> {
    let mut files = Vec::new();
    for path in parse_optional_path_list(raw_paths) {
        let target = engine
            .path_policy
            .resolve_existing(repo, &path, true)
            .map_err(|error| error.to_string())?;
        engine
            .path_policy
            .assert_not_restricted(&target.relative_path, false)
            .map_err(|error| error.to_string())?;
        let metadata = fs::metadata(&target.absolute_path).map_err(|error| error.to_string())?;
        if !metadata.is_file() {
            return Err("context path must be a file".to_string());
        }
        if !files
            .iter()
            .any(|existing| existing == &target.relative_path)
        {
            files.push(target.relative_path);
        }
    }
    Ok(files)
}

fn validate_working_folder(repo: &str) -> Result<PathBuf, String> {
    let path = fs::canonicalize(repo)
        .map_err(|error| format!("working folder does not exist: {error}"))?;
    if !path.is_dir() {
        return Err("working folder must be a directory".to_string());
    }
    Ok(path)
}

fn validate_workspace_path(repo: &str, relative_path: &str) -> Result<PathBuf, String> {
    let root = validate_working_folder(repo)?;
    let path = fs::canonicalize(root.join(relative_path))
        .map_err(|error| format!("workspace path does not exist: {error}"))?;
    if !path.starts_with(&root) {
        return Err("workspace path must stay inside the selected repository".to_string());
    }
    Ok(path)
}

#[derive(Debug, Clone)]
struct TerminalCommandResult {
    cwd: PathBuf,
    exit_code: i32,
    stdout: String,
    stderr: String,
}

pub fn terminal_cwd_for_repo(repo: &str) -> Result<PathBuf, String> {
    if repo.trim().is_empty() {
        home_dir()
    } else {
        validate_working_folder(repo)
    }
}

fn run_terminal_command(cwd: &str, command: &str) -> Result<TerminalCommandResult, String> {
    let cwd = resolve_terminal_cwd(cwd)?;
    let command = command.trim();
    if command.is_empty() {
        return Err("terminal command is required".to_string());
    }

    if let Some(target) = parse_terminal_cd(command) {
        let cwd = resolve_terminal_target(&cwd, &target)?;
        return Ok(TerminalCommandResult {
            cwd,
            exit_code: 0,
            stdout: String::new(),
            stderr: String::new(),
        });
    }

    let shell = env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".to_string());
    let output = Command::new(shell)
        .arg("-lc")
        .arg(command)
        .current_dir(&cwd)
        .output()
        .map_err(|error| format!("failed to run terminal command: {error}"))?;
    Ok(TerminalCommandResult {
        cwd,
        exit_code: output.status.code().unwrap_or(-1),
        stdout: String::from_utf8_lossy(&output.stdout).to_string(),
        stderr: String::from_utf8_lossy(&output.stderr).to_string(),
    })
}

fn parse_terminal_cd(command: &str) -> Option<String> {
    let trimmed = command.trim();
    if trimmed == "cd" {
        return Some(String::new());
    }
    let target = trimmed.strip_prefix("cd ")?;
    if target.contains(';')
        || target.contains('|')
        || target.contains("&&")
        || target.contains("||")
    {
        return None;
    }
    Some(unquote_terminal_path(target.trim()))
}

fn unquote_terminal_path(value: &str) -> String {
    let quoted = (value.starts_with('"') && value.ends_with('"'))
        || (value.starts_with('\'') && value.ends_with('\''));
    if quoted && value.len() >= 2 {
        value[1..value.len() - 1].to_string()
    } else {
        value.to_string()
    }
}

fn resolve_terminal_cwd(cwd: &str) -> Result<PathBuf, String> {
    if cwd.trim().is_empty() {
        return home_dir();
    }
    let path =
        fs::canonicalize(cwd).map_err(|error| format!("terminal cwd does not exist: {error}"))?;
    if path.is_dir() {
        Ok(path)
    } else {
        Err("terminal cwd must be a directory".to_string())
    }
}

fn resolve_terminal_target(cwd: &Path, target: &str) -> Result<PathBuf, String> {
    let target = target.trim();
    let path = if target.is_empty() {
        home_dir()?
    } else {
        let expanded = expand_home_path(target)?;
        if expanded.is_absolute() {
            expanded
        } else {
            cwd.join(expanded)
        }
    };
    let path = fs::canonicalize(path)
        .map_err(|error| format!("terminal target does not exist: {error}"))?;
    if path.is_dir() {
        Ok(path)
    } else {
        Err("terminal target must be a directory".to_string())
    }
}

fn expand_home_path(value: &str) -> Result<PathBuf, String> {
    if value == "~" {
        return home_dir();
    }
    if let Some(rest) = value.strip_prefix("~/") {
        return Ok(home_dir()?.join(rest));
    }
    Ok(PathBuf::from(value))
}

fn home_dir() -> Result<PathBuf, String> {
    let home = env::var("HOME").map_err(|_| "HOME is not set".to_string())?;
    let path = fs::canonicalize(home)
        .map_err(|error| format!("home directory is unavailable: {error}"))?;
    if path.is_dir() {
        Ok(path)
    } else {
        Err("HOME must point to a directory".to_string())
    }
}

/// Builds the `code --goto` target string `path[:line[:col]]`, preserving
/// the path's bytes (which may contain spaces) since it's passed as a single
/// argv entry, not through a shell.
fn goto_target(path: &Path, line: u32, col: Option<u32>) -> std::ffi::OsString {
    let mut target = path.as_os_str().to_os_string();
    target.push(format!(":{line}"));
    if let Some(col) = col {
        target.push(format!(":{col}"));
    }
    target
}

#[cfg(target_os = "macos")]
fn launch_vscode(path: &Path, line: Option<u32>, col: Option<u32>) -> Result<(), String> {
    // `open -a` cannot jump to a line, so when one is requested try the
    // `code` CLI's `--goto` first. If `code` isn't on PATH (the user never
    // installed the shell command) fall back to `open -a`, which still opens
    // the file — just not at the exact line.
    if let Some(line) = line
        && let Ok(status) = Command::new("code")
            .arg("--goto")
            .arg(goto_target(path, line, col))
            .status()
        && status.success()
    {
        return Ok(());
    }
    let status = Command::new("open")
        .arg("-a")
        .arg("Visual Studio Code")
        .arg(path)
        .status()
        .map_err(|error| format!("failed to launch Visual Studio Code: {error}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!(
            "Visual Studio Code launch failed with status {status}"
        ))
    }
}

#[cfg(not(target_os = "macos"))]
fn launch_vscode(path: &Path, line: Option<u32>, col: Option<u32>) -> Result<(), String> {
    let mut command = Command::new("code");
    match line {
        Some(line) => {
            command.arg("--goto").arg(goto_target(path, line, col));
        }
        None => {
            command.arg(path);
        }
    }
    let status = command
        .status()
        .map_err(|error| format!("failed to launch Visual Studio Code: {error}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!(
            "Visual Studio Code launch failed with status {status}"
        ))
    }
}

fn update_config_overlay(
    path: std::path::PathBuf,
    key: &str,
    value: &str,
) -> Result<std::path::PathBuf, String> {
    let mut overlay = workspace_engine::ConfigOverlay::load_or_default(&path)
        .map_err(|error| error.to_string())?;
    overlay.set(key, value).map_err(|error| error.to_string())?;
    overlay.save(&path).map_err(|error| error.to_string())?;
    Ok(path)
}

fn engine_for_repo(repo: &str) -> Result<WorkspaceEngine, String> {
    let config = config_for_repo(repo)?;
    Ok(WorkspaceEngine::new(config))
}

fn engine_for_repo_with_model_options(
    repo: &str,
    form: &HashMap<String, String>,
) -> Result<WorkspaceEngine, String> {
    let mut config = config_for_repo(repo)?;
    apply_model_form_options(&mut config, form)?;
    Ok(WorkspaceEngine::new(config))
}

fn config_for_repo(repo: &str) -> Result<Config, String> {
    let repo_path = if repo.is_empty() {
        None
    } else {
        Some(Path::new(repo))
    };
    Config::load_for_repository(repo_path).map_err(|error| error.to_string())
}

fn config_for_repo_with_provider(repo: &str, provider: Option<&str>) -> Result<Config, String> {
    let mut config = config_for_repo(repo)?;
    if let Some(provider) = provider.map(str::trim).filter(|value| !value.is_empty()) {
        config.model_provider =
            normalize_model_provider(provider).map_err(|error| error.to_string())?;
        config.apply_model_provider_defaults();
    }
    Ok(config)
}

fn apply_model_form_options(
    config: &mut Config,
    form: &HashMap<String, String>,
) -> Result<(), String> {
    if let Some(provider) = form
        .get("model_provider")
        .map(String::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        config.model_provider =
            normalize_model_provider(provider).map_err(|error| error.to_string())?;
        config.apply_model_provider_defaults();
    }
    if let Some(model) = form
        .get("model")
        .map(String::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        config.model_name = model.to_string();
    }
    if let Some(reasoning_level) = form
        .get("reasoning_level")
        .map(String::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        config.model_reasoning_level =
            normalize_model_reasoning_level(reasoning_level).map_err(|error| error.to_string())?;
    }
    Ok(())
}

#[derive(Debug, Clone)]
struct Request {
    method: String,
    path: String,
    query: HashMap<String, String>,
    headers: HashMap<String, String>,
    body: String,
}

impl Request {
    fn param(&self, name: &str) -> Option<String> {
        self.query.get(name).cloned()
    }

    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .get(&name.to_ascii_lowercase())
            .map(String::as_str)
    }
}

fn read_request(stream: &mut TcpStream) -> Result<Request, String> {
    let mut buffer = Vec::new();
    let mut temp = [0_u8; 8192];
    loop {
        let read = stream.read(&mut temp).map_err(|error| error.to_string())?;
        if read == 0 {
            break;
        }
        buffer.extend_from_slice(&temp[..read]);
        if buffer.windows(4).any(|window| window == b"\r\n\r\n") {
            break;
        }
        if buffer.len() > 1024 * 1024 {
            return Err("request header too large".to_string());
        }
    }
    let header_end = buffer
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .ok_or_else(|| "malformed request".to_string())?
        + 4;
    let header = String::from_utf8_lossy(&buffer[..header_end]).to_string();
    let mut lines = header.lines();
    let request_line = lines
        .next()
        .ok_or_else(|| "missing request line".to_string())?;
    let parts = request_line.split_whitespace().collect::<Vec<_>>();
    if parts.len() < 2 {
        return Err("malformed request line".to_string());
    }
    let headers = lines
        .filter_map(|line| line.split_once(':'))
        .map(|(key, value)| (key.trim().to_ascii_lowercase(), value.trim().to_string()))
        .collect::<HashMap<_, _>>();
    let content_length = headers
        .get("content-length")
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(0);
    while buffer.len() < header_end + content_length {
        let read = stream.read(&mut temp).map_err(|error| error.to_string())?;
        if read == 0 {
            break;
        }
        buffer.extend_from_slice(&temp[..read]);
    }

    let (path, query) = split_path_query(parts[1]);
    let body = String::from_utf8_lossy(
        &buffer
            [header_end..header_end + content_length.min(buffer.len().saturating_sub(header_end))],
    )
    .to_string();
    Ok(Request {
        method: parts[0].to_string(),
        path,
        query,
        headers,
        body,
    })
}

fn split_path_query(raw: &str) -> (String, HashMap<String, String>) {
    let (path, query) = raw.split_once('?').unwrap_or((raw, ""));
    (path.to_string(), parse_form(query))
}

fn required_param(request: &Request, name: &str) -> Result<String, String> {
    request
        .param(name)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| format!("missing query parameter: {name}"))
}

fn required_form(form: &HashMap<String, String>, name: &str) -> Result<String, String> {
    form.get(name)
        .filter(|value| !value.is_empty())
        .cloned()
        .ok_or_else(|| format!("missing form field: {name}"))
}

fn api_request_requires_token(path: &str) -> bool {
    path.starts_with("/api/")
}

fn parse_path_list(value: &str) -> Result<Vec<String>, String> {
    let paths = parse_optional_path_list(value);
    if paths.is_empty() {
        Err("at least one patch file must be selected".to_string())
    } else {
        Ok(paths)
    }
}

fn parse_optional_path_list(value: &str) -> Vec<String> {
    value
        .split(['\n', '|'])
        .map(str::trim)
        .filter(|path| !path.is_empty())
        .map(|path| path.to_string())
        .collect()
}

fn parse_form(body: &str) -> HashMap<String, String> {
    body.split('&')
        .filter(|part| !part.is_empty())
        .map(|part| {
            let (key, value) = part.split_once('=').unwrap_or((part, ""));
            (percent_decode(key), percent_decode(value))
        })
        .collect()
}

fn percent_decode(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut output = Vec::new();
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'+' => output.push(b' '),
            b'%' if index + 2 < bytes.len() => {
                if let (Some(high), Some(low)) =
                    (hex_value(bytes[index + 1]), hex_value(bytes[index + 2]))
                {
                    output.push(high * 16 + low);
                    index += 3;
                    continue;
                }
                output.push(bytes[index]);
            }
            byte => output.push(byte),
        }
        index += 1;
    }
    String::from_utf8_lossy(&output).to_string()
}

fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

fn index_html() -> String {
    INDEX_HTML.to_string()
}

fn write_response(
    stream: &mut TcpStream,
    request: &Request,
    status: u16,
    content_type: &str,
    body: &str,
) -> Result<(), String> {
    write_response_with_extra_headers(stream, request, status, content_type, body, "")
}

fn write_basic_response(
    stream: &mut TcpStream,
    status: u16,
    content_type: &str,
    body: &str,
) -> Result<(), String> {
    let response = format!(
        "HTTP/1.1 {status} {}\r\ncontent-type: {content_type}\r\ncontent-length: {}\r\ncache-control: no-store\r\ncontent-security-policy: {CONTENT_SECURITY_POLICY}\r\nconnection: close\r\n\r\n{body}",
        status_text(status),
        body.len()
    );
    stream
        .write_all(response.as_bytes())
        .map_err(|error| error.to_string())
}

fn write_binary_response(
    stream: &mut TcpStream,
    request: &Request,
    status: u16,
    content_type: &str,
    body: &[u8],
) -> Result<(), String> {
    let cors_headers = cors_headers(request);
    let header = format!(
        "HTTP/1.1 {status} {}\r\ncontent-type: {content_type}\r\ncontent-length: {}\r\ncache-control: no-store\r\ncontent-security-policy: {CONTENT_SECURITY_POLICY}\r\n{cors_headers}connection: close\r\n\r\n",
        status_text(status),
        body.len()
    );
    stream
        .write_all(header.as_bytes())
        .and_then(|_| stream.write_all(body))
        .map_err(|error| error.to_string())
}

fn write_preflight_response(stream: &mut TcpStream, request: &Request) -> Result<(), String> {
    if allowed_cors_origin(request).is_none() {
        return write_response(
            stream,
            request,
            403,
            "application/json",
            &json_error("forbidden"),
        );
    }
    write_response_with_extra_headers(stream, request, 204, "text/plain; charset=utf-8", "", "")
}

fn write_response_with_extra_headers(
    stream: &mut TcpStream,
    request: &Request,
    status: u16,
    content_type: &str,
    body: &str,
    extra_headers: &str,
) -> Result<(), String> {
    let cors_headers = cors_headers(request);
    let response = format!(
        "HTTP/1.1 {status} {}\r\ncontent-type: {content_type}\r\ncontent-length: {}\r\ncache-control: no-store\r\ncontent-security-policy: {CONTENT_SECURITY_POLICY}\r\n{cors_headers}{extra_headers}connection: close\r\n\r\n{body}",
        status_text(status),
        body.len()
    );
    stream
        .write_all(response.as_bytes())
        .map_err(|error| error.to_string())
}

fn write_event_stream_headers(stream: &mut TcpStream, request: &Request) -> Result<(), String> {
    let cors_headers = cors_headers(request);
    let response = format!(
        "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream; charset=utf-8\r\ncache-control: no-store\r\n{cors_headers}connection: close\r\n\r\n"
    );
    stream
        .write_all(response.as_bytes())
        .map_err(|error| error.to_string())
}

fn status_text(status: u16) -> &'static str {
    match status {
        200 => "OK",
        204 => "No Content",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        500 => "Internal Server Error",
        _ => "OK",
    }
}

fn cors_headers(request: &Request) -> String {
    allowed_cors_origin(request)
        .map(|origin| {
            format!(
                "access-control-allow-origin: {origin}\r\naccess-control-allow-methods: GET, POST, OPTIONS\r\naccess-control-allow-headers: content-type, x-damaian-api-token\r\nvary: origin\r\n"
            )
        })
        .unwrap_or_default()
}

fn allowed_cors_origin(request: &Request) -> Option<&str> {
    let origin = request.header("origin")?;
    let allowed = matches!(
        origin,
        "http://tauri.localhost"
            | "https://tauri.localhost"
            | "tauri://localhost"
            | "http://localhost:4765"
            | "http://127.0.0.1:4765"
    );
    allowed.then_some(origin)
}

fn require_api_token(request: &Request, expected_token: &str) -> Result<(), String> {
    if request.header("x-damaian-api-token") == Some(expected_token) {
        Ok(())
    } else {
        Err("unauthorized API request".to_string())
    }
}

fn generate_api_token() -> String {
    let mut bytes = [0_u8; 32];
    getrandom::fill(&mut bytes).expect("secure random token generation failed");
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn write_sse_event<W: Write>(out: &mut W, event: &str, data: &str) -> Result<(), String> {
    out.write_all(format!("event: {event}\ndata: {data}\n\n").as_bytes())
        .and_then(|_| out.flush())
        .map_err(|error| error.to_string())
}

/// How often the handler writes to a silent stream. This is the only thing that
/// reveals a client that has gone away mid-turn: with no data flowing there is
/// nothing else to fail on. Note that on macOS the first write after the peer
/// closes usually succeeds into the kernel buffer, so detection takes one or two
/// of these rather than being instant.
const KEEPALIVE_INTERVAL: Duration = Duration::from_secs(1);

/// What the worker running a turn reports back to the handler holding the socket.
enum TurnEvent {
    Session(String),
    Phase(TurnPhase),
    Token(String),
    Done(Box<ChatTurnResult>),
    Failed(String),
}

fn phase_json(phase: &TurnPhase) -> String {
    format!(
        "{{\"phase\":\"{}\",\"label\":\"{}\",\"round\":{},\"maxRounds\":{}}}",
        phase.kind.as_str(),
        escape_json(&phase.label),
        phase.round,
        phase.max_rounds
    )
}

/// Forwards a turn's events to the client as SSE, and stops the turn if the
/// client goes away.
///
/// Runs on the thread that owns the socket while the turn itself runs on a
/// worker, because the turn spends most of its time blocked on the provider and
/// could not otherwise notice a disconnect.
fn relay_turn_events<W: Write>(
    out: &mut W,
    cancel: &CancelToken,
    events: std::sync::mpsc::Receiver<TurnEvent>,
    keepalive: Duration,
) -> Result<(), String> {
    let mut client_gone = false;
    loop {
        match events.recv_timeout(keepalive) {
            Ok(event) => {
                let finished = matches!(event, TurnEvent::Done(_) | TurnEvent::Failed(_));
                if !client_gone {
                    let written = match &event {
                        TurnEvent::Session(session_id) => write_sse_event(
                            out,
                            "session",
                            &format!("{{\"sessionId\":\"{}\"}}", escape_json(session_id)),
                        ),
                        TurnEvent::Phase(phase) => {
                            write_sse_event(out, "phase", &phase_json(phase))
                        }
                        TurnEvent::Token(token) => write_sse_event(
                            out,
                            "token",
                            &format!("{{\"token\":\"{}\"}}", escape_json(token)),
                        ),
                        TurnEvent::Done(result) => {
                            write_sse_event(out, "done", &chat_result_json(result))
                        }
                        TurnEvent::Failed(error) => {
                            write_sse_event(out, "error", &json_error(&friendly_chat_error(error)))
                        }
                    };
                    if written.is_err() {
                        client_gone = true;
                        cancel.cancel();
                    }
                }
                if finished {
                    return Ok(());
                }
            }
            // Nothing to send: poke the socket so a departed client is noticed.
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                if client_gone {
                    continue;
                }
                // An SSE comment. The client ignores it; the kernel does not.
                if out
                    .write_all(b": keepalive\n\n")
                    .and_then(|_| out.flush())
                    .is_err()
                {
                    client_gone = true;
                    cancel.cancel();
                }
            }
            // The worker dropped its sender, so the turn is over either way.
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => return Ok(()),
        }
    }
}

/// Runs `turn` on a worker thread and relays its events to `stream`.
fn stream_turn<F>(stream: &mut TcpStream, turn: F) -> Result<(), String>
where
    F: FnOnce(&CancelToken, &std::sync::mpsc::Sender<TurnEvent>) -> Result<ChatTurnResult, String>
        + Send
        + 'static,
{
    let (sender, receiver) = std::sync::mpsc::channel();
    let cancel = CancelToken::new();
    let worker_cancel = cancel.clone();
    let worker = std::thread::spawn(move || {
        let event = match turn(&worker_cancel, &sender) {
            Ok(result) => TurnEvent::Done(Box::new(result)),
            Err(error) => TurnEvent::Failed(error),
        };
        let _ = sender.send(event);
        // `sender` drops here, which is what disconnects the channel and lets
        // the relay finish even if the terminal event could not be delivered.
    });

    let outcome = relay_turn_events(stream, &cancel, receiver, KEEPALIVE_INTERVAL);
    // Joined unconditionally: the worker owns the curl child, and leaving it
    // unreaped is how a stopped turn would keep billing tokens.
    let _ = worker.join();
    outcome
}

fn chat_result_json(result: &ChatTurnResult) -> String {
    format!(
        "{{\"response\":\"{}\",\"contextFiles\":[{}],\"sessionId\":\"{}\",\"taskId\":\"{}\",\"taskStatus\":\"{}\",\"modelRunId\":\"{}\",\"incomplete\":{},\"cancelled\":{},\"commandProposal\":{},\"patchProposal\":{}}}",
        escape_json(&result.response),
        json_string_array(&result.context_files),
        escape_json(&result.session.id),
        escape_json(&result.task.id),
        result.task.status.as_str(),
        escape_json(&result.model_run.run_id),
        result.model_run.incomplete,
        result.cancelled,
        command_proposal_json(result),
        patch_proposal_json(result)
    )
}

fn command_proposal_json(result: &ChatTurnResult) -> String {
    let Some(proposal) = &result.command_proposal else {
        return "null".to_string();
    };
    format!(
        "{{\"proposalId\":\"{}\",\"command\":\"{}\",\"prompt\":\"{}\",\"risk\":\"{}\",\"requiresApproval\":{},\"blocked\":{},\"allowAlways\":{},\"allowBrowserDiagnosticsForSession\":{}}}",
        escape_json(&proposal.id),
        escape_json(&proposal.command),
        escape_json(&proposal.prompt),
        escape_json(&proposal.risk),
        proposal.requires_approval,
        proposal.blocked,
        proposal.allow_always,
        proposal.allow_browser_diagnostics_for_session
    )
}

/// Shaped identically to `/api/propose-edit`'s response (`patchId` +
/// `summary` + `files`) so the frontend can render both with the exact same
/// `createPatchPreview` component regardless of whether the patch came from
/// a `propose_patch` tool call mid-chat or the dedicated one-shot edit flow.
fn patch_proposal_json(result: &ChatTurnResult) -> String {
    let Some(proposal) = &result.patch_proposal else {
        return "null".to_string();
    };
    format!(
        "{{\"patchId\":\"{}\",\"summary\":\"{}\",\"files\":[{}]}}",
        escape_json(&proposal.patch_id),
        escape_json(&proposal.summary),
        patch_files_json(&proposal.files)
    )
}

fn patch_files_json(files: &[ProposedFilePatch]) -> String {
    files
        .iter()
        .map(|file| {
            format!(
                "{{\"path\":\"{}\",\"status\":\"{}\",\"baseHash\":{},\"newHash\":\"{}\",\"diff\":\"{}\",\"hunks\":{}}}",
                escape_json(&file.path),
                escape_json(&file.status),
                file.base_hash
                    .as_ref()
                    .map(|hash| format!("\"{}\"", escape_json(hash)))
                    .unwrap_or_else(|| "null".to_string()),
                escape_json(&file.new_hash),
                escape_json(&file.diff),
                serde_json::to_string(&file.hunks).unwrap_or_else(|_| "[]".to_string())
            )
        })
        .collect::<Vec<_>>()
        .join(",")
}

fn sessions_json(sessions: &[Session]) -> String {
    sessions
        .iter()
        .map(session_json)
        .collect::<Vec<_>>()
        .join(",")
}

fn session_json(session: &Session) -> String {
    format!(
        "{{\"id\":\"{}\",\"repositoryId\":\"{}\",\"title\":\"{}\",\"createdAtMs\":{},\"updatedAtMs\":{},\"summary\":\"{}\"}}",
        escape_json(&session.id),
        escape_json(&session.repository_id),
        escape_json(&session.title),
        session.created_at_ms,
        session.updated_at_ms,
        escape_json(&session.summary)
    )
}

fn task_statuses_json(statuses: &HashMap<String, String>) -> String {
    let mut entries: Vec<&String> = statuses.keys().collect();
    // Sorted so the payload is stable between requests.
    entries.sort();
    entries
        .iter()
        .map(|id| {
            format!(
                "{{\"id\":\"{}\",\"status\":\"{}\"}}",
                escape_json(id),
                escape_json(&statuses[*id])
            )
        })
        .collect::<Vec<_>>()
        .join(",")
}

fn messages_json(messages: &[ChatMessage]) -> String {
    messages
        .iter()
        .map(message_json)
        .collect::<Vec<_>>()
        .join(",")
}

fn message_json(message: &ChatMessage) -> String {
    format!(
        "{{\"id\":\"{}\",\"sessionId\":\"{}\",\"taskId\":{},\"role\":\"{}\",\"content\":\"{}\",\"createdAtMs\":{}}}",
        escape_json(&message.id),
        escape_json(&message.session_id),
        message
            .task_id
            .as_ref()
            .map(|value| format!("\"{}\"", escape_json(value)))
            .unwrap_or_else(|| "null".to_string()),
        escape_json(&message.role),
        escape_json(&message.content),
        message.created_at_ms
    )
}

fn friendly_chat_error(error: &str) -> String {
    let lower = error.to_lowercase();
    if !workspace_engine::error::is_retryable_message(error) {
        return error.to_string();
    }
    if lower.contains("rate limit") || lower.contains("429") {
        "Model provider rate limit. Wait for the provider retry window, then try again.".to_string()
    } else if lower.contains("timeout") || lower.contains("timed out") || lower.contains("too slow")
    {
        "Model provider request timed out. Try again, or lower the context size.".to_string()
    } else {
        "Model provider network request failed. Check connectivity and provider URL.".to_string()
    }
}

fn json_error(message: &str) -> String {
    format!("{{\"error\":\"{}\"}}", escape_json(message))
}

/// Serialises secret-scan warnings for the patch UI. Categories and counts
/// only — the matched values never leave the engine.
fn generated_secret_warnings_json(warnings: &[GeneratedSecretWarning]) -> String {
    warnings
        .iter()
        .map(|warning| {
            format!(
                "{{\"path\":\"{}\",\"count\":{},\"categories\":[{}]}}",
                escape_json(&warning.path),
                warning.count,
                json_string_array(&warning.categories)
            )
        })
        .collect::<Vec<_>>()
        .join(",")
}

/// Renders an optional path as a JSON string or `null`, so clients can tell
/// "no allowlist entry was written" apart from "written to the empty path".
fn json_optional_string(value: Option<&Path>) -> String {
    match value {
        Some(path) => format!("\"{}\"", escape_json(&path.to_string_lossy())),
        None => "null".to_string(),
    }
}

fn json_string_array(values: &[String]) -> String {
    values
        .iter()
        .map(|value| format!("\"{}\"", escape_json(value)))
        .collect::<Vec<_>>()
        .join(",")
}

fn escape_json(value: &str) -> String {
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

#[cfg(test)]
mod tests {
    use super::{
        Request, ShellOptions, TurnEvent, allowed_cors_origin, api_request_requires_token,
        cached_model_api_key, desktop_settings_config_path, effective_policy_for_repo,
        engine_for_repo, forget_model_api_key, generated_secret_warnings_json, handle_connection,
        index_html, json_optional_string, keychain, mcp_browser_arguments, parse_form,
        parse_path_list, percent_decode, relay_turn_events, remember_model_api_key,
        render_markdown_with_optional_file_links, require_api_token, run_terminal_command,
        save_config_file, terminal_cwd_for_repo, validate_context_files, validate_working_folder,
        validate_workspace_path,
    };
    use std::collections::HashMap;
    use std::fs;
    use std::io::{Read, Write};
    use std::net::{TcpListener, TcpStream};
    use std::time::{Duration, SystemTime, UNIX_EPOCH};
    use workspace_engine::{CancelToken, Config, GeneratedSecretWarning, WorkspaceEngine};

    /// A sink that starts refusing writes after `writes_before_failure`, the way
    /// a socket does once the client has gone away.
    struct FlakyWriter {
        written: Vec<u8>,
        writes_before_failure: usize,
    }

    impl Write for FlakyWriter {
        fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
            if self.writes_before_failure == 0 {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::BrokenPipe,
                    "client went away",
                ));
            }
            self.writes_before_failure -= 1;
            self.written.extend_from_slice(buffer);
            Ok(buffer.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    // Without a periodic write there is nothing to fail on, so a client that
    // disappeared while the provider was still thinking would go unnoticed —
    // exactly the long-wait case the whole feature exists for.
    #[test]
    fn the_relay_writes_a_keepalive_while_the_turn_is_silent() {
        let (sender, receiver) = std::sync::mpsc::channel();
        let cancel = CancelToken::new();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(150));
            let _ = sender.send(TurnEvent::Token("hi".to_string()));
        });

        let mut out = FlakyWriter {
            written: Vec::new(),
            writes_before_failure: usize::MAX,
        };
        relay_turn_events(&mut out, &cancel, receiver, Duration::from_millis(25)).expect("relay");

        let text = String::from_utf8_lossy(&out.written);
        assert!(text.contains(": keepalive"), "got {text:?}");
        assert!(text.contains("\"token\":\"hi\""), "got {text:?}");
        assert!(!cancel.is_cancelled());
    }

    #[test]
    fn the_relay_cancels_the_turn_once_the_client_stops_listening() {
        let (sender, receiver) = std::sync::mpsc::channel();
        let cancel = CancelToken::new();
        // Sends for long enough that the write failure happens well before the
        // channel disconnects, which is what ends the relay here.
        std::thread::spawn(move || {
            for _ in 0..20 {
                if sender.send(TurnEvent::Token("x".to_string())).is_err() {
                    return;
                }
                std::thread::sleep(Duration::from_millis(10));
            }
        });

        let mut out = FlakyWriter {
            written: Vec::new(),
            writes_before_failure: 2,
        };
        relay_turn_events(&mut out, &cancel, receiver, Duration::from_millis(25)).expect("relay");

        assert!(
            cancel.is_cancelled(),
            "a dead client must stop the turn, not just stop the writes"
        );
    }

    #[test]
    fn legacy_run_scenario_receives_steps_without_actions() {
        let call = workspace_engine::WebDiagnosticCall::from_tool_call(
            "run_web_scenario",
            r##"{
                "url":"http://localhost:5001/",
                "viewport":{"width":1280,"height":720},
                "actions":[
                    {"action":"fill","selector":"#username","value":"tester"},
                    {"action":"click","selector":"#register"}
                ],
                "capture":{"screenshot":true}
            }"##,
        )
        .expect("valid scenario")
        .expect("web diagnostic call");

        let arguments = mcp_browser_arguments("run_scenario", &call).expect("arguments");
        let value: serde_json::Value = serde_json::from_str(&arguments).expect("JSON arguments");

        assert!(value.get("actions").is_none(), "got {value}");
        assert_eq!(
            value["steps"],
            serde_json::json!([
                {"action":"goto","url":"http://localhost:5001/"},
                {"action":"fill","selector":"#username","value":"tester"},
                {"action":"click","selector":"#register"}
            ])
        );
        assert_eq!(value["viewport"]["width"], 1280);
        assert_eq!(value["capture"]["screenshot"], true);
    }

    #[test]
    fn native_run_web_scenario_keeps_the_damaian_actions_contract() {
        let call = workspace_engine::WebDiagnosticCall::from_tool_call(
            "run_web_scenario",
            r##"{"url":"http://localhost:5001/","actions":[{"action":"click","selector":"#register"}]}"##,
        )
        .expect("valid scenario")
        .expect("web diagnostic call");

        assert_eq!(
            mcp_browser_arguments("run_web_scenario", &call).expect("arguments"),
            call.arguments_json
        );
    }

    #[test]
    fn the_relay_reports_a_failed_turn_as_an_error_event() {
        let (sender, receiver) = std::sync::mpsc::channel();
        let cancel = CancelToken::new();
        sender
            .send(TurnEvent::Failed("provider exploded".to_string()))
            .unwrap();
        drop(sender);

        let mut out = FlakyWriter {
            written: Vec::new(),
            writes_before_failure: usize::MAX,
        };
        relay_turn_events(&mut out, &cancel, receiver, Duration::from_millis(25)).expect("relay");

        let text = String::from_utf8_lossy(&out.written);
        assert!(text.contains("event: error"), "got {text:?}");
        assert!(text.contains("provider exploded"), "got {text:?}");
    }

    #[test]
    fn decodes_forms() {
        let form = parse_form("repo=%2Ftmp%2Fapp&prompt=hello+world");
        assert_eq!(form.get("repo").unwrap(), "/tmp/app");
        assert_eq!(form.get("prompt").unwrap(), "hello world");
    }

    #[test]
    fn parses_selected_patch_paths() {
        assert_eq!(
            parse_path_list("src/a.js\nsrc/b.js|src/c.js").unwrap(),
            vec!["src/a.js", "src/b.js", "src/c.js"]
        );
        assert!(parse_path_list(" \n ").is_err());
    }

    #[test]
    fn serializes_generated_secret_warnings_for_the_patch_ui() {
        let json = generated_secret_warnings_json(&[GeneratedSecretWarning {
            path: "docs/\"README\".md".to_string(),
            categories: vec!["credential_assignment".to_string()],
            count: 2,
        }]);

        assert_eq!(
            json,
            "{\"path\":\"docs/\\\"README\\\".md\",\"count\":2,\"categories\":[\"credential_assignment\"]}"
        );
    }

    #[test]
    fn renders_absent_allowlist_path_as_json_null() {
        // `null` rather than `""`, so the UI can tell "no permanent allowance
        // was granted" apart from "granted, path unknown".
        assert_eq!(json_optional_string(None), "null");
        assert_eq!(
            json_optional_string(Some(std::path::Path::new("/tmp/repo/.damaian/config.conf"))),
            "\"/tmp/repo/.damaian/config.conf\""
        );
    }

    #[test]
    fn percent_decodes_invalid_hex_literally() {
        assert_eq!(percent_decode("a%zz"), "a%zz");
    }

    #[test]
    fn percent_decodes_malformed_unicode_adjacent_escape_literally() {
        assert_eq!(percent_decode("%aé"), "%aé");
    }

    #[test]
    fn validates_desktop_api_token_header() {
        let request = test_request_with_headers(&[("x-damaian-api-token", "secret")]);

        assert!(require_api_token(&request, "secret").is_ok());
        assert!(require_api_token(&request, "wrong").is_err());
    }

    #[test]
    fn http_server_never_serves_desktop_api_token() {
        let options = ShellOptions::new(0, Some("/tmp/damaian-repo".to_string()));
        let token = options.api_token.clone();

        let bare_bootstrap_request = test_request("/api/bootstrap", &[]);
        assert!(api_request_requires_token(&bare_bootstrap_request.path));
        assert!(require_api_token(&bare_bootstrap_request, &token).is_err());

        let first_page = index_html();
        assert!(!first_page.contains(&token));
        assert!(!first_page.contains("data-api-token"));
        assert!(!first_page.contains("data-default-repo"));
        assert!(!first_page.contains("/tmp/damaian-repo"));

        let second_page = index_html();
        assert_eq!(first_page, second_page);
        assert!(!second_page.contains(&token));

        let authenticated_bootstrap_request =
            test_request("/api/bootstrap", &[("x-damaian-api-token", &token)]);
        assert!(require_api_token(&authenticated_bootstrap_request, &token).is_ok());
    }

    /// Actually launches Finder, so this is excluded from normal `cargo
    /// test` runs (same reasoning as there being no test for
    /// `open_in_vscode`/`launch_vscode`, which also has a real side effect).
    /// Run manually with `cargo test -p desktop-shell -- --ignored
    /// reveal_in_finder_endpoint_opens_the_requested_repository_root` to
    /// verify end-to-end.
    #[test]
    #[ignore]
    fn reveal_in_finder_endpoint_opens_the_requested_repository_root() {
        let options = ShellOptions::new(0, None);
        let token = options.api_token.clone();
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind test listener");
        let port = listener.local_addr().expect("local addr").port();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { break };
                let _ = handle_connection(&mut stream, &options);
            }
        });

        let repo = std::env::temp_dir();
        let body = format!("repo={}", repo.to_string_lossy());
        let request = format!(
            "POST /api/reveal-in-finder HTTP/1.1\r\nHost: 127.0.0.1\r\ncontent-type: application/x-www-form-urlencoded\r\nx-damaian-api-token: {token}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
            body.len()
        );

        let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("connect to test server");
        stream.write_all(request.as_bytes()).expect("write request");
        let mut response = String::new();
        stream.read_to_string(&mut response).expect("read response");

        assert!(
            response.starts_with("HTTP/1.1 200"),
            "unexpected response: {response}"
        );
    }

    /// The reported bug end to end: `apply selected` on flagged content used
    /// to fail with a `policy_blocked` error the user could not get past. The
    /// route must instead report *what* was found without writing anything,
    /// then apply the same selection once the user consents.
    #[test]
    fn apply_patch_endpoint_warns_then_applies_when_the_user_accepts() {
        let repo = std::env::temp_dir().join(format!(
            "damaian-apply-secret-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(repo.join(".damaian")).unwrap();
        let data_dir = repo.join(".damaian").join("data");
        // Pin data_dir through repo config so the test never touches the
        // user's real application-support directory or a shared env var.
        fs::write(
            repo.join(".damaian").join("config.conf"),
            format!("data_dir={}\n", data_dir.to_string_lossy()),
        )
        .unwrap();
        fs::write(repo.join("config.js"), "export const token = \"\";\n").unwrap();

        let repo_arg = repo.to_string_lossy().to_string();
        let engine = engine_for_repo(&repo_arg).expect("engine for test repo");
        let patch = engine
            .patch_engine
            .create_patch(
                &repo,
                &[workspace_engine::ProposedChange {
                    path: "config.js".to_string(),
                    new_content: "export const api_key = \"sk_live_9f8a7b6c5d4e3f2a1b0c\";\n"
                        .to_string(),
                    status: None,
                    allow_restricted: false,
                }],
                None,
                "add key",
            )
            .expect("create patch");
        engine.patch_store.save(&patch).expect("store patch");

        let options = ShellOptions::new(0, None);
        let token = options.api_token.clone();
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind test listener");
        let port = listener.local_addr().expect("local addr").port();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { break };
                let _ = handle_connection(&mut stream, &options);
            }
        });

        let post = |body: String| {
            let request = format!(
                "POST /api/apply-patch HTTP/1.1\r\nHost: 127.0.0.1\r\ncontent-type: application/x-www-form-urlencoded\r\nx-damaian-api-token: {token}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                body.len()
            );
            let mut stream =
                TcpStream::connect(("127.0.0.1", port)).expect("connect to test server");
            stream.write_all(request.as_bytes()).expect("write request");
            let mut response = String::new();
            stream.read_to_string(&mut response).expect("read response");
            response
        };

        let base = format!(
            "repo={}&patch_id={}",
            percent_encode_for_test(&repo_arg),
            patch.id
        );

        // First attempt: warned, nothing written.
        let warned = post(base.clone());
        assert!(warned.starts_with("HTTP/1.1 200"), "{warned}");
        assert!(warned.contains("\"blockedBySecrets\""), "{warned}");
        assert!(warned.contains("config.js"), "{warned}");
        assert!(warned.contains("credential_assignment"), "{warned}");
        assert!(warned.contains("\"appliedFiles\":[]"), "{warned}");
        assert_eq!(
            fs::read_to_string(repo.join("config.js")).unwrap(),
            "export const token = \"\";\n",
            "a warned apply must not write anything"
        );
        // The response must name the category, never the matched value.
        assert!(!warned.contains("sk_live_9f8a7b6c5d4e3f2a1b0c"), "{warned}");

        // Second attempt: the user accepted.
        let accepted = post(format!("{base}&allow_secrets=1"));
        assert!(accepted.starts_with("HTTP/1.1 200"), "{accepted}");
        assert!(!accepted.contains("\"blockedBySecrets\""), "{accepted}");
        assert!(
            accepted.contains("\"appliedFiles\":[\"config.js\"]"),
            "{accepted}"
        );
        assert_eq!(
            fs::read_to_string(repo.join("config.js")).unwrap(),
            "export const api_key = \"sk_live_9f8a7b6c5d4e3f2a1b0c\";\n"
        );

        fs::remove_dir_all(&repo).ok();
    }

    fn percent_encode_for_test(value: &str) -> String {
        value
            .chars()
            .map(|character| match character {
                'A'..='Z' | 'a'..='z' | '0'..='9' | '-' | '_' | '.' | '~' | '/' => {
                    character.to_string()
                }
                other => format!("%{:02X}", other as u32),
            })
            .collect()
    }

    #[test]
    fn render_markdown_endpoint_returns_syntax_highlighted_html() {
        let options = ShellOptions::new(0, None);
        let token = options.api_token.clone();
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind test listener");
        let port = listener.local_addr().expect("local addr").port();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { break };
                let _ = handle_connection(&mut stream, &options);
            }
        });

        let body = "content=%23%20Title%0A%0A%60%60%60rust%0Afn%20main()%20%7B%7D%0A%60%60%60";
        let request = format!(
            "POST /api/render-markdown HTTP/1.1\r\nHost: 127.0.0.1\r\ncontent-type: application/x-www-form-urlencoded\r\nx-damaian-api-token: {token}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
            body.len()
        );

        let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("connect to test server");
        stream.write_all(request.as_bytes()).expect("write request");
        let mut response = String::new();
        stream.read_to_string(&mut response).expect("read response");

        assert!(
            response.starts_with("HTTP/1.1 200"),
            "unexpected response: {response}"
        );
        assert!(response.contains("<h1>Title</h1>"));
        assert!(response.contains("hl-"));
        assert!(!response.contains("<script>"));
    }

    #[test]
    fn render_markdown_links_only_real_non_restricted_files() {
        let repo = temp_path("render-file-links");
        fs::create_dir_all(repo.join("src")).unwrap();
        fs::write(repo.join("src/auth.rs"), "fn login() {}\n").unwrap();
        fs::write(repo.join(".env"), "SECRET=1\n").unwrap();
        let repo_str = repo.to_string_lossy().to_string();

        let content = "The fix is in src/auth.rs:12, not in src/missing.rs, and never .env.";
        let html = render_markdown_with_optional_file_links(content, Some(&repo_str));

        // Real, allowed file becomes a link with its line number.
        assert!(html.contains("class=\"file-reference\""));
        assert!(html.contains("data-path=\"src/auth.rs\""));
        assert!(html.contains("data-line=\"12\""));
        // Nonexistent path and the restricted .env are left as plain text.
        assert!(html.contains("src/missing.rs"));
        assert!(!html.contains("data-path=\"src/missing.rs\""));
        assert!(!html.contains(".env<"));
        assert!(!html.contains("data-path=\".env\""));

        // Without a repo, nothing is linked.
        let plain = render_markdown_with_optional_file_links(content, None);
        assert!(!plain.contains("file-reference"));

        fs::remove_dir_all(repo).unwrap();
    }

    #[test]
    fn parses_keychain_api_key_references() {
        assert_eq!(
            keychain::account_from_reference("keychain:model-api-key"),
            Some("model-api-key")
        );
        assert_eq!(keychain::account_from_reference("OPENAI_API_KEY"), None);
        assert_eq!(keychain::account_from_reference("keychain:  "), None);
        assert_eq!(
            keychain::reference_for_account(" model-api-key ").unwrap(),
            "keychain:model-api-key"
        );
    }

    #[test]
    fn rejects_invalid_keychain_account_names() {
        assert!(keychain::validate_account("").is_err());
        assert!(keychain::validate_account(" \n ").is_err());
        assert!(keychain::validate_account("model-api-key").is_ok());
    }

    #[test]
    fn caches_model_api_keys_for_current_process() {
        let account = "test-process-cache-model-key";
        forget_model_api_key(account);

        assert_eq!(cached_model_api_key(account), None);
        remember_model_api_key(account, "sk-test-value");
        assert_eq!(
            cached_model_api_key(" test-process-cache-model-key "),
            Some("sk-test-value".to_string())
        );
        forget_model_api_key(account);
        assert_eq!(cached_model_api_key(account), None);
    }

    #[test]
    fn desktop_settings_config_path_is_user_only() {
        assert!(desktop_settings_config_path(None).is_ok());
        assert!(desktop_settings_config_path(Some("user")).is_ok());

        let repo_error = desktop_settings_config_path(Some("repo")).unwrap_err();
        assert!(repo_error.contains("desktop settings only write user config"));

        let unknown_error = desktop_settings_config_path(Some("admin")).unwrap_err();
        assert_eq!(unknown_error, "scope must be user");
    }

    #[test]
    fn only_allows_tauri_cors_origins() {
        let tauri_request = test_request_with_headers(&[("origin", "http://tauri.localhost")]);
        let local_request = test_request_with_headers(&[("origin", "http://localhost:4765")]);
        let browser_request = test_request_with_headers(&[("origin", "https://example.test")]);
        let same_origin_request = test_request_with_headers(&[]);

        assert_eq!(
            allowed_cors_origin(&tauri_request),
            Some("http://tauri.localhost")
        );
        assert_eq!(
            allowed_cors_origin(&local_request),
            Some("http://localhost:4765")
        );
        assert_eq!(allowed_cors_origin(&browser_request), None);
        assert_eq!(allowed_cors_origin(&same_origin_request), None);
    }

    #[test]
    fn saves_valid_config_file() {
        let path = temp_path("valid").join("config").join("user.conf");
        save_config_file(
            &path,
            "model_base_url=https://api.example.test\nmodel_name=test-model\n",
        )
        .unwrap();
        assert_eq!(
            fs::read_to_string(path).unwrap(),
            "model_base_url=https://api.example.test\nmodel_name=test-model\n"
        );
    }

    // The Providers settings panel writes these two keys, so the save endpoint
    // must accept them and the engine must read back what the form wrote.
    // Blank fields are omitted entirely rather than written empty, which is
    // what makes the engine fall back to its per-model defaults.
    #[test]
    fn saves_provider_token_settings_written_by_the_settings_panel() {
        let path = temp_path("token-settings").join("config").join("user.conf");
        let content = "model_provider=deepseek\n\
             model_name=deepseek-v4-flash\n\
             model_provider.deepseek.max_output_tokens=120000\n\
             model_provider.deepseek.context_token_budget=90000\n";
        save_config_file(&path, content).unwrap();

        let overlay =
            workspace_engine::ConfigOverlay::load(&path).expect("saved config must reload");
        let mut config = workspace_engine::Config::default();
        config.apply_overlay(overlay);
        assert_eq!(config.max_output_tokens(), Some(120_000));
        assert_eq!(config.context_token_budget(), 90_000);

        // Omitting the keys restores the built-in per-model defaults.
        let blanked = temp_path("token-settings-blank")
            .join("config")
            .join("user.conf");
        save_config_file(
            &blanked,
            "model_provider=deepseek\nmodel_name=deepseek-v4-flash\n",
        )
        .unwrap();
        let mut defaults = workspace_engine::Config::default();
        defaults.apply_overlay(workspace_engine::ConfigOverlay::load(&blanked).unwrap());
        assert_eq!(defaults.max_output_tokens(), Some(65_536));
        assert_eq!(defaults.context_token_budget(), 64_000);
    }

    #[test]
    fn rejects_invalid_token_settings_without_writing() {
        let path = temp_path("token-settings-invalid").join("config.conf");
        let error = save_config_file(&path, "model_provider.deepseek.context_token_budget=0\n")
            .unwrap_err();
        assert!(error.contains("between 1"), "unexpected error: {error}");
        assert!(!path.exists());
    }

    #[test]
    fn rejects_invalid_config_file_without_writing() {
        let path = temp_path("invalid").join("config.conf");
        let error = save_config_file(&path, "unknown_key=value\n").unwrap_err();
        assert!(error.contains("Unknown config key"));
        assert!(!path.exists());
    }

    #[test]
    fn reports_effective_policy_load_errors_without_panicking() {
        let repo = temp_path("invalid-effective-policy");
        fs::create_dir_all(repo.join(".damaian")).unwrap();
        fs::write(
            repo.join(".damaian").join("config.conf"),
            "unknown_key=value\n",
        )
        .unwrap();

        let (policy, error) = effective_policy_for_repo(repo.to_str().unwrap());

        assert!(policy.is_empty());
        assert!(error.contains("Unknown config key"));
    }

    #[test]
    fn validates_context_files_inside_repo() {
        let repo = temp_path("context-file");
        fs::create_dir_all(repo.join("src")).unwrap();
        let file = repo.join("src").join("main.rs");
        fs::write(&file, "fn main() {}\n").unwrap();
        let engine = WorkspaceEngine::new(Config::default());

        assert_eq!(
            validate_context_files(&engine, repo.to_str().unwrap(), "src/main.rs").unwrap(),
            vec!["src/main.rs"]
        );
        assert_eq!(
            validate_context_files(&engine, repo.to_str().unwrap(), file.to_str().unwrap())
                .unwrap(),
            vec!["src/main.rs"]
        );
    }

    #[test]
    fn rejects_context_directories() {
        let repo = temp_path("context-directory");
        fs::create_dir_all(repo.join("src")).unwrap();
        let engine = WorkspaceEngine::new(Config::default());

        let error = validate_context_files(&engine, repo.to_str().unwrap(), "src").unwrap_err();

        assert_eq!(error, "context path must be a file");
    }

    #[test]
    fn allows_context_files_outside_repo() {
        let repo = temp_path("context-outside");
        fs::create_dir_all(&repo).unwrap();
        let outside = repo.with_file_name(format!(
            "{}-outside.txt",
            repo.file_name().unwrap().to_string_lossy()
        ));
        fs::write(&outside, "notes").unwrap();
        let engine = WorkspaceEngine::new(Config::default());

        let expected = fs::canonicalize(&outside)
            .unwrap()
            .to_string_lossy()
            .replace('\\', "/");
        assert_eq!(
            validate_context_files(&engine, repo.to_str().unwrap(), outside.to_str().unwrap())
                .unwrap(),
            vec![expected]
        );
        fs::remove_file(outside).unwrap();
    }

    #[test]
    fn rejects_restricted_context_files() {
        let repo = temp_path("context-restricted");
        fs::create_dir_all(&repo).unwrap();
        fs::write(repo.join(".env"), "API_KEY=secret\n").unwrap();
        let engine = WorkspaceEngine::new(Config::default());

        let error = validate_context_files(&engine, repo.to_str().unwrap(), ".env").unwrap_err();

        assert!(error.contains("restricted by policy"));
    }

    #[test]
    fn rejects_restricted_context_files_outside_repo() {
        let repo = temp_path("context-restricted-outside-repo");
        fs::create_dir_all(&repo).unwrap();
        let outside_dir = temp_path("context-restricted-outside-dir");
        fs::create_dir_all(&outside_dir).unwrap();
        let outside = outside_dir.join("id_rsa");
        fs::write(&outside, "-----BEGIN PRIVATE KEY-----").unwrap();
        let engine = WorkspaceEngine::new(Config::default());

        let error =
            validate_context_files(&engine, repo.to_str().unwrap(), outside.to_str().unwrap())
                .unwrap_err();

        assert!(error.contains("restricted by policy"));
        fs::remove_dir_all(outside_dir).unwrap();
    }

    #[test]
    fn validates_existing_working_folder() {
        let path = temp_path("working-folder");
        fs::create_dir_all(&path).unwrap();
        let expected = fs::canonicalize(&path).unwrap();
        assert_eq!(
            validate_working_folder(path.to_str().unwrap()).unwrap(),
            expected
        );
    }

    #[test]
    fn rejects_file_as_working_folder() {
        let path = temp_path("working-file");
        fs::write(&path, "not a directory").unwrap();
        let error = validate_working_folder(path.to_str().unwrap()).unwrap_err();
        assert_eq!(error, "working folder must be a directory");
    }

    #[test]
    fn rejects_missing_working_folder() {
        let path = temp_path("missing-folder");
        let error = validate_working_folder(path.to_str().unwrap()).unwrap_err();
        assert!(error.contains("working folder does not exist"));
    }

    #[test]
    fn validates_workspace_path_inside_repo() {
        let repo = temp_path("workspace-path");
        fs::create_dir_all(repo.join("src")).unwrap();
        let file = repo.join("src").join("main.rs");
        fs::write(&file, "fn main() {}\n").unwrap();
        assert_eq!(
            validate_workspace_path(repo.to_str().unwrap(), "src/main.rs").unwrap(),
            fs::canonicalize(file).unwrap()
        );
    }

    #[test]
    fn rejects_workspace_path_outside_repo() {
        let repo = temp_path("workspace-traversal");
        fs::create_dir_all(&repo).unwrap();
        let outside = repo.with_file_name(format!(
            "{}-outside",
            repo.file_name().unwrap().to_string_lossy()
        ));
        fs::write(&outside, "secret").unwrap();
        let relative = format!("../{}", outside.file_name().unwrap().to_string_lossy());
        let error = validate_workspace_path(repo.to_str().unwrap(), &relative).unwrap_err();
        assert_eq!(
            error,
            "workspace path must stay inside the selected repository"
        );
        fs::remove_file(outside).unwrap();
    }

    #[test]
    fn terminal_cwd_uses_selected_working_folder() {
        let repo = temp_path("terminal-cwd");
        fs::create_dir_all(&repo).unwrap();

        assert_eq!(
            terminal_cwd_for_repo(repo.to_str().unwrap()).unwrap(),
            fs::canonicalize(&repo).unwrap()
        );
    }

    #[test]
    fn terminal_cd_updates_cwd_without_shelling_out() {
        let repo = temp_path("terminal-cd");
        fs::create_dir_all(repo.join("child")).unwrap();

        let result = run_terminal_command(repo.to_str().unwrap(), "cd child").unwrap();

        assert_eq!(result.cwd, fs::canonicalize(repo.join("child")).unwrap());
        assert_eq!(result.exit_code, 0);
        assert!(result.stdout.is_empty());
        assert!(result.stderr.is_empty());
    }

    #[test]
    fn terminal_rejects_missing_cwd() {
        let cwd = temp_path("terminal-missing");

        let error = run_terminal_command(cwd.to_str().unwrap(), "pwd").unwrap_err();

        assert!(error.contains("terminal cwd does not exist"));
    }

    fn temp_path(name: &str) -> std::path::PathBuf {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("damaian-desktop-shell-{name}-{stamp}"))
    }

    fn test_request_with_headers(headers: &[(&str, &str)]) -> Request {
        test_request("/api/test", headers)
    }

    fn test_request(path: &str, headers: &[(&str, &str)]) -> Request {
        Request {
            method: "GET".to_string(),
            path: path.to_string(),
            query: HashMap::new(),
            headers: headers
                .iter()
                .map(|(key, value)| (key.to_ascii_lowercase(), value.to_string()))
                .collect(),
            body: String::new(),
        }
    }

    /// End-to-end check of the real pty terminal driven directly through the
    /// `terminal` module (the same entry points the desktop app's Tauri
    /// commands call): open a shell, type a command and confirm the shell's
    /// output comes back over the session's output channel, then resize and
    /// close. Spawns a real login shell, so it is excluded from the default
    /// run.
    #[test]
    #[ignore]
    fn terminal_pty_round_trips_shell_output() {
        let cwd = super::terminal_cwd_for_repo("").expect("resolve terminal cwd");
        let id = super::terminal::open(&cwd, 80, 24).expect("open pty session");
        let receiver = super::terminal::take_output(&id).expect("take output channel");

        let marker = "pty_marker_9931";
        super::terminal::write_input(&id, format!("echo {marker}\n").as_bytes())
            .expect("write to pty");

        let mut decoded = String::new();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(8);
        while std::time::Instant::now() < deadline && !decoded.contains(marker) {
            match receiver.recv_timeout(std::time::Duration::from_millis(200)) {
                Ok(chunk) => decoded.push_str(&String::from_utf8_lossy(&chunk)),
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
            }
        }
        assert!(
            decoded.contains(marker),
            "expected shell output to contain {marker}, got: {decoded:?}"
        );

        super::terminal::resize(&id, 120, 40).expect("resize pty");
        assert!(
            super::terminal::close(&id).is_some(),
            "close should reap the shell"
        );
    }
}
