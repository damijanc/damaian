use std::collections::HashMap;
use std::env;
use std::fs;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Mutex, OnceLock};
use workspace_engine::{
    ChatMessage, ChatTurnResult, Config, CurlModelTransport, OpenAICompatibleAdapter,
    ProposedFilePatch, Session, WorkspaceEngine, command_approval_prompt, normalize_model_provider,
    normalize_model_reasoning_level, patch_diff_text,
};

mod keychain;

const INDEX_HTML: &str = include_str!("../static/index.html");
const STYLE_CSS: &str = include_str!("../static/style.css");
const APP_JS: &str = include_str!("../static/app.js");
const CONTENT_SECURITY_POLICY: &str = "default-src 'self'; connect-src 'self'; img-src 'self' data:; style-src 'self'; script-src 'self'; object-src 'none'; base-uri 'none'; frame-ancestors 'none'";
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
        Self {
            port,
            default_repo,
            api_token: generate_api_token(),
        }
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
        Self {
            port,
            default_repo,
            api_token: generate_api_token(),
        }
    }
}

fn handle_connection(stream: &mut TcpStream, options: &ShellOptions) -> Result<(), String> {
    let request = read_request(stream)?;
    if request.method == "OPTIONS" && request.path.starts_with("/api/") {
        return write_preflight_response(stream, &request);
    }
    if request.path.starts_with("/api/") && request.path != "/api/bootstrap" {
        if let Err(error) = require_api_token(&request, &options.api_token) {
            return write_response(
                stream,
                &request,
                401,
                "application/json",
                &json_error(&error),
            );
        }
    }
    match (request.method.as_str(), request.path.as_str()) {
        ("GET", "/") => write_response(
            stream,
            &request,
            200,
            "text/html; charset=utf-8",
            INDEX_HTML,
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
        ("GET", "/api/bootstrap") => {
            let repo = options.default_repo.clone().unwrap_or_default();
            write_response(
                stream,
                &request,
                200,
                "application/json",
                &format!(
                    "{{\"defaultRepo\":\"{}\",\"apiToken\":\"{}\"}}",
                    escape_json(&repo),
                    escape_json(&options.api_token)
                ),
            )
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
            write_response(
                stream,
                &request,
                200,
                "application/json",
                &format!(
                    "{{\"session\":{},\"messages\":[{}]}}",
                    session_json(&session),
                    messages_json(&messages)
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
            let opened_path = open_workspace_path_in_vscode(&repo, &path)?;
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
        ("POST", "/api/ask-stream") => handle_ask_stream(stream, &request),
        ("POST", "/api/ask") => {
            let form = parse_form(&request.body);
            let mut on_token = |_token: &str| {};
            let result = run_chat_request(&form, &mut on_token)?;
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
            let engine = engine_for_repo(&repo)?;
            let result = engine
                .edit_orchestrator
                .apply_stored_patch(&repo, &patch_id, approved_paths.as_deref(), "desktop_user")
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
                    "{{\"proposalId\":\"{}\",\"prompt\":\"{}\",\"risk\":\"{}\",\"requiresApproval\":{},\"blocked\":{}}}",
                    escape_json(&proposal.id),
                    escape_json(&command_approval_prompt(&proposal)),
                    proposal.risk.as_str(),
                    proposal.requires_approval,
                    proposal.blocked
                ),
            )
        }
        ("POST", "/api/run-command") => {
            let form = parse_form(&request.body);
            let proposal_id = required_form(&form, "proposal_id")?;
            let engine = engine_for_repo(form.get("repo").map(String::as_str).unwrap_or_default())?;
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
                    "{{\"proposalId\":\"{}\",\"commandId\":\"{}\",\"exitCode\":{},\"stdout\":\"{}\",\"stderr\":\"{}\"}}",
                    escape_json(&record.proposal_id),
                    escape_json(&record.execution.id),
                    record.execution.exit_code.unwrap_or(-1),
                    escape_json(&record.execution.stdout),
                    escape_json(&record.execution.stderr)
                ),
            )
        }
        ("POST", "/api/reject-command") => {
            let form = parse_form(&request.body);
            let proposal_id = required_form(&form, "proposal_id")?;
            let engine = engine_for_repo(form.get("repo").map(String::as_str).unwrap_or_default())?;
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

    let mut write_error = None;
    let result = {
        let mut on_token = |token: &str| {
            if write_error.is_none() {
                let data = format!("{{\"token\":\"{}\"}}", escape_json(token));
                if let Err(error) = write_sse_event(stream, "token", &data) {
                    write_error = Some(error);
                }
            }
        };
        run_chat_request(&form, &mut on_token)
    };

    if let Some(error) = write_error {
        return Err(error);
    }

    match result {
        Ok(result) => write_sse_event(stream, "done", &chat_result_json(&result)),
        Err(error) => write_sse_event(stream, "error", &json_error(&friendly_chat_error(&error))),
    }
}

fn run_chat_request(
    form: &HashMap<String, String>,
    on_token: &mut dyn FnMut(&str),
) -> Result<ChatTurnResult, String> {
    let repo = required_form(form, "repo")?;
    let prompt = required_form(form, "prompt")?;
    let session_id = form
        .get("session_id")
        .map(String::as_str)
        .filter(|value| !value.is_empty());
    let engine = engine_for_repo_with_model_options(&repo, form)?;
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
    engine
        .chat_orchestrator
        .ask_with_session(
            &repo,
            &prompt,
            &context_files,
            session_id,
            &mut adapter,
            on_token,
        )
        .map_err(|error| error.to_string())
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

fn open_in_vscode(repo: &str) -> Result<PathBuf, String> {
    let path = validate_working_folder(repo)?;
    launch_vscode(&path)?;
    Ok(path)
}

fn open_workspace_path_in_vscode(repo: &str, relative_path: &str) -> Result<PathBuf, String> {
    let path = validate_workspace_path(repo, relative_path)?;
    launch_vscode(&path)?;
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
            .resolve_existing(repo, &path)
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

fn terminal_cwd_for_repo(repo: &str) -> Result<PathBuf, String> {
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

#[cfg(target_os = "macos")]
fn launch_vscode(path: &Path) -> Result<(), String> {
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
fn launch_vscode(path: &Path) -> Result<(), String> {
    let status = Command::new("code")
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
        .filter_map(|part| {
            let (key, value) = part.split_once('=').unwrap_or((part, ""));
            Some((percent_decode(key), percent_decode(value)))
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

fn write_sse_event(stream: &mut TcpStream, event: &str, data: &str) -> Result<(), String> {
    stream
        .write_all(format!("event: {event}\ndata: {data}\n\n").as_bytes())
        .and_then(|_| stream.flush())
        .map_err(|error| error.to_string())
}

fn chat_result_json(result: &ChatTurnResult) -> String {
    format!(
        "{{\"response\":\"{}\",\"contextFiles\":[{}],\"sessionId\":\"{}\",\"taskId\":\"{}\",\"taskStatus\":\"{}\",\"modelRunId\":\"{}\",\"incomplete\":{},\"commandProposal\":{}}}",
        escape_json(&result.response),
        json_string_array(&result.context_files),
        escape_json(&result.session.id),
        escape_json(&result.task.id),
        result.task.status.as_str(),
        escape_json(&result.model_run.run_id),
        result.model_run.incomplete,
        command_proposal_json(result)
    )
}

fn command_proposal_json(result: &ChatTurnResult) -> String {
    let Some(proposal) = &result.command_proposal else {
        return "null".to_string();
    };
    format!(
        "{{\"proposalId\":\"{}\",\"command\":\"{}\",\"prompt\":\"{}\",\"risk\":\"{}\",\"requiresApproval\":{},\"blocked\":{}}}",
        escape_json(&proposal.id),
        escape_json(&proposal.command),
        escape_json(&proposal.prompt),
        escape_json(&proposal.risk),
        proposal.requires_approval,
        proposal.blocked
    )
}

fn patch_files_json(files: &[ProposedFilePatch]) -> String {
    files
        .iter()
        .map(|file| {
            format!(
                "{{\"path\":\"{}\",\"status\":\"{}\",\"baseHash\":{},\"newHash\":\"{}\",\"diff\":\"{}\"}}",
                escape_json(&file.path),
                escape_json(&file.status),
                file.base_hash
                    .as_ref()
                    .map(|hash| format!("\"{}\"", escape_json(hash)))
                    .unwrap_or_else(|| "null".to_string()),
                escape_json(&file.new_hash),
                escape_json(&file.diff)
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
    if lower.contains("rate limit") || lower.contains("429") {
        "Model provider rate limit. Wait for the provider retry window, then try again.".to_string()
    } else if lower.contains("timeout") || lower.contains("timed out") {
        "Model provider request timed out. Try again, or lower the context size.".to_string()
    } else if lower.contains("connection") || lower.contains("could not resolve") {
        "Model provider network request failed. Check connectivity and provider URL.".to_string()
    } else {
        error.to_string()
    }
}

fn json_error(message: &str) -> String {
    format!("{{\"error\":\"{}\"}}", escape_json(message))
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
        Request, allowed_cors_origin, cached_model_api_key, desktop_settings_config_path,
        effective_policy_for_repo, forget_model_api_key, keychain, parse_form, parse_path_list,
        percent_decode, remember_model_api_key, require_api_token, run_terminal_command,
        save_config_file, terminal_cwd_for_repo, validate_context_files, validate_working_folder,
        validate_workspace_path,
    };
    use std::collections::HashMap;
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};
    use workspace_engine::{Config, WorkspaceEngine};

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
    fn rejects_context_files_outside_repo() {
        let repo = temp_path("context-outside");
        fs::create_dir_all(&repo).unwrap();
        let outside = repo.with_file_name(format!(
            "{}-outside.txt",
            repo.file_name().unwrap().to_string_lossy()
        ));
        fs::write(&outside, "secret").unwrap();
        let engine = WorkspaceEngine::new(Config::default());

        let error =
            validate_context_files(&engine, repo.to_str().unwrap(), outside.to_str().unwrap())
                .unwrap_err();

        assert!(error.contains("outside the selected repository"));
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
        Request {
            method: "GET".to_string(),
            path: "/api/test".to_string(),
            query: HashMap::new(),
            headers: headers
                .iter()
                .map(|(key, value)| (key.to_ascii_lowercase(), value.to_string()))
                .collect(),
            body: String::new(),
        }
    }
}
