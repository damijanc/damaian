use std::collections::HashMap;
use std::env;
use std::fs;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::Command;
use workspace_engine::{
    Config, CurlModelTransport, MockModelAdapter, OpenAICompatibleAdapter, WorkspaceEngine,
    command_approval_prompt, patch_diff_text,
};

const INDEX_HTML: &str = include_str!("../static/index.html");
const STYLE_CSS: &str = include_str!("../static/style.css");
const APP_JS: &str = include_str!("../static/app.js");

pub fn run_from_env() -> Result<(), String> {
    run_server(ShellOptions::from_args(env::args().skip(1).collect()))
}

pub fn run_server(options: ShellOptions) -> Result<(), String> {
    let bind = format!("127.0.0.1:{}", options.port);
    let listener = TcpListener::bind(&bind).map_err(|error| format!("bind {bind}: {error}"))?;
    println!("Damaian desktop shell listening at http://{bind}");
    if let Some(repo) = &options.default_repo {
        println!("Default repository: {repo}");
    }

    for stream in listener.incoming() {
        match stream {
            Ok(mut stream) => {
                let options = options.clone();
                if let Err(error) = handle_connection(&mut stream, &options) {
                    let _ =
                        write_response(&mut stream, 500, "application/json", &json_error(&error));
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
}

impl ShellOptions {
    pub fn new(port: u16, default_repo: Option<String>) -> Self {
        Self { port, default_repo }
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
        Self { port, default_repo }
    }
}

fn handle_connection(stream: &mut TcpStream, options: &ShellOptions) -> Result<(), String> {
    let request = read_request(stream)?;
    match (request.method.as_str(), request.path.as_str()) {
        ("GET", "/") => write_response(stream, 200, "text/html; charset=utf-8", INDEX_HTML),
        ("GET", "/style.css") | ("GET", "/assets/style.css") => {
            write_response(stream, 200, "text/css; charset=utf-8", STYLE_CSS)
        }
        ("GET", "/app.js") | ("GET", "/assets/app.js") => {
            write_response(stream, 200, "application/javascript; charset=utf-8", APP_JS)
        }
        ("GET", "/api/bootstrap") => {
            let repo = options.default_repo.clone().unwrap_or_default();
            write_response(
                stream,
                200,
                "application/json",
                &format!("{{\"defaultRepo\":\"{}\"}}", escape_json(&repo)),
            )
        }
        ("GET", "/api/config") => {
            let repo = request.param("repo");
            let config = Config::load_for_repository(repo.as_deref().map(Path::new))
                .map_err(|error| error.to_string())?;
            write_response(
                stream,
                200,
                "application/json",
                &format!(
                    "{{\"policy\":\"{}\"}}",
                    escape_json(&config.to_policy_text())
                ),
            )
        }
        ("GET", "/api/config-file") => {
            let scope = required_param(&request, "scope")?;
            let repo = request.param("repo").unwrap_or_default();
            let path = config_path_for_scope(&scope, &repo)?;
            let content = if path.exists() {
                fs::read_to_string(&path).map_err(|error| error.to_string())?
            } else {
                String::new()
            };
            let config = Config::load_for_repository(if repo.is_empty() {
                None
            } else {
                Some(Path::new(&repo))
            })
            .map_err(|error| error.to_string())?;
            write_response(
                stream,
                200,
                "application/json",
                &format!(
                    "{{\"path\":\"{}\",\"exists\":{},\"content\":\"{}\",\"effectivePolicy\":\"{}\"}}",
                    escape_json(&path.to_string_lossy()),
                    path.exists(),
                    escape_json(&content),
                    escape_json(&config.to_policy_text())
                ),
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
                200,
                "application/json",
                &format!(
                    "{{\"clean\":{},\"exitCode\":{},\"files\":[{}]}}",
                    status.clean, status.exit_code, files
                ),
            )
        }
        ("POST", "/api/open-vscode") => {
            let form = parse_form(&request.body);
            let repo = required_form(&form, "repo")?;
            let path = open_in_vscode(&repo)?;
            write_response(
                stream,
                200,
                "application/json",
                &format!("{{\"path\":\"{}\"}}", escape_json(&path.to_string_lossy())),
            )
        }
        ("POST", "/api/ask") => {
            let form = parse_form(&request.body);
            let repo = required_form(&form, "repo")?;
            let prompt = required_form(&form, "prompt")?;
            let engine = engine_for_repo(&repo)?;
            let mut streamed = String::new();
            let mut on_token = |token: &str| streamed.push_str(token);
            let result =
                if let Some(mock) = form.get("mock_response").filter(|value| !value.is_empty()) {
                    let mut adapter = MockModelAdapter::new(mock.clone());
                    engine
                        .chat_orchestrator
                        .ask(&repo, &prompt, &[], &mut adapter, &mut on_token)
                        .map_err(|error| error.to_string())?
                } else {
                    let api_key = env::var(&engine.config.model_api_key_env).map_err(|_| {
                        format!(
                            "{} is required, or provide a mock response",
                            engine.config.model_api_key_env
                        )
                    })?;
                    let transport = CurlModelTransport::new(&engine.config.model_base_url, api_key);
                    let mut adapter =
                        OpenAICompatibleAdapter::new(&engine.config.model_name, transport);
                    engine
                        .chat_orchestrator
                        .ask(&repo, &prompt, &[], &mut adapter, &mut on_token)
                        .map_err(|error| error.to_string())?
                };
            write_response(
                stream,
                200,
                "application/json",
                &format!(
                    "{{\"response\":\"{}\",\"contextFiles\":[{}],\"sessionId\":\"{}\"}}",
                    escape_json(&result.response),
                    json_string_array(&result.context_files),
                    escape_json(&result.session.id)
                ),
            )
        }
        ("POST", "/api/propose-edit") => {
            let form = parse_form(&request.body);
            let repo = required_form(&form, "repo")?;
            let prompt = required_form(&form, "prompt")?;
            let mock_response = required_form(&form, "model_output")?;
            let engine = engine_for_repo(&repo)?;
            let mut adapter = MockModelAdapter::new(mock_response);
            let result = engine
                .edit_orchestrator
                .propose_edit(&repo, &prompt, &[], &mut adapter)
                .map_err(|error| error.to_string())?;
            write_response(
                stream,
                200,
                "application/json",
                &format!(
                    "{{\"patchId\":\"{}\",\"diff\":\"{}\",\"contextFiles\":[{}]}}",
                    escape_json(&result.patch.id),
                    escape_json(&patch_diff_text(&result.patch)),
                    json_string_array(&result.context_files)
                ),
            )
        }
        ("POST", "/api/apply-patch") => {
            let form = parse_form(&request.body);
            let repo = required_form(&form, "repo")?;
            let patch_id = required_form(&form, "patch_id")?;
            let engine = engine_for_repo(&repo)?;
            let result = engine
                .edit_orchestrator
                .apply_stored_patch(&repo, &patch_id, None, "desktop_user")
                .map_err(|error| error.to_string())?;
            write_response(
                stream,
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
            let scope = required_form(&form, "scope")?;
            let key = required_form(&form, "key")?;
            let value = required_form(&form, "value")?;
            let path = match scope.as_str() {
                "user" => {
                    let config =
                        Config::load_for_repository(None).map_err(|error| error.to_string())?;
                    update_config_overlay(config.user_config_path(), &key, &value)?
                }
                "repo" => {
                    let repo = required_form(&form, "repo")?;
                    update_config_overlay(Config::repository_config_path(&repo), &key, &value)?
                }
                _ => return Err("scope must be user or repo".to_string()),
            };
            write_response(
                stream,
                200,
                "application/json",
                &format!("{{\"path\":\"{}\"}}", escape_json(&path.to_string_lossy())),
            )
        }
        ("POST", "/api/config-file") => {
            let form = parse_form(&request.body);
            let scope = required_form(&form, "scope")?;
            let repo = form.get("repo").cloned().unwrap_or_default();
            let content = form.get("content").cloned().unwrap_or_default();
            let path = config_path_for_scope(&scope, &repo)?;
            save_config_file(&path, &content)?;
            let config = Config::load_for_repository(if repo.is_empty() {
                None
            } else {
                Some(Path::new(&repo))
            })
            .map_err(|error| error.to_string())?;
            write_response(
                stream,
                200,
                "application/json",
                &format!(
                    "{{\"path\":\"{}\",\"effectivePolicy\":\"{}\"}}",
                    escape_json(&path.to_string_lossy()),
                    escape_json(&config.to_policy_text())
                ),
            )
        }
        _ => write_response(stream, 404, "application/json", &json_error("not found")),
    }
}

fn config_path_for_scope(scope: &str, repo: &str) -> Result<PathBuf, String> {
    match scope {
        "user" => {
            let config = Config::load_for_repository(None).map_err(|error| error.to_string())?;
            Ok(config.user_config_path())
        }
        "repo" => {
            if repo.is_empty() {
                return Err("repository is required for repository config".to_string());
            }
            Ok(Config::repository_config_path(repo))
        }
        _ => Err("scope must be user or repo".to_string()),
    }
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

fn validate_working_folder(repo: &str) -> Result<PathBuf, String> {
    let path = fs::canonicalize(repo)
        .map_err(|error| format!("working folder does not exist: {error}"))?;
    if !path.is_dir() {
        return Err("working folder must be a directory".to_string());
    }
    Ok(path)
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
    let repo_path = if repo.is_empty() {
        None
    } else {
        Some(Path::new(repo))
    };
    let config = Config::load_for_repository(repo_path).map_err(|error| error.to_string())?;
    Ok(WorkspaceEngine::new(config))
}

#[derive(Debug, Clone)]
struct Request {
    method: String,
    path: String,
    query: HashMap<String, String>,
    body: String,
}

impl Request {
    fn param(&self, name: &str) -> Option<String> {
        self.query.get(name).cloned()
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
    let content_length = lines
        .filter_map(|line| line.split_once(':'))
        .find(|(key, _)| key.eq_ignore_ascii_case("content-length"))
        .and_then(|(_, value)| value.trim().parse::<usize>().ok())
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
    status: u16,
    content_type: &str,
    body: &str,
) -> Result<(), String> {
    let status_text = match status {
        200 => "OK",
        404 => "Not Found",
        500 => "Internal Server Error",
        _ => "OK",
    };
    let response = format!(
        "HTTP/1.1 {status} {status_text}\r\ncontent-type: {content_type}\r\ncontent-length: {}\r\ncache-control: no-store\r\naccess-control-allow-origin: *\r\nconnection: close\r\n\r\n{body}",
        body.len()
    );
    stream
        .write_all(response.as_bytes())
        .map_err(|error| error.to_string())
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
    use super::{parse_form, percent_decode, save_config_file, validate_working_folder};
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn decodes_forms() {
        let form = parse_form("repo=%2Ftmp%2Fapp&prompt=hello+world");
        assert_eq!(form.get("repo").unwrap(), "/tmp/app");
        assert_eq!(form.get("prompt").unwrap(), "hello world");
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

    fn temp_path(name: &str) -> std::path::PathBuf {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("damaian-desktop-shell-{name}-{stamp}"))
    }
}
