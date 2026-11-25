//! Routes that reach the desktop: the terminal, VS Code, Finder, the
//! browser, and markdown rendering with file links.

use super::*;

pub(crate) fn handle_terminal_cwd(stream: &mut dyn Write, request: &Request) -> Result<(), String> {
    let repo = request.param("repo").unwrap_or_default();
    let cwd = terminal_cwd_for_repo(&repo)?;
    write_response(
        stream,
        request,
        200,
        "application/json",
        &format!("{{\"cwd\":\"{}\"}}", escape_json(&cwd.to_string_lossy())),
    )
}

pub(crate) fn handle_terminal_run(stream: &mut dyn Write, request: &Request) -> Result<(), String> {
    let form = parse_form(&request.body);
    let cwd = form.get("cwd").cloned().unwrap_or_default();
    let command = required_form(&form, "command")?;
    let result = run_terminal_command(&cwd, &command)?;
    write_response(
        stream,
        request,
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

pub(crate) fn handle_open_vscode(stream: &mut dyn Write, request: &Request) -> Result<(), String> {
    let form = parse_form(&request.body);
    let repo = required_form(&form, "repo")?;
    let path = open_in_vscode(&repo)?;
    write_response(
        stream,
        request,
        200,
        "application/json",
        &format!("{{\"path\":\"{}\"}}", escape_json(&path.to_string_lossy())),
    )
}

pub(crate) fn handle_reveal_in_finder(
    stream: &mut dyn Write,
    request: &Request,
) -> Result<(), String> {
    let form = parse_form(&request.body);
    let repo = required_form(&form, "repo")?;
    let path = reveal_in_finder(&repo)?;
    write_response(
        stream,
        request,
        200,
        "application/json",
        &format!("{{\"path\":\"{}\"}}", escape_json(&path.to_string_lossy())),
    )
}

pub(crate) fn handle_open_github(stream: &mut dyn Write, request: &Request) -> Result<(), String> {
    let form = parse_form(&request.body);
    let url = open_github_for_form(&form, open_url_in_browser)?;
    write_response(
        stream,
        request,
        200,
        "application/json",
        &format!("{{\"url\":\"{}\"}}", escape_json(&url)),
    )
}

pub(crate) fn handle_open_vscode_file(
    stream: &mut dyn Write,
    request: &Request,
) -> Result<(), String> {
    let form = parse_form(&request.body);
    let repo = required_form(&form, "repo")?;
    let path = required_form(&form, "path")?;
    let line = form.get("line").and_then(|value| value.parse::<u32>().ok());
    let col = form.get("col").and_then(|value| value.parse::<u32>().ok());
    let opened_path = open_workspace_path_in_vscode(&repo, &path, line, col)?;
    write_response(
        stream,
        request,
        200,
        "application/json",
        &format!(
            "{{\"path\":\"{}\"}}",
            escape_json(&opened_path.to_string_lossy())
        ),
    )
}

pub(crate) fn handle_render_markdown(
    stream: &mut dyn Write,
    request: &Request,
) -> Result<(), String> {
    let form = parse_form(&request.body);
    let content = required_form(&form, "content")?;
    let html = render_markdown_with_optional_file_links(&content, form.get("repo"));
    write_response(
        stream,
        request,
        200,
        "application/json",
        &format!("{{\"html\":\"{}\"}}", escape_json(&html)),
    )
}

/// Renders assistant markdown to HTML, upgrading in-text file references to
/// clickable links when a valid `repo` is supplied. Verification goes
/// through the repo's `path_policy` so restricted files (`.env`, etc.) and
/// paths outside the repo never become links. Any failure to build the
/// per-repo engine falls back to a plain (link-free) render rather than
/// erroring, so message rendering is never blocked by it.
pub(crate) fn render_markdown_with_optional_file_links(
    content: &str,
    repo: Option<&String>,
) -> String {
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

pub(crate) fn open_in_vscode(repo: &str) -> Result<PathBuf, String> {
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

/// Spec 58 requirement 13. Only `repo` is read from the form: the URL is
/// rebuilt from the validated `origin` (`GitHubRepo::url`), never taken from
/// the client or from the raw remote. `open` is injected so tests can see the
/// URL without launching a browser.
pub(crate) fn open_github_for_form(
    form: &HashMap<String, String>,
    open: impl FnOnce(&str) -> Result<(), String>,
) -> Result<String, String> {
    let repo = required_form(form, "repo")?;
    let path = validate_working_folder(&repo)?;
    let path = path.to_string_lossy();
    let summary = engine_for_repo(&path)?
        .git
        .summary(path.as_ref())
        .map_err(|error| error.to_string())?;
    let url = summary
        .github
        .ok_or_else(|| "this folder has no GitHub origin remote".to_string())?
        .url();
    open(&url)?;
    Ok(url)
}

/// Opens `url` in the default browser, the way `reveal_in_finder` opens a
/// folder.
fn open_url_in_browser(url: &str) -> Result<(), String> {
    let status = Command::new("open")
        .arg(url)
        .status()
        .map_err(|error| format!("failed to open the browser: {error}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("browser launch failed with status {status}"))
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

pub(crate) fn validate_context_files(
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

pub(crate) fn validate_working_folder(repo: &str) -> Result<PathBuf, String> {
    let path = fs::canonicalize(repo)
        .map_err(|error| format!("working folder does not exist: {error}"))?;
    if !path.is_dir() {
        return Err("working folder must be a directory".to_string());
    }
    Ok(path)
}

pub(crate) fn validate_workspace_path(repo: &str, relative_path: &str) -> Result<PathBuf, String> {
    let root = validate_working_folder(repo)?;
    let path = fs::canonicalize(root.join(relative_path))
        .map_err(|error| format!("workspace path does not exist: {error}"))?;
    if !path.starts_with(&root) {
        return Err("workspace path must stay inside the selected repository".to_string());
    }
    Ok(path)
}

#[derive(Debug, Clone)]
pub(crate) struct TerminalCommandResult {
    pub(crate) cwd: PathBuf,
    pub(crate) exit_code: i32,
    pub(crate) stdout: String,
    pub(crate) stderr: String,
}

pub fn terminal_cwd_for_repo(repo: &str) -> Result<PathBuf, String> {
    if repo.trim().is_empty() {
        home_dir()
    } else {
        validate_working_folder(repo)
    }
}

pub(crate) fn run_terminal_command(
    cwd: &str,
    command: &str,
) -> Result<TerminalCommandResult, String> {
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
pub(crate) fn launch_vscode(
    path: &Path,
    line: Option<u32>,
    col: Option<u32>,
) -> Result<(), String> {
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
pub(crate) fn launch_vscode(
    path: &Path,
    line: Option<u32>,
    col: Option<u32>,
) -> Result<(), String> {
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
