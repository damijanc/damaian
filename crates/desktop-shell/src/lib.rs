use std::collections::{BTreeMap, HashMap};
use std::env;
use std::fs;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use workspace_engine::finding::{FindingStatus, RepairRequest};
use workspace_engine::hooks::{
    HookReviewDecision, list_hooks, review_hook, set_hook_enabled, test_run,
};
use workspace_engine::{
    AgentCheckBatch, AgentFallbackProposal, AgentPlanProposal, AgentQuestionProposal,
    AgentRoundBudgetProposal, CURRENT_DATA_SCHEMA_VERSION, CancelToken, ChatMessage,
    ChatTurnOptions, ChatTurnResult, CommitError, Config, CostEstimate, CurlModelTransport,
    DataSchemaOutcome, EffectivePolicy, ExportFormat, GeneratedSecretWarning, HookActivation,
    HookEvent, LanguageServerStatus, McpClient, McpServerConfig, McpTokenResolver, McpTransport,
    ModelAdapter, ModelMessage, ModelRequest, ModelRun, OpenAICompatibleAdapter, PausedTurns,
    PlanRevisionStep, ProcessRegistry, ProfileId, ProposedFilePatch, QuestionState,
    RepositoryConfigReport, ResumeDecisionOptions, RoundBudgetDecision, SearchOptions, Session,
    SessionMode, StepStatus, SymbolRef, TaskPlan, TaskUsage, TokenUsage, TurnNotes, TurnPhase,
    TurnProgress, TurnSink, VerificationReport, WebDiagnosticCall, WebDiagnosticKind,
    WebDiagnosticRecord, WebDiagnosticReport, WebDiagnosticsRunner, WebDiagnosticsRunnerHandle,
    WorkspaceEngine, allow_always_eligible, command_approval_prompt, custom_profile_ids,
    ensure_data_dir_schema, export_profile, import_profile, normalize_mcp_server_id,
    normalize_model_provider, normalize_model_reasoning_level, parse_hunk_selection,
    parse_mcp_transport, patch_diff_text, profile_import_base, repository_id_for_root,
    review_profile_import, review_profile_rejections, select_profile,
};

mod keychain;
mod recovery;
pub mod terminal;
mod worker;
pub mod workers;

pub use worker::{WORKER_FLAG, run_worker_from_args, worker_socket_path};
pub use workers::shutdown_all_workers;

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

/// The stack a request or a turn gets, matching the main thread's 8 MiB.
///
/// The standalone shell serves requests on its main thread, but the desktop
/// app serves them on a spawned thread and every turn runs on one, and a
/// spawned thread defaults to 2 MiB. `handle_connection`'s debug-build frame
/// once reached that by itself; moving route bodies into `handle_*` functions
/// brought it back under, and this keeps the margin rather than relying on
/// every future route staying small.
pub const SHELL_THREAD_STACK_BYTES: usize = 8 * 1024 * 1024;

/// Spawns a thread with [`SHELL_THREAD_STACK_BYTES`]. Use it for anything that
/// runs `handle_connection` or a turn, so a host never gets less stack than the
/// standalone shell's main thread.
pub fn spawn_with_shell_stack<F, T>(
    name: &str,
    work: F,
) -> std::io::Result<std::thread::JoinHandle<T>>
where
    F: FnOnce() -> T + Send + 'static,
    T: Send + 'static,
{
    std::thread::Builder::new()
        .name(name.to_string())
        .stack_size(SHELL_THREAD_STACK_BYTES)
        .spawn(work)
}

/// Resolves the effective data directory and brings its schema marker up to
/// this build's version, or refuses. Called before the shell serves anything,
/// so a directory this build cannot read produces a startup error instead of an
/// empty UI. See `docs/specs/15_install_and_update_verification.md` §5.2.
pub fn verify_data_dir_schema() -> Result<(), String> {
    let config = Config::load_for_repository(None).map_err(|error| error.to_string())?;
    verify_data_dir_schema_at(&config.data_dir)
}

fn verify_data_dir_schema_at(data_dir: &Path) -> Result<(), String> {
    match ensure_data_dir_schema(data_dir).map_err(|error| error.to_string())? {
        DataSchemaOutcome::Adopted => println!(
            "Data directory {} adopted at schema version {CURRENT_DATA_SCHEMA_VERSION}",
            data_dir.display()
        ),
        DataSchemaOutcome::Upgraded { from } => println!(
            "Data directory {} migrated from schema version {from} to {CURRENT_DATA_SCHEMA_VERSION}",
            data_dir.display()
        ),
        DataSchemaOutcome::Initialized | DataSchemaOutcome::Current => {}
    }
    Ok(())
}

/// Kills what a crashed instance left running, per
/// `docs/specs/46_process_registry_and_orphan_sweep/proposal.md` §5.7.
///
/// Eagerly at startup rather than inside `recovery::sweep_once`, which is
/// memoized on the first HTTP request: an orphan must not outlive the crash
/// just because nobody opened the UI. An entry whose owner is still alive is
/// skipped, so running this from both front ends is a no-op the second time.
pub fn sweep_orphaned_processes(config: &Config) -> Result<(), String> {
    let (registry, audit) =
        ProcessRegistry::open_with_audit(config).map_err(|error| error.to_string())?;
    let report = registry.sweep(&audit).map_err(|error| error.to_string())?;
    if report.killed() > 0 || report.refused() > 0 {
        println!(
            "Orphan sweep: killed {}, refused {} on a start-time mismatch",
            report.killed(),
            report.refused()
        );
    }
    Ok(())
}

pub fn run_server_with_ready<F>(options: ShellOptions, ready: F) -> Result<(), String>
where
    F: FnOnce(u16),
{
    verify_data_dir_schema()?;
    // Before the port is bound, so an orphan never overlaps a new session.
    let config = Config::load_for_repository(None).map_err(|error| error.to_string())?;
    sweep_orphaned_processes(&config)?;
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
    // A normal exit: servers get `shutdown` and `exit` before they are
    // killed. A signal goes through `sweep_own` instead (spec 25 context.md §3).
    workspace_engine::LanguageServerHub::shutdown_global();
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

/// Each route's body lives in its own `handle_*` function, not inline in the
/// match. A debug build gives every arm's locals their own stack slot, so one
/// frame pays for all routes at once. With the bodies inline (each holding
/// ~23 KB `WorkspaceEngine` copies) the frame reached 2.05 MiB and overflowed
/// the 2 MiB stack of a spawned thread, which is where both the tests and the
/// desktop app run this.
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
        ("GET", "/api/web-diagnostic-artifact") => handle_web_diagnostic_artifact(stream, &request),
        ("GET", "/api/config") => handle_config(stream, &request),
        ("GET", "/api/config-file") => handle_get_config_file(stream, &request),
        ("GET", "/api/effective-policy") => handle_effective_policy(stream, &request),
        ("POST", "/api/permission-profile") => handle_permission_profile(stream, &request),
        ("GET", "/api/permission-profiles") => handle_permission_profiles(stream, &request),
        ("GET", "/api/permission-profile-export") => {
            handle_permission_profile_export(stream, &request)
        }
        ("POST", "/api/permission-profile-import") => {
            handle_permission_profile_import(stream, &request)
        }
        ("GET", "/api/model-key-status") => handle_model_key_status(stream, &request),
        ("GET", "/api/git-status") => handle_git_status(stream, &request),
        ("POST", "/api/commit-message") => handle_commit_message(stream, &request),
        ("GET", "/api/commit-status") => handle_commit_status(stream, &request),
        ("GET", "/api/commit-preparation") => handle_commit_preparation(stream, &request),
        ("POST", "/api/commit") => handle_commit(stream, &request),
        ("GET", "/api/terminal-cwd") => handle_terminal_cwd(stream, &request),
        ("POST", "/api/terminal-run") => handle_terminal_run(stream, &request),
        ("GET", "/api/sessions") => handle_sessions(stream, &request),
        ("GET", "/api/session-search") => handle_session_search(stream, &request),
        ("GET", "/api/session-export") => handle_session_export(stream, &request),
        // The launch sweep behind `docs/specs/45_crash_recovery_prompt.md`. It
        // runs once per process; every call after the first serves the same
        // snapshot, minus tasks the user has since dealt with.
        ("GET", "/api/recovery") => handle_recovery(stream, &request),
        // Resume, mark failed, or abandon one recovered task. The decision is
        // re-classified server-side: see `recovery::decide`.
        ("POST", "/api/recovery-decision") => handle_recovery_decision(stream, &request),
        // Closes out a task whose reattached approval has been answered, so the
        // next launch does not offer to run the same command again.
        ("POST", "/api/recovery-approval-resolved") => {
            handle_recovery_approval_resolved(stream, &request)
        }
        ("GET", "/api/checkpoints") => handle_checkpoints(stream, &request),
        // Rewind. `files` and `conversation` are independent switches, and
        // `path` narrows the file half to one file, which is the fourth of the
        // restore operations rather than a fourth code path.
        ("POST", "/api/rewind") => handle_rewind(stream, &request),
        ("GET", "/api/session") => handle_session(stream, &request),
        ("POST", "/api/session-create") => handle_session_create(stream, &request),
        ("POST", "/api/session-rename") => handle_session_rename(stream, &request),
        ("POST", "/api/session-mode") => handle_session_mode(stream, &request),
        // Spec 22 Task 10. Each findings route is its own function, so the
        // engine it builds lives in that frame rather than this one: every
        // arm's locals share `handle_connection`'s frame, and inlining three
        // more `WorkspaceEngine`s overflowed a 2 MiB thread stack in a debug
        // build, which is what the desktop app gives the server.
        ("GET", "/api/findings") => handle_findings(stream, &request),
        ("POST", "/api/finding-status") => handle_finding_status(stream, &request),
        ("POST", "/api/findings-repair") => handle_findings_repair(stream, &request),
        ("POST", "/api/session-delete") => handle_session_delete(stream, &request),
        ("POST", "/api/open-vscode") => handle_open_vscode(stream, &request),
        ("POST", "/api/reveal-in-finder") => handle_reveal_in_finder(stream, &request),
        // Spec 58: takes `repo` only and opens the URL derived from `origin`.
        ("POST", "/api/open-github") => handle_open_github(stream, &request),
        ("POST", "/api/reveal-web-diagnostic-artifact") => {
            handle_reveal_web_diagnostic_artifact(stream, &request)
        }
        ("POST", "/api/context-file") => handle_context_file(stream, &request),
        ("POST", "/api/open-vscode-file") => handle_open_vscode_file(stream, &request),
        ("POST", "/api/render-markdown") => handle_render_markdown(stream, &request),
        ("POST", "/api/ask-stream") => handle_ask_stream(stream, &request),
        ("POST", "/api/resume-command-stream") => handle_resume_command_stream(stream, &request),
        ("POST", "/api/verify-stream") => handle_verify_stream(stream, &request),
        ("POST", "/api/resume-checks-stream") => handle_resume_checks_stream(stream, &request),
        ("POST", "/api/resume-plan-stream") => handle_resume_plan_stream(stream, &request),
        ("POST", "/api/answer-question-stream") => handle_answer_question_stream(stream, &request),
        ("POST", "/api/resume-fallback-stream") => handle_resume_fallback_stream(stream, &request),
        ("POST", "/api/resume-round-budget-stream") => {
            handle_resume_round_budget_stream(stream, &request)
        }
        ("POST", "/api/ask") => handle_ask(stream, &request),
        ("POST", "/api/propose-edit") => handle_propose_edit(stream, &request),
        ("POST", "/api/apply-patch") => handle_apply_patch(stream, &request),
        ("POST", "/api/rollback-patch") => handle_rollback_patch(stream, &request),
        ("POST", "/api/reject-patch-files") => handle_reject_patch_files(stream, &request),
        ("POST", "/api/reject-patch") => handle_reject_patch(stream, &request),
        ("POST", "/api/propose-command") => handle_propose_command(stream, &request),
        ("POST", "/api/run-command") => handle_run_command(stream, &request),
        ("POST", "/api/reject-command") => handle_reject_command(stream, &request),
        ("GET", "/api/repository-map") => handle_repository_map(stream, &request),
        ("POST", "/api/repository-roots") => handle_repository_roots(stream, &request),
        // Spec 25 Task 10: symbol search, on-demand references, and the state
        // of each configured language server. Each is its own function for the
        // reason the findings routes are (spec 22 Task 10): the engine they
        // build belongs to that frame, not to `handle_connection`'s.
        ("GET", "/api/symbols") => handle_symbols(stream, &request),
        ("GET", "/api/symbol-references") => handle_symbol_references(stream, &request),
        ("GET", "/api/language-servers") => handle_language_servers(stream, &request),
        ("GET", "/api/command-proposal") => handle_command_proposal(stream, &request),
        ("GET", "/api/repository-config-review") => {
            handle_repository_config_review(stream, &request)
        }
        ("POST", "/api/repository-config-allowlist") => {
            handle_repository_config_allowlist(stream, &request)
        }
        // Spec 32 Task 9: the hooks the user can see, review, switch and test.
        // None of these takes a config path: user config is the only file they
        // can write, and its location comes from the data directory.
        ("GET", "/api/hooks") => handle_hooks(stream, &request),
        ("POST", "/api/hook-review") => handle_hook_review(stream, &request),
        ("POST", "/api/hook-enabled") => handle_hook_enabled(stream, &request),
        ("POST", "/api/hook-test") => handle_hook_test(stream, &request),
        ("POST", "/api/config-set") => handle_config_set(stream, &request),
        ("POST", "/api/config-file") => handle_post_config_file(stream, &request),
        ("POST", "/api/model-key") => handle_model_key(stream, &request),
        ("POST", "/api/provider-key") => handle_provider_key(stream, &request),
        ("POST", "/api/model-key-delete") => handle_model_key_delete(stream, &request),
        ("POST", "/api/mcp-test") => handle_mcp_test(stream, &request),
        ("GET", "/api/workers-enabled")
        | ("GET", "/api/worker-source-status")
        | ("POST", "/api/workers")
        | ("GET", "/api/workers")
        | ("GET", "/api/worker-progress")
        | ("POST", "/api/worker-control")
        | ("POST", "/api/worker-restart")
        | ("POST", "/api/worker-remove-worktree") => handle_worker_route(stream, &request),
        _ => write_response(
            stream,
            &request,
            404,
            "application/json",
            &json_error("not found"),
        ),
    }
}

fn handle_config(stream: &mut TcpStream, request: &Request) -> Result<(), String> {
    let repo = request.param("repo");
    let config = Config::load_for_repository(repo.as_deref().map(Path::new))
        .map_err(|error| error.to_string())?;
    write_response(
        stream,
        request,
        200,
        "application/json",
        &format!(
            "{{\"policy\":\"{}\"}}",
            escape_json(&config.to_policy_text())
        ),
    )
}

fn handle_get_config_file(stream: &mut TcpStream, request: &Request) -> Result<(), String> {
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
        request,
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

fn handle_model_key_status(stream: &mut TcpStream, request: &Request) -> Result<(), String> {
    let repo = request.param("repo").unwrap_or_default();
    let model_provider = request.param("model_provider");
    write_response(
        stream,
        request,
        200,
        "application/json",
        &model_key_status_json(&repo, model_provider.as_deref())?,
    )
}

fn handle_terminal_cwd(stream: &mut TcpStream, request: &Request) -> Result<(), String> {
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

fn handle_terminal_run(stream: &mut TcpStream, request: &Request) -> Result<(), String> {
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

fn handle_session_export(stream: &mut TcpStream, request: &Request) -> Result<(), String> {
    let session_id = required_param(request, "session_id")?;
    let format = request
        .param("format")
        .as_deref()
        .map_or(ExportFormat::Markdown, |value| match value {
            "json" => ExportFormat::Json,
            _ => ExportFormat::Markdown,
        });
    let engine = default_engine()?;
    let content = engine
        .session_store
        .export_session(&session_id, format, &engine.scanner)
        .map_err(|error| error.to_string())?;
    let (content_type, extension) = match format {
        ExportFormat::Markdown => ("text/markdown; charset=utf-8", "md"),
        ExportFormat::Json => ("application/json", "json"),
    };
    // The browser's save dialog is the path the user chose: the export
    // writes nowhere itself and makes no network request.
    let filename = export_filename(&engine, &session_id, extension)?;
    let disposition = format!("content-disposition: attachment; filename=\"{filename}\"");
    write_response_with_extra_headers(stream, request, 200, content_type, &content, &disposition)
}

fn handle_recovery(stream: &mut TcpStream, request: &Request) -> Result<(), String> {
    let payload = recovery::sweep_json()?;
    write_response(stream, request, 200, "application/json", &payload)
}

fn handle_recovery_decision(stream: &mut TcpStream, request: &Request) -> Result<(), String> {
    let form = parse_form(&request.body);
    let payload = recovery::decide(&form)?;
    write_response(stream, request, 200, "application/json", &payload)
}

fn handle_recovery_approval_resolved(
    stream: &mut TcpStream,
    request: &Request,
) -> Result<(), String> {
    let form = parse_form(&request.body);
    let payload = recovery::resolve_reattached_approval(&form)?;
    write_response(stream, request, 200, "application/json", &payload)
}

fn handle_session(stream: &mut TcpStream, request: &Request) -> Result<(), String> {
    let session_id = required_param(request, "session_id")?;
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
        .read_messages_with_seq(&session_id)
        .map_err(|error| error.to_string())?;
    // A question's options are in its `question_asked` event, not on the
    // transcript message, and a reopened card is answerable only with them.
    // Read once per question so `messages_json` stays a formatter.
    let mut question_states = HashMap::new();
    for (_, message) in &messages {
        if message.role != "question" {
            continue;
        }
        if let Some(state) = engine
            .session_store
            .question_state(&session_id, &message.id)
            .map_err(|error| error.to_string())?
        {
            question_states.insert(message.id.clone(), state);
        }
    }
    // Message roles alone cannot say whether a turn was stopped, so the
    // task statuses ride along and the UI joins them by `taskId`.
    let task_statuses = engine
        .session_store
        .read_task_statuses(&session_id)
        .map_err(|error| error.to_string())?;
    // What each turn spent, joined by the same `taskId` (spec 19 §5.6).
    let task_usage = engine
        .session_store
        .read_task_usage(&session_id)
        .map_err(|error| error.to_string())?;
    // The plan each turn worked through, joined by the same `taskId`
    // (spec 21 §5.6). Without this the panel — and with it the
    // completion report — would live only for the turn that produced
    // it, so reopening a session would lose the record of what a plan
    // came to, which is the part a user comes back for.
    let task_plans = engine
        .session_store
        .read_session_plans(&session_id)
        .map_err(|error| error.to_string())?;
    // A named failure reason, joined by `taskId` (spec 48 §5.4), so a
    // refused turn says *why* it failed rather than only that it did.
    let task_failure_kinds = engine
        .session_store
        .read_task_failure_kinds(&session_id)
        .map_err(|error| error.to_string())?;
    // Each turn's browser diagnostics, joined by `taskId` (spec 12
    // `context.md` §3.3). Already redacted when recorded. Without
    // this the card would exist only for the turn that ran it, and a
    // reload would fall back to bare thumbnails.
    let task_web_diagnostics = engine
        .session_store
        .read_session_web_diagnostics(&session_id)
        .map_err(|error| error.to_string())?;
    // Each turn's final notes, joined by `taskId` (spec 65 §5.8). Already
    // redacted when recorded.
    let task_notes = engine
        .session_store
        .read_turn_notes(&session_id)
        .map_err(|error| error.to_string())?;
    write_response(
        stream,
        request,
        200,
        "application/json",
        &format!(
            "{{\"session\":{},\"messages\":[{}],\"tasks\":[{}]}}",
            session_json(&session, engine.session_store.session_mode(&session.id)),
            messages_json(&messages, &question_states),
            task_states_json(
                &task_statuses,
                &task_usage,
                &task_plans,
                &task_failure_kinds,
                &task_web_diagnostics,
                &task_notes,
                &engine.config
            )
        ),
    )
}

fn handle_session_rename(stream: &mut TcpStream, request: &Request) -> Result<(), String> {
    let form = parse_form(&request.body);
    let session_id = required_form(&form, "session_id")?;
    let title = required_form(&form, "title")?;
    let engine = default_engine()?;
    let session = engine
        .session_store
        .rename_session(&session_id, &title)
        .map_err(|error| error.to_string())?;
    let mode = engine.session_store.session_mode(&session.id);
    write_response(
        stream,
        request,
        200,
        "application/json",
        &format!("{{\"session\":{}}}", session_json(&session, mode)),
    )
}

fn handle_session_mode(stream: &mut TcpStream, request: &Request) -> Result<(), String> {
    let form = parse_form(&request.body);
    let session_id = required_form(&form, "session_id")?;
    let requested = required_form(&form, "mode")?;
    // Rejected, never defaulted: falling back to Code would turn a
    // malformed request into the most permissive mode there is.
    let mode = SessionMode::parse(&requested).ok_or_else(|| {
        format!("Unknown mode: {requested}. Expected ask, plan, code, or review.")
    })?;
    let engine = default_engine()?;
    if engine
        .session_store
        .read_session(&session_id)
        .map_err(|error| error.to_string())?
        .is_none()
    {
        return Err(format!("Unknown session: {session_id}"));
    }
    // Always "user": this endpoint is the user's own selection, and
    // nothing the model emits reaches it (spec 20 requirement 4).
    engine
        .session_store
        .set_session_mode(&session_id, mode, "user")
        .map_err(|error| error.to_string())?;
    let Some(session) = engine
        .session_store
        .read_session(&session_id)
        .map_err(|error| error.to_string())?
    else {
        return Err(format!("Unknown session: {session_id}"));
    };
    let mode = engine.session_store.session_mode(&session.id);
    write_response(
        stream,
        request,
        200,
        "application/json",
        &format!("{{\"session\":{}}}", session_json(&session, mode)),
    )
}

/// Spec 57's routes. Each answers from files or one short socket exchange
/// with a worker this shell started (`workers.rs`), and every one but
/// `GET /api/workers` and `GET /api/workers-enabled` refuses while
/// `enable_worker_sessions` is off.
fn handle_worker_route(stream: &mut TcpStream, request: &Request) -> Result<(), String> {
    let config = Config::load_for_repository(None).map_err(|error| error.to_string())?;
    let form = parse_form(&request.body);
    let answer = workers::route(
        workers::global(),
        &config,
        &|config: &Config| resolve_shell_model_api_key(config),
        &workers::WorkerRequest {
            method: &request.method,
            path: &request.path,
            query: &request.query,
            form: &form,
        },
    );
    match answer {
        Ok(body) => write_response(stream, request, 200, "application/json", &body),
        Err(refusal) => write_response(
            stream,
            request,
            refusal.status,
            "application/json",
            &json_error(&refusal.message),
        ),
    }
}

fn handle_session_delete(stream: &mut TcpStream, request: &Request) -> Result<(), String> {
    let form = parse_form(&request.body);
    let session_id = required_form(&form, "session_id")?;
    // A `session_end` hook runs in the repository, and a session stores only
    // its repository id, so the caller says which checkout this session
    // belongs to (spec 32 Task 6).
    let repo = required_form(&form, "repo")?;
    // Loaded for the repository, not with `default_engine`: a repository hook
    // the user has reviewed should see `session_end` as a user hook does, and
    // only a repository-scoped load carries it.
    let engine = engine_for_repo(&repo)?;
    // Before the delete: the children are recorded in the log it removes.
    for worker in workers::workers_to_stop_for_deleted_session(&engine.session_store, &session_id) {
        workers::global().shutdown(&worker);
    }
    engine
        .delete_session(&session_id, &repo)
        .map_err(|error| error.to_string())?;
    write_response(
        stream,
        request,
        200,
        "application/json",
        &format!(
            "{{\"sessionId\":\"{}\",\"status\":\"deleted\"}}",
            escape_json(&session_id)
        ),
    )
}

fn handle_open_vscode(stream: &mut TcpStream, request: &Request) -> Result<(), String> {
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

fn handle_reveal_in_finder(stream: &mut TcpStream, request: &Request) -> Result<(), String> {
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

fn handle_open_github(stream: &mut TcpStream, request: &Request) -> Result<(), String> {
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

fn handle_reveal_web_diagnostic_artifact(
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

fn handle_open_vscode_file(stream: &mut TcpStream, request: &Request) -> Result<(), String> {
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

fn handle_render_markdown(stream: &mut TcpStream, request: &Request) -> Result<(), String> {
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

fn handle_ask(stream: &mut TcpStream, request: &Request) -> Result<(), String> {
    let form = parse_form(&request.body);
    // The non-streaming fallback: there is no stream for a client to
    // abort, so nothing here can be stopped. The events go nowhere.
    let (events, _discard) = std::sync::mpsc::channel();
    let result = run_chat_request(&form, &CancelToken::new(), &events)?;
    write_response(
        stream,
        request,
        200,
        "application/json",
        &chat_result_json(&result),
    )
}

fn handle_repository_config_review(
    stream: &mut TcpStream,
    request: &Request,
) -> Result<(), String> {
    let repo = request.param("repo").unwrap_or_default();
    write_response(
        stream,
        request,
        200,
        "application/json",
        &repository_config_review_json(&repo)?,
    )
}

fn handle_repository_config_allowlist(
    stream: &mut TcpStream,
    request: &Request,
) -> Result<(), String> {
    let form = parse_form(&request.body);
    let repo = required_form(&form, "repo")?;
    // Pipe-separated, matching how `command_allowlist` is written on
    // disk — which is also why an entry can never contain a pipe.
    let keep = form
        .get("keep")
        .map(String::as_str)
        .unwrap_or_default()
        .split('|')
        .map(str::trim)
        .filter(|entry| !entry.is_empty())
        .map(str::to_string)
        .collect::<Vec<_>>();
    write_response(
        stream,
        request,
        200,
        "application/json",
        &resolve_repository_allowlist_migration(&repo, &keep)?,
    )
}

/// Every configured hook for `repo`, with its activation, so Settings can list
/// and act on them. `repo` is the only input: there is no path parameter that
/// could name a config file.
fn hooks_json(repo: &str) -> Result<String, String> {
    if repo.trim().is_empty() {
        return Ok("{\"repositoryPath\":\"\",\"hooks\":[]}".to_string());
    }
    let config = config_for_repo(repo)?;
    let hooks = list_hooks(&config, Path::new(repo));
    serde_json::to_string(&serde_json::json!({
        "repositoryPath": repo,
        "hooks": hooks,
    }))
    .map_err(|error| error.to_string())
}

fn handle_hooks(stream: &mut TcpStream, request: &Request) -> Result<(), String> {
    let repo = request.param("repo").unwrap_or_default();
    write_response(
        stream,
        request,
        200,
        "application/json",
        &hooks_json(&repo)?,
    )
}

fn handle_hook_review(stream: &mut TcpStream, request: &Request) -> Result<(), String> {
    let form = parse_form(&request.body);
    let repo = required_form(&form, "repo")?;
    let hook = required_form(&form, "hook")?;
    let decision = match required_form(&form, "decision")?.as_str() {
        "approve" => HookReviewDecision::Approve {
            hash: required_form(&form, "hash")?,
        },
        "decline" => HookReviewDecision::Decline,
        other => {
            return Err(format!(
                "A hook review decision is approve or decline, not {other}"
            ));
        }
    };
    let config = config_for_repo(&repo)?;
    let path = review_hook(&config, Path::new(&repo), &hook, decision)
        .map_err(|error| error.to_string())?;
    // Re-read so the response carries the review that was just written.
    let config = config_for_repo(&repo)?;
    write_response(
        stream,
        request,
        200,
        "application/json",
        &serde_json::to_string(&serde_json::json!({
            "path": path.to_string_lossy(),
            "hooks": list_hooks(&config, Path::new(&repo)),
        }))
        .map_err(|error| error.to_string())?,
    )
}

fn handle_hook_enabled(stream: &mut TcpStream, request: &Request) -> Result<(), String> {
    let form = parse_form(&request.body);
    let repo = required_form(&form, "repo")?;
    let hook = required_form(&form, "hook")?;
    let enabled = match required_form(&form, "enabled")?.as_str() {
        "true" | "1" => true,
        "false" | "0" => false,
        other => return Err(format!("A hook toggle is true or false, not {other}")),
    };
    let config = config_for_repo(&repo)?;
    let path = set_hook_enabled(&config, &hook, enabled).map_err(|error| error.to_string())?;
    let config = config_for_repo(&repo)?;
    write_response(
        stream,
        request,
        200,
        "application/json",
        &serde_json::to_string(&serde_json::json!({
            "path": path.to_string_lossy(),
            "hooks": list_hooks(&config, Path::new(&repo)),
        }))
        .map_err(|error| error.to_string())?,
    )
}

fn handle_hook_test(stream: &mut TcpStream, request: &Request) -> Result<(), String> {
    let form = parse_form(&request.body);
    let repo = required_form(&form, "repo")?;
    let hook_id = required_form(&form, "hook")?;
    let event_name = required_form(&form, "event")?;
    let event =
        HookEvent::parse(&event_name).ok_or_else(|| format!("Unknown hook event: {event_name}"))?;
    let config = config_for_repo(&repo)?;
    let hook = config
        .hook(&hook_id)
        .cloned()
        .ok_or_else(|| format!("Hook `{hook_id}` is not configured"))?;
    let run =
        test_run(&config, &hook, event, Path::new(&repo)).map_err(|error| error.to_string())?;
    write_response(
        stream,
        request,
        200,
        "application/json",
        &serde_json::to_string(&serde_json::json!({
            "id": hook_id,
            "event": event.as_str(),
            "verdict": run.verdict.as_str(),
            "failure": run.failure.map(|kind| kind.as_str()),
            "stderr": run.stderr,
            "durationMs": run.duration_ms,
        }))
        .map_err(|error| error.to_string())?,
    )
}

fn handle_config_set(stream: &mut TcpStream, request: &Request) -> Result<(), String> {
    let form = parse_form(&request.body);
    let scope = form.get("scope").map(String::as_str);
    let key = required_form(&form, "key")?;
    let value = required_form(&form, "value")?;
    let path = desktop_settings_config_path(scope)?;
    let path = update_config_overlay(path, &key, &value)?;
    write_response(
        stream,
        request,
        200,
        "application/json",
        &format!("{{\"path\":\"{}\"}}", escape_json(&path.to_string_lossy())),
    )
}

fn handle_post_config_file(stream: &mut TcpStream, request: &Request) -> Result<(), String> {
    let form = parse_form(&request.body);
    let scope = form.get("scope").map(String::as_str);
    let repo = form.get("repo").cloned().unwrap_or_default();
    let content = form.get("content").cloned().unwrap_or_default();
    let path = desktop_settings_config_path(scope)?;
    save_config_file(&path, &content)?;
    let (effective_policy, effective_error) = effective_policy_for_repo(&repo);
    write_response(
        stream,
        request,
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

fn handle_model_key(stream: &mut TcpStream, request: &Request) -> Result<(), String> {
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
        request,
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

fn handle_provider_key(stream: &mut TcpStream, request: &Request) -> Result<(), String> {
    let form = parse_form(&request.body);
    let account = required_form(&form, "account")?;
    let api_key = required_form(&form, "api_key")?;
    let reference = keychain::reference_for_account(&account)?;
    keychain::write_password(&account, &api_key)?;
    remember_model_api_key(&account, &api_key);
    write_response(
        stream,
        request,
        200,
        "application/json",
        &format!(
            "{{\"reference\":\"{}\",\"account\":\"{}\",\"configured\":true}}",
            escape_json(&reference),
            escape_json(account.trim())
        ),
    )
}

fn handle_model_key_delete(stream: &mut TcpStream, request: &Request) -> Result<(), String> {
    let form = parse_form(&request.body);
    let account = required_form(&form, "account")?;
    let deleted = keychain::delete_password(&account)?;
    forget_model_api_key(&account);
    write_response(
        stream,
        request,
        200,
        "application/json",
        &format!(
            "{{\"account\":\"{}\",\"deleted\":{},\"configured\":false}}",
            escape_json(account.trim()),
            deleted
        ),
    )
}

fn handle_mcp_test(stream: &mut TcpStream, request: &Request) -> Result<(), String> {
    let form = parse_form(&request.body);
    let config = Config::load_for_repository(None).map_err(|error| error.to_string())?;
    let body = match mcp_test_connection(&form, &config.data_dir) {
        Ok(tools) => format!(
            "{{\"ok\":true,\"toolCount\":{},\"tools\":[{}]}}",
            tools.len(),
            json_string_array(&tools)
        ),
        Err(error) => {
            format!("{{\"ok\":false,\"error\":\"{}\"}}", escape_json(&error))
        }
    };
    write_response(stream, request, 200, "application/json", &body)
}

fn handle_web_diagnostic_artifact(stream: &mut TcpStream, request: &Request) -> Result<(), String> {
    let repo = request.param("repo").unwrap_or_default();
    let relative_path = request
        .param("path")
        .ok_or_else(|| "path is required".to_string())?;
    let engine = engine_for_repo(&repo)?;
    let path = web_diagnostic_artifact_path(&engine.config, &relative_path)?;
    let bytes = fs::read(&path).map_err(|error| error.to_string())?;
    write_binary_response(stream, request, 200, content_type_for_path(&path), &bytes)
}

fn handle_git_status(stream: &mut TcpStream, request: &Request) -> Result<(), String> {
    let repo = required_param(request, "repo")?;
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
    // Spec 58 requirement 11: the repo-bar fields follow the original three,
    // which stay byte-identical. A non-git folder's summary is the default,
    // so it serves `null`/0 here rather than an error.
    let summary = engine
        .git
        .summary(&repo)
        .map_err(|error| error.to_string())?;
    let github_url = summary.github.as_ref().map(|github| github.url());
    write_response(
        stream,
        request,
        200,
        "application/json",
        &format!(
            "{{\"clean\":{},\"exitCode\":{},\"files\":[{}],\"branch\":{},\"head\":{},\"insertions\":{},\"deletions\":{},\"githubUrl\":{}}}",
            status.clean,
            status.exit_code,
            files,
            json_nullable_str(summary.branch.as_deref()),
            json_nullable_str(summary.head.as_deref()),
            summary.insertions,
            summary.deletions,
            json_nullable_str(github_url.as_deref()),
        ),
    )
}

fn handle_commit_status(stream: &mut TcpStream, request: &Request) -> Result<(), String> {
    let repo = required_param(request, "repo")?;
    let session = required_param(request, "session")?;
    let engine = engine_for_repo(&repo)?;
    let status = engine
        .commit
        .status(&repo, &session)
        .map_err(|error| error.to_string())?;
    let body = serde_json::to_string(&status).map_err(|error| error.to_string())?;
    write_response(stream, request, 200, "application/json", &body)
}

fn handle_commit_preparation(stream: &mut TcpStream, request: &Request) -> Result<(), String> {
    let repo = required_param(request, "repo")?;
    let session = required_param(request, "session")?;
    let engine = engine_for_repo(&repo)?;
    let preparation = engine
        .commit
        .prepare(&repo, &session)
        .map_err(|error| error.to_string())?;
    let body = serde_json::to_string(&preparation).map_err(|error| error.to_string())?;
    write_response(stream, request, 200, "application/json", &body)
}

/// `POST /api/commit`: the typed approval. Its own field, its own audit event
/// and no "always"; a question or a command approval never satisfies it.
fn handle_commit(stream: &mut TcpStream, request: &Request) -> Result<(), String> {
    let form = parse_form(&request.body);
    if form.get("approval").map(String::as_str) != Some("commit") {
        return write_response(
            stream,
            request,
            400,
            "application/json",
            "{\"error\":\"A commit needs its own approval.\",\"code\":\"approval_required\"}",
        );
    }
    let repo = required_form(&form, "repo")?;
    let session_id = required_form(&form, "session_id")?;
    let prepare_id = required_form(&form, "prepare_id")?;
    let message = required_form(&form, "message")?;
    let paths = form
        .get("paths")
        .map(|paths| {
            paths
                .lines()
                .map(str::trim)
                .filter(|path| !path.is_empty())
                .map(str::to_string)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let engine = engine_for_repo(&repo)?;
    let outcome = engine.commit.commit(
        &repo,
        &workspace_engine::CommitRequest {
            session_id,
            prepare_id,
            selected_paths: paths,
            message,
            allow_generated_secrets: form.get("allow_generated_secrets").map(String::as_str)
                == Some("true"),
            approved_by: "desktop_user".to_string(),
        },
        &CancelToken::new(),
    );
    match outcome {
        Ok(outcome) => {
            let body = serde_json::to_string(&outcome).map_err(|error| error.to_string())?;
            write_response(stream, request, 200, "application/json", &body)
        }
        Err(error) => {
            let (status, body) = commit_error_response(&error);
            write_response(stream, request, status, "application/json", &body)
        }
    }
}

fn commit_error_response(error: &CommitError) -> (u16, String) {
    match error {
        CommitError::Refused { code, message } => (
            409,
            format!(
                "{{\"error\":\"{}\",\"code\":\"{}\",\"message\":\"{}\"}}",
                escape_json(message),
                escape_json(code),
                escape_json(message),
            ),
        ),
        CommitError::HookFailed {
            name,
            exit_code,
            termination,
            stdout,
            stderr,
        } => (
            409,
            format!(
                "{{\"error\":\"{}\",\"code\":\"commit_hook_failed\",\"hook\":\"{}\",\"exitCode\":{},\"termination\":\"{}\",\"stdout\":\"{}\",\"stderr\":\"{}\"}}",
                escape_json(&error.message()),
                escape_json(name),
                exit_code
                    .map(|code| code.to_string())
                    .unwrap_or_else(|| "null".to_string()),
                escape_json(termination),
                escape_json(stdout),
                escape_json(stderr),
            ),
        ),
        CommitError::MessageRewritten { message } => (
            409,
            format!(
                "{{\"error\":\"A commit-message hook rewrote the message. Review it and commit again.\",\"code\":\"commit_message_rewritten\",\"message\":\"{}\"}}",
                escape_json(message),
            ),
        ),
        CommitError::Engine(message) => (
            500,
            format!(
                "{{\"error\":\"{}\",\"code\":\"commit_engine_error\"}}",
                escape_json(message),
            ),
        ),
    }
}

/// `POST /api/commit-message`: a model-drafted commit message, with the
/// deterministic template as the answer whenever the model is missing, refuses,
/// returns nothing, or returns something that does not parse.
///
/// The diff in the record is already redacted (spec 35 §5.6): this is the one
/// place redaction is exactly right, because the model is drafting prose about
/// the change, and the secret check at commit covers the gap between the
/// redacted preview and the real bytes.
fn handle_commit_message(stream: &mut TcpStream, request: &Request) -> Result<(), String> {
    let form = parse_form(&request.body);
    let repo = required_form(&form, "repo")?;
    let session_id = required_form(&form, "session_id")?;
    let prepare_id = required_form(&form, "prepare_id")?;
    let engine = engine_for_repo(&repo)?;
    let record = engine
        .commit
        .load_record(&prepare_id)
        .map_err(|error| error.to_string())?;
    if record.session_id != session_id {
        return Err("That commit preparation belongs to a different session.".to_string());
    }
    let selected = form
        .get("paths")
        .map(|paths| {
            paths
                .lines()
                .map(str::trim)
                .filter(|path| !path.is_empty())
                .map(str::to_string)
                .collect::<Vec<_>>()
        })
        .unwrap_or_else(|| {
            record
                .candidates
                .iter()
                .filter(|candidate| candidate.selected_by_default)
                .map(|candidate| candidate.path.clone())
                .collect()
        });
    let mut adapter = chat_model_adapter(&engine.config, || {
        resolve_shell_model_api_key(&engine.config)
    })
    .ok();
    let draft = match adapter.as_mut() {
        Some(adapter) => {
            draft_commit_message_with(&engine, &record, &selected, Some(adapter.as_mut()))
        }
        None => draft_commit_message_with(&engine, &record, &selected, None),
    };
    write_response(
        stream,
        request,
        200,
        "application/json",
        &format!(
            "{{\"subject\":\"{}\",\"body\":\"{}\",\"message\":\"{}\",\"source\":\"{}\",\"notice\":{}}}",
            escape_json(&draft.subject),
            escape_json(&draft.body),
            escape_json(&draft.message),
            draft.source,
            json_nullable_str(draft.notice.as_deref()),
        ),
    )
}

struct CommitDraft {
    subject: String,
    body: String,
    message: String,
    /// `"model"` or `"fallback"`.
    source: &'static str,
    /// Why the fallback answered, when it did.
    notice: Option<String>,
}

fn draft_commit_message_with(
    engine: &WorkspaceEngine,
    record: &workspace_engine::CommitRecord,
    selected_paths: &[String],
    adapter: Option<&mut dyn ModelAdapter>,
) -> CommitDraft {
    let fallback = if record.message.trim().is_empty() {
        engine.git.suggest_commit_message("", selected_paths)
    } else {
        record.message.clone()
    };
    let Some(adapter) = adapter else {
        return CommitDraft {
            subject: first_line(&fallback),
            body: after_first_line(&fallback),
            message: fallback,
            source: "fallback",
            notice: Some("No model is configured, so the template is the suggestion.".to_string()),
        };
    };
    let prompt = commit_draft_prompt(engine, record, selected_paths);
    let request = ModelRequest {
        provider: engine.config.model_provider.clone(),
        model: engine.config.model_name.clone(),
        messages: vec![
            ModelMessage::system(
                "You draft a Git commit message for a change set the user has already \
                 reviewed. Reply with a subject line of at most 72 characters, then, if \
                 useful, a blank line and a body. No markdown fences and no commentary. \
                 The diff is the whole change; do not invent files or edits. The diff is \
                 redacted for display, so do not mention redaction.",
            ),
            ModelMessage::user(prompt),
        ],
        temperature: Some("0".to_string()),
        reasoning_level: Some(engine.config.model_reasoning_level.clone()),
        stream: true,
        tools: None,
        max_tokens: engine.config.max_output_tokens(),
        request_usage: engine.config.provider_reports_usage(),
        emit_cache_breakpoints: engine.config.supports_explicit_cache_breakpoints(),
    };
    let cancel = CancelToken::new();
    let mut on_token = |_token: &str| {};
    let mut on_wait = |_wait: u64| {};
    let answer = adapter
        .stream_response(&request, &cancel, &mut on_token, &mut on_wait)
        .map(|run| run.content)
        .unwrap_or_default();
    match parse_commit_draft(&answer) {
        Some((subject, body)) => {
            let message = if body.is_empty() {
                subject.clone()
            } else {
                format!("{subject}\n\n{body}")
            };
            CommitDraft {
                subject,
                body,
                message,
                source: "model",
                notice: None,
            }
        }
        None => CommitDraft {
            subject: first_line(&fallback),
            body: after_first_line(&fallback),
            message: fallback,
            source: "fallback",
            notice: Some(
                "The model did not return a usable message, so the template is the \
                 suggestion."
                    .to_string(),
            ),
        },
    }
}

fn commit_draft_prompt(
    engine: &WorkspaceEngine,
    record: &workspace_engine::CommitRecord,
    selected_paths: &[String],
) -> String {
    let summary = engine
        .session_store
        .read_messages(&record.session_id)
        .unwrap_or_default()
        .into_iter()
        .rev()
        .find(|message| message.role == "user" && message.kind.is_none())
        .map(|message| message.content)
        .unwrap_or_default();
    let plans = engine
        .session_store
        .read_session_plans(&record.session_id)
        .unwrap_or_default();
    let plan = plans
        .into_values()
        .max_by_key(|plan| plan.created_at_ms)
        .map(|plan| {
            plan.steps
                .iter()
                .map(|step| format!("- {} ({:?})", step.title, step.status))
                .collect::<Vec<_>>()
                .join("\n")
        })
        .unwrap_or_default();
    let mut diff = String::new();
    for path in selected_paths {
        let Some(candidate) = record
            .candidates
            .iter()
            .find(|candidate| &candidate.path == path)
        else {
            continue;
        };
        diff.push_str(&format!("\n### {path}\n{}", candidate.diff));
    }
    if diff.chars().count() > 20_000 {
        diff = diff.chars().take(20_000).collect::<String>();
        diff.push_str("\n[Diff truncated for the draft.]\n");
    }
    format!(
        "Task: {summary}\n\nPlan steps:\n{}\n\nFiles:\n{}\n\nRedacted diff:\n{diff}",
        if plan.is_empty() { "- none" } else { &plan },
        selected_paths.join("\n"),
    )
}

/// Split a model answer into a subject and a body. Fences, a `Subject:` prefix
/// and a `Body:` prefix are tolerated because a provider that ignores the
/// instruction should cost a worse message, never a failed commit.
fn parse_commit_draft(answer: &str) -> Option<(String, String)> {
    let mut lines = answer.trim().lines().collect::<Vec<_>>();
    if lines
        .first()
        .is_some_and(|line| line.trim_start().starts_with("```"))
    {
        lines.remove(0);
    }
    if lines.last().is_some_and(|line| line.trim() == "```") {
        lines.pop();
    }
    let index = lines.iter().position(|line| !line.trim().is_empty())?;
    let subject = lines[index]
        .trim()
        .trim_start_matches("Subject:")
        .trim()
        .chars()
        .take(200)
        .collect::<String>();
    if subject.is_empty() {
        return None;
    }
    let body = lines[index + 1..]
        .join("\n")
        .trim()
        .trim_start_matches("Body:")
        .trim()
        .to_string();
    Some((subject, body))
}

fn first_line(message: &str) -> String {
    message
        .lines()
        .next()
        .unwrap_or_default()
        .trim()
        .to_string()
}

fn after_first_line(message: &str) -> String {
    message
        .split_once('\n')
        .map(|(_, rest)| rest.trim().to_string())
        .unwrap_or_default()
}

fn json_nullable_str(value: Option<&str>) -> String {
    match value {
        Some(value) => format!("\"{}\"", escape_json(value)),
        None => "null".to_string(),
    }
}

fn handle_sessions(stream: &mut TcpStream, request: &Request) -> Result<(), String> {
    let repo = required_param(request, "repo")?;
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
        request,
        200,
        "application/json",
        &format!("{{\"sessions\":[{}]}}", sessions_json(&sessions)),
    )
}

fn handle_session_search(stream: &mut TcpStream, request: &Request) -> Result<(), String> {
    let query = required_param(request, "query")?;
    let whole_word = request.param("whole_word").as_deref() == Some("true");
    let literal_phrase = request.param("literal").as_deref() != Some("false");
    let max_results = request
        .param("max")
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(50);
    let (engine, repository_id) = if request.param("scope").as_deref() == Some("all") {
        (default_engine()?, None)
    } else {
        let repo = required_param(request, "repo")?;
        let engine = engine_for_repo(&repo)?;
        let repository_id = engine
            .indexer
            .repository_id_for_path(&repo)
            .map_err(|error| error.to_string())?;
        (engine, Some(repository_id))
    };
    let result = engine
        .session_store
        .search_sessions(
            repository_id.as_deref(),
            &query,
            SearchOptions {
                whole_word,
                literal_phrase,
                max_results,
            },
            &engine.scanner,
        )
        .map_err(|error| error.to_string())?;
    write_response(
        stream,
        request,
        200,
        "application/json",
        &serde_json::to_string(&result).map_err(|error| error.to_string())?,
    )
}

fn handle_checkpoints(stream: &mut TcpStream, request: &Request) -> Result<(), String> {
    let repo = required_param(request, "repo")?;
    let engine = engine_for_repo(&repo)?;
    let repository_id = engine
        .indexer
        .repository_id_for_path(&repo)
        .map_err(|error| error.to_string())?;
    let mut checkpoints = engine
        .checkpoint_store
        .list_checkpoints(&repository_id)
        .map_err(|error| error.to_string())?;
    if let Some(session_id) = request.param("session_id") {
        checkpoints.retain(|manifest| manifest.session_id == session_id);
    }
    write_response(
        stream,
        request,
        200,
        "application/json",
        &checkpoint_list_json(&checkpoints),
    )
}

fn handle_rewind(stream: &mut TcpStream, request: &Request) -> Result<(), String> {
    let form = parse_form(&request.body);
    let repo = required_form(&form, "repo")?;
    let checkpoint_id = required_form(&form, "checkpoint_id")?;
    let files = form.get("files").map(String::as_str) != Some("false");
    let conversation = form.get("conversation").map(String::as_str) == Some("true");
    let only_path = form
        .get("path")
        .map(String::as_str)
        .filter(|path| !path.is_empty());
    if !files && !conversation {
        return Err("Choose files, conversation, or both".to_string());
    }
    let engine = engine_for_repo(&repo)?;
    let repository_id = engine
        .indexer
        .repository_id_for_path(&repo)
        .map_err(|error| error.to_string())?;
    let manifest = engine
        .checkpoint_store
        .read_checkpoint(&repository_id, &checkpoint_id)
        .map_err(|error| error.to_string())?
        .ok_or_else(|| format!("Unknown checkpoint: {checkpoint_id}"))?;
    let result = engine
        .checkpoint_store
        .restore(
            &repo,
            &manifest,
            workspace_engine::CheckpointRestoreOptions {
                files,
                conversation,
                only_path,
            },
            "desktop_user",
        )
        .map_err(|error| error.to_string())?;
    write_response(
        stream,
        request,
        200,
        "application/json",
        &checkpoint_restore_json(&result),
    )
}

fn handle_session_create(stream: &mut TcpStream, request: &Request) -> Result<(), String> {
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
    let mode = engine.session_store.session_mode(&session.id);
    write_response(
        stream,
        request,
        200,
        "application/json",
        &format!("{{\"session\":{}}}", session_json(&session, mode)),
    )
}

fn handle_context_file(stream: &mut TcpStream, request: &Request) -> Result<(), String> {
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
        request,
        200,
        "application/json",
        &format!("{{\"path\":\"{}\"}}", escape_json(path)),
    )
}

fn handle_propose_edit(stream: &mut TcpStream, request: &Request) -> Result<(), String> {
    let form = parse_form(&request.body);
    let repo = required_form(&form, "repo")?;
    let prompt = required_form(&form, "prompt")?;
    // The chat session the prompt was typed in, so its mode governs
    // this flow too. Absent for a caller with no session open.
    let session_id = form
        .get("session_id")
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());
    let (config, report) = config_for_repo_with_model_options(&repo, &form)?;
    let engine = WorkspaceEngine::new_with_report(config, &report);
    let context_files = form
        .get("context_files")
        .map(|value| validate_context_files(&engine, &repo, value))
        .transpose()?
        .unwrap_or_default();
    let api_key = resolve_shell_model_api_key(&engine.config)?;
    let transport = CurlModelTransport::new(
        &engine.config.model_base_url,
        api_key,
        ProcessRegistry::open(&engine.config.data_dir).map_err(|error| error.to_string())?,
        &engine.config.data_dir,
    );
    let mut adapter = OpenAICompatibleAdapter::with_provider(
        &engine.config.model_provider,
        &engine.config.model_name,
        transport,
    );
    let result = engine
        .edit_orchestrator
        .propose_edit(
            &repo,
            &prompt,
            &context_files,
            session_id.as_deref(),
            &mut adapter,
        )
        .map_err(|error| error.to_string())?;
    write_response(
        stream,
        request,
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

fn handle_apply_patch(stream: &mut TcpStream, request: &Request) -> Result<(), String> {
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
                request,
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
        request,
        200,
        "application/json",
        &format!(
            "{{\"patchId\":\"{}\",\"appliedFiles\":[{}],\"warningCount\":{},\"hookWarnings\":[{}]}}",
            escape_json(&result.patch_id),
            json_string_array(&result.applied_files),
            result.warnings.len(),
            json_string_array(&result.hook_warnings)
        ),
    )
}

fn handle_rollback_patch(stream: &mut TcpStream, request: &Request) -> Result<(), String> {
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
        request,
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

fn handle_reject_patch_files(stream: &mut TcpStream, request: &Request) -> Result<(), String> {
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
        request,
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

fn handle_reject_patch(stream: &mut TcpStream, request: &Request) -> Result<(), String> {
    let form = parse_form(&request.body);
    let patch_id = required_form(&form, "patch_id")?;
    let engine = engine_for_repo(form.get("repo").map(String::as_str).unwrap_or_default())?;
    let path = engine
        .edit_orchestrator
        .reject_stored_patch(&patch_id, "desktop_user")
        .map_err(|error| error.to_string())?;
    write_response(
        stream,
        request,
        200,
        "application/json",
        &format!(
            "{{\"patchId\":\"{}\",\"status\":\"rejected\",\"path\":\"{}\"}}",
            escape_json(&patch_id),
            escape_json(&path.to_string_lossy())
        ),
    )
}

fn handle_propose_command(stream: &mut TcpStream, request: &Request) -> Result<(), String> {
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
        request,
        200,
        "application/json",
        &format!(
            "{{\"proposalId\":\"{}\",\"prompt\":\"{}\",\"risk\":\"{}\",\"requiresApproval\":{},\"blocked\":{},\"cause\":\"{}\",\"reason\":\"{}\",\"allowAlways\":{},\"allowBrowserDiagnosticsForSession\":false}}",
            escape_json(&proposal.id),
            escape_json(&command_approval_prompt(&proposal)),
            proposal.risk.as_str(),
            proposal.requires_approval,
            proposal.blocked,
            escape_json(proposal.reasons.first().map(String::as_str).unwrap_or("")),
            escape_json(&proposal.reason),
            allow_always_eligible(&engine.config, &proposal.command, proposal.blocked)
        ),
    )
}

fn handle_run_command(stream: &mut TcpStream, request: &Request) -> Result<(), String> {
    let form = parse_form(&request.body);
    let proposal_id = required_form(&form, "proposal_id")?;
    let mut engine = engine_for_repo(form.get("repo").map(String::as_str).unwrap_or_default())?;

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
            request,
            200,
            "application/json",
            &chat_result_json(&result),
        )
    } else {
        let record = {
            // A command run from this endpoint has no turn behind it,
            // so it has no stop to honour and no progress to stream.
            let cancel = CancelToken::new();
            let mut on_output = |_line: &str| {};
            engine.validation_orchestrator.run_proposal(
                &proposal_id,
                true,
                "desktop_user",
                None,
                &cancel,
                &mut on_output,
            )
        }
        .map_err(|error| error.to_string())?;
        write_response(
            stream,
            request,
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

fn handle_reject_command(stream: &mut TcpStream, request: &Request) -> Result<(), String> {
    let form = parse_form(&request.body);
    let proposal_id = required_form(&form, "proposal_id")?;
    let mut engine = engine_for_repo(form.get("repo").map(String::as_str).unwrap_or_default())?;

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
            request,
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
            request,
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

fn handle_verify_stream(stream: &mut TcpStream, request: &Request) -> Result<(), String> {
    let form = parse_form(&request.body);
    write_event_stream_headers(stream, request)?;
    stream_turn(stream, move |cancel, events| {
        run_verify_request(&form, cancel, events)
    })
}

fn handle_resume_checks_stream(stream: &mut TcpStream, request: &Request) -> Result<(), String> {
    let form = parse_form(&request.body);
    write_event_stream_headers(stream, request)?;
    stream_turn(stream, move |cancel, events| {
        run_resume_checks_request(&form, cancel, events)
    })
}

fn handle_resume_plan_stream(stream: &mut TcpStream, request: &Request) -> Result<(), String> {
    let form = parse_form(&request.body);
    write_event_stream_headers(stream, request)?;
    stream_turn(stream, move |cancel, events| {
        run_resume_plan_request(&form, cancel, events)
    })
}

fn handle_answer_question_stream(stream: &mut TcpStream, request: &Request) -> Result<(), String> {
    let form = parse_form(&request.body);
    write_event_stream_headers(stream, request)?;
    stream_turn(stream, move |cancel, events| {
        run_answer_question_request(&form, cancel, events)
    })
}

fn handle_resume_fallback_stream(stream: &mut TcpStream, request: &Request) -> Result<(), String> {
    let form = parse_form(&request.body);
    write_event_stream_headers(stream, request)?;
    stream_turn(stream, move |cancel, events| {
        run_resume_fallback_request(&form, cancel, events)
    })
}

fn handle_resume_round_budget_stream(
    stream: &mut TcpStream,
    request: &Request,
) -> Result<(), String> {
    let form = parse_form(&request.body);
    write_event_stream_headers(stream, request)?;
    stream_turn(stream, move |cancel, events| {
        run_resume_round_budget_request(&form, cancel, events)
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
    let engine = chat_engine(config_and_report_for_repo(&repo)?);

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

    let mut adapter = chat_model_adapter(&engine.config, || {
        resolve_shell_model_api_key(&engine.config)
    })?;
    let mut on_token = |token: &str| {
        let _ = events.send(TurnEvent::Token(token.to_string()));
    };
    let mut on_progress = |progress: TurnProgress| {
        if let Some(event) = turn_progress_event(progress) {
            let _ = events.send(event);
        }
    };
    let mut sink = TurnSink {
        on_token: &mut on_token,
        on_progress: &mut on_progress,
        cancel,
        steering: None,
    };
    engine
        .chat_orchestrator
        .resume_after_command_decision_with_options(
            &proposal_id,
            approved,
            "desktop_user",
            adapter.as_mut(),
            &mut sink,
            resume_decision_options(form),
        )
        .map_err(|error| error.to_string())
}

/// Runs the verification a settled patch made due (spec 23 Task 8).
///
/// The engine decides whether anything is due: an all-rejected repair patch
/// still closes its attempt through this route.
fn run_verify_request(
    form: &HashMap<String, String>,
    cancel: &CancelToken,
    events: &std::sync::mpsc::Sender<TurnEvent>,
) -> Result<ChatTurnResult, String> {
    let repo = required_form(form, "repo")?;
    let session_id = required_form(form, "session_id")?;
    let engine = chat_engine(config_and_report_for_repo(&repo)?);
    let mut adapter = chat_model_adapter(&engine.config, || {
        resolve_shell_model_api_key(&engine.config)
    })?;
    let mut on_token = |token: &str| {
        let _ = events.send(TurnEvent::Token(token.to_string()));
    };
    let mut on_progress = |progress: TurnProgress| {
        if let Some(event) = turn_progress_event(progress) {
            let _ = events.send(event);
        }
    };
    let mut sink = TurnSink {
        on_token: &mut on_token,
        on_progress: &mut on_progress,
        cancel,
        steering: None,
    };
    engine
        .chat_orchestrator
        .verify_due(&repo, &session_id, adapter.as_mut(), &mut sink)
        .map_err(|error| error.to_string())
}

/// Answers a check card (spec 23 Task 8).
///
/// `decisions` is a JSON array of `{"proposalId": "...", "approved": true}`,
/// one per check on the card. A missing decision is refused by the engine and
/// the pause is put back, so a stale client can retry.
fn run_resume_checks_request(
    form: &HashMap<String, String>,
    cancel: &CancelToken,
    events: &std::sync::mpsc::Sender<TurnEvent>,
) -> Result<ChatTurnResult, String> {
    let repo = required_form(form, "repo")?;
    let proposal_id = required_form(form, "proposal_id")?;
    let decisions = parse_check_decisions(&required_form(form, "decisions")?)?;
    let engine = chat_engine(config_and_report_for_repo(&repo)?);
    // *Allow Always* is still per check and exact-command: it writes the same
    // repository-qualified allowlist entry the single command card does, before
    // the resumed turn runs anything (spec 23 proposal §5.3).
    for proposal_id in form
        .get("always")
        .map(|raw| raw.split(['\n', ',']).filter(|id| !id.trim().is_empty()))
        .into_iter()
        .flatten()
    {
        engine
            .validation_orchestrator
            .allow_command_always(proposal_id.trim(), "desktop_user")
            .map_err(|error| error.to_string())?;
    }
    let mut adapter = chat_model_adapter(&engine.config, || {
        resolve_shell_model_api_key(&engine.config)
    })?;
    let mut on_token = |token: &str| {
        let _ = events.send(TurnEvent::Token(token.to_string()));
    };
    let mut on_progress = |progress: TurnProgress| {
        if let Some(event) = turn_progress_event(progress) {
            let _ = events.send(event);
        }
    };
    let mut sink = TurnSink {
        on_token: &mut on_token,
        on_progress: &mut on_progress,
        cancel,
        steering: None,
    };
    engine
        .chat_orchestrator
        .resume_after_check_decision(
            &proposal_id,
            &decisions,
            "desktop_user",
            adapter.as_mut(),
            &mut sink,
        )
        .map_err(|error| error.to_string())
}

fn parse_check_decisions(raw: &str) -> Result<Vec<(String, bool)>, String> {
    let value: serde_json::Value =
        serde_json::from_str(raw).map_err(|error| format!("decisions must be JSON: {error}"))?;
    let items = value
        .as_array()
        .ok_or_else(|| "decisions must be a JSON array".to_string())?;
    items
        .iter()
        .map(|item| {
            let proposal_id = item
                .get("proposalId")
                .and_then(serde_json::Value::as_str)
                .filter(|id| !id.is_empty())
                .ok_or_else(|| "each decision needs a proposalId".to_string())?;
            let approved = item
                .get("approved")
                .and_then(serde_json::Value::as_bool)
                .ok_or_else(|| "each decision needs an approved boolean".to_string())?;
            Ok((proposal_id.to_string(), approved))
        })
        .collect()
}

/// Continues a turn paused for plan review (spec 21 §5.5).
///
/// The revision arrives as `steps`: a JSON array of `{id, title}` in the order
/// the user wants them to run. Absent means "approved as proposed" — which is
/// not the same as an empty array, and the difference matters: an empty array
/// is a revision that deletes every step the engine would let it delete.
fn run_resume_plan_request(
    form: &HashMap<String, String>,
    cancel: &CancelToken,
    events: &std::sync::mpsc::Sender<TurnEvent>,
) -> Result<ChatTurnResult, String> {
    let repo = required_form(form, "repo")?;
    let proposal_id = required_form(form, "proposal_id")?;
    let approved = form.get("approved").map(String::as_str) == Some("true");
    let revised = match form.get("steps").filter(|value| !value.is_empty()) {
        Some(raw) => Some(parse_plan_revision(raw)?),
        None => None,
    };
    let engine = chat_engine(config_and_report_for_repo(&repo)?);

    let mut adapter = chat_model_adapter(&engine.config, || {
        resolve_shell_model_api_key(&engine.config)
    })?;
    let mut on_token = |token: &str| {
        let _ = events.send(TurnEvent::Token(token.to_string()));
    };
    let mut on_progress = |progress: TurnProgress| {
        if let Some(event) = turn_progress_event(progress) {
            let _ = events.send(event);
        }
    };
    let mut sink = TurnSink {
        on_token: &mut on_token,
        on_progress: &mut on_progress,
        cancel,
        steering: None,
    };
    engine
        .chat_orchestrator
        .resume_after_plan_decision(
            &proposal_id,
            approved,
            revised,
            "desktop_user",
            adapter.as_mut(),
            &mut sink,
        )
        .map_err(|error| error.to_string())
}

/// Answers the question a turn paused on (spec 50 requirement 1).
///
/// The answer is a tool result and authorises nothing: this route only resumes
/// the paused turn, and every side effect the model asks for afterwards still
/// needs its own approval or review. A command, plan, fallback or round-budget
/// pause is refused here before a model adapter (and so a key) is built — those
/// are decisions with their own routes, and an answer must never stand in for
/// one.
fn run_answer_question_request(
    form: &HashMap<String, String>,
    cancel: &CancelToken,
    events: &std::sync::mpsc::Sender<TurnEvent>,
) -> Result<ChatTurnResult, String> {
    let repo = required_form(form, "repo")?;
    let question_id = required_form(form, "question_id")?;
    let answer = required_form(form, "answer")?;
    let selected_option = match form
        .get("selected_option")
        .map(String::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        Some(raw) => Some(
            raw.parse::<usize>()
                .map_err(|_| "selected_option must be a number".to_string())?,
        ),
        None => None,
    };
    let engine = chat_engine(config_and_report_for_repo(&repo)?);

    // `has_pending_chat_command` is true for every non-question pause. Asking
    // for one of those here is a client error, and checking before the adapter
    // means a refused answer never touches the Keychain.
    if engine
        .chat_orchestrator
        .has_pending_chat_command(&question_id)
    {
        return Err(format!(
            "Proposal {question_id} is not a question; resume it with its own route"
        ));
    }

    let mut adapter = chat_model_adapter(&engine.config, || {
        resolve_model_api_key(&engine.config.model_api_key_env)
    })?;
    let mut on_token = |token: &str| {
        let _ = events.send(TurnEvent::Token(token.to_string()));
    };
    let mut on_progress = |progress: TurnProgress| {
        if let Some(event) = turn_progress_event(progress) {
            let _ = events.send(event);
        }
    };
    let mut sink = TurnSink {
        on_token: &mut on_token,
        on_progress: &mut on_progress,
        cancel,
        steering: None,
    };
    engine
        .chat_orchestrator
        .resume_after_answer(
            &question_id,
            &answer,
            selected_option,
            adapter.as_mut(),
            &mut sink,
        )
        .map_err(|error| error.to_string())
}

/// Answers a provider fallback offer (spec 56).
///
/// The engine that takes an approval must already have the fallback active,
/// because the request's provider, model and token ceiling come from its
/// config (`tasks.md` C1). That config is switched in memory and never written
/// back, which is what "not remembered" means here. There is no `always`: a
/// form field of that name is ignored rather than refused, so no client can
/// turn this into a standing switch.
fn run_resume_fallback_request(
    form: &HashMap<String, String>,
    cancel: &CancelToken,
    events: &std::sync::mpsc::Sender<TurnEvent>,
) -> Result<ChatTurnResult, String> {
    let repo = required_form(form, "repo")?;
    resume_fallback_decision(
        form,
        cancel,
        events,
        &|provider| {
            let (config, report) = config_for_repo_with_provider(&repo, provider)?;
            let mut engine = WorkspaceEngine::new_with_report(config, &report);
            configure_chat_integrations(&mut engine);
            Ok(engine)
        },
        &mut |config| {
            let api_key = resolve_shell_model_api_key(config)?;
            let transport = CurlModelTransport::new(
                &config.model_base_url,
                api_key,
                ProcessRegistry::open(&config.data_dir).map_err(|error| error.to_string())?,
                &config.data_dir,
            );
            Ok(Box::new(OpenAICompatibleAdapter::with_provider(
                &config.model_provider,
                &config.model_name,
                transport,
            )))
        },
    )
}

/// Builds the model adapter for a config, resolving its active provider's key.
type AdapterFor<'a> = dyn FnMut(&Config) -> Result<Box<dyn ModelAdapter>, String> + 'a;

/// The decision itself, with how engines and adapters are built passed in so a
/// test can see that a decline, or an unknown offer, never reaches a key.
///
/// `engine_for(None)` is the repository's configured engine; `engine_for(Some(id))`
/// the same with `id` active. `adapter_for` resolves the key of whichever
/// provider the config it is handed has active.
fn resume_fallback_decision(
    form: &HashMap<String, String>,
    cancel: &CancelToken,
    events: &std::sync::mpsc::Sender<TurnEvent>,
    engine_for: &dyn Fn(Option<&str>) -> Result<WorkspaceEngine, String>,
    adapter_for: &mut AdapterFor<'_>,
) -> Result<ChatTurnResult, String> {
    let proposal_id = required_form(form, "proposal_id")?;
    let approved = form.get("approved").map(String::as_str) == Some("true");
    let configured = engine_for(None)?;
    let target = PausedTurns::new(&configured.config.data_dir)
        .provider_fallback_target(&proposal_id)
        .ok_or_else(|| format!("No pending provider fallback offer for proposal: {proposal_id}"))?;

    let (engine, mut adapter): (WorkspaceEngine, Box<dyn ModelAdapter>) = if approved {
        let engine = engine_for(Some(&target))?;
        let adapter = adapter_for(&engine.config)?;
        (engine, adapter)
    } else {
        // A decline fails the task without a model call, so it resolves no
        // key: the user should not meet a Keychain prompt for a provider they
        // just said no to.
        (configured, Box::new(NoModelCall))
    };

    let mut on_token = |token: &str| {
        let _ = events.send(TurnEvent::Token(token.to_string()));
    };
    let mut on_progress = |progress: TurnProgress| {
        if let Some(event) = turn_progress_event(progress) {
            let _ = events.send(event);
        }
    };
    let mut sink = TurnSink {
        on_token: &mut on_token,
        on_progress: &mut on_progress,
        cancel,
        steering: None,
    };
    engine
        .chat_orchestrator
        .resume_after_fallback_decision(
            &proposal_id,
            approved,
            "desktop_user",
            adapter.as_mut(),
            &mut sink,
        )
        .map_err(|error| error.to_string())
}

/// Continues a turn paused at its round budget (spec 61 §5.6). Unlike the
/// fallback there is no provider switch: the decision is only *Continue* or
/// *Answer now*, and the engine is the repository's configured one.
fn run_resume_round_budget_request(
    form: &HashMap<String, String>,
    cancel: &CancelToken,
    events: &std::sync::mpsc::Sender<TurnEvent>,
) -> Result<ChatTurnResult, String> {
    let repo = required_form(form, "repo")?;
    resume_round_budget_decision(
        form,
        cancel,
        events,
        &|| {
            let engine = chat_engine(config_and_report_for_repo(&repo)?);
            Ok(engine)
        },
        &mut |config| chat_model_adapter(config, || resolve_shell_model_api_key(config)),
    )
}

/// The decision itself, with how the engine and adapter are built passed in so
/// a test can resume a paused turn without touching the user's config or
/// Keychain.
fn resume_round_budget_decision(
    form: &HashMap<String, String>,
    cancel: &CancelToken,
    events: &std::sync::mpsc::Sender<TurnEvent>,
    engine_for: &dyn Fn() -> Result<WorkspaceEngine, String>,
    adapter_for: &mut AdapterFor<'_>,
) -> Result<ChatTurnResult, String> {
    let proposal_id = required_form(form, "proposal_id")?;
    let decision = match form.get("decision").map(String::as_str) {
        Some("continue") => RoundBudgetDecision::Continue,
        Some("answer_now") => RoundBudgetDecision::AnswerNow,
        _ => return Err("decision must be \"continue\" or \"answer_now\"".to_string()),
    };
    let engine = engine_for()?;
    let mut adapter = adapter_for(&engine.config)?;
    let mut on_token = |token: &str| {
        let _ = events.send(TurnEvent::Token(token.to_string()));
    };
    let mut on_progress = |progress: TurnProgress| {
        if let Some(event) = turn_progress_event(progress) {
            let _ = events.send(event);
        }
    };
    let mut sink = TurnSink {
        on_token: &mut on_token,
        on_progress: &mut on_progress,
        cancel,
        steering: None,
    };
    engine
        .chat_orchestrator
        .resume_round_budget(
            &proposal_id,
            decision,
            "desktop_user",
            adapter.as_mut(),
            &mut sink,
        )
        .map_err(|error| error.to_string())
}

/// The adapter handed to a declined fallback offer. The engine fails the task
/// before any model call, so reaching this is a bug, and it says so rather
/// than answering.
struct NoModelCall;

impl ModelAdapter for NoModelCall {
    fn stream_response(
        &mut self,
        _request: &ModelRequest,
        _cancel: &CancelToken,
        _on_token: &mut dyn FnMut(&str),
        _on_wait: &mut dyn FnMut(u64),
    ) -> workspace_engine::Result<ModelRun> {
        Err(workspace_engine::ClientError::InvalidInput(
            "a declined provider fallback must not call a model".to_string(),
        ))
    }
}

fn parse_plan_revision(raw: &str) -> Result<Vec<PlanRevisionStep>, String> {
    let parsed: Vec<serde_json::Value> =
        serde_json::from_str(raw).map_err(|error| format!("Invalid plan revision: {error}"))?;
    parsed
        .into_iter()
        .map(|entry| {
            let id = entry
                .get("id")
                .and_then(|value| value.as_str())
                .ok_or_else(|| "Each revised step needs an id".to_string())?;
            let title = entry
                .get("title")
                .and_then(|value| value.as_str())
                .ok_or_else(|| "Each revised step needs a title".to_string())?;
            Ok(PlanRevisionStep {
                id: id.to_string(),
                title: title.to_string(),
            })
        })
        .collect()
}

/// A send failure only means the relay has gone, and the cancel token is what
/// stops the turn in that case, so the result is deliberately discarded.
/// `None` for progress the shell never causes: it passes no steering, so a
/// turn it hosts never pauses or takes a note (spec 57 §5.4).
fn turn_progress_event(progress: TurnProgress) -> Option<TurnEvent> {
    match progress {
        TurnProgress::Session(session_id) => Some(TurnEvent::Session(session_id)),
        TurnProgress::Phase(phase) => Some(TurnEvent::Phase(phase)),
        TurnProgress::Plan(plan) => Some(TurnEvent::Plan {
            plan: Box::new(plan),
            active: true,
        }),
        TurnProgress::PlanFinished(plan) => Some(TurnEvent::Plan {
            plan: Box::new(plan),
            active: false,
        }),
        TurnProgress::WebDiagnostic(record) => Some(TurnEvent::WebDiagnostic(record)),
        TurnProgress::Paused | TurnProgress::Resumed | TurnProgress::NoteAdded { .. } => None,
    }
}

fn run_chat_request(
    form: &HashMap<String, String>,
    cancel: &CancelToken,
    events: &std::sync::mpsc::Sender<TurnEvent>,
) -> Result<ChatTurnResult, String> {
    run_chat_request_with(form, cancel, events, &mut |config| {
        chat_model_adapter(config, || resolve_shell_model_api_key(config))
    })
}

/// [`run_chat_request`] with how the adapter is built passed in, so a test can
/// script the model and compare this host's turn with a worker's (spec 57
/// requirement 4).
fn run_chat_request_with(
    form: &HashMap<String, String>,
    cancel: &CancelToken,
    events: &std::sync::mpsc::Sender<TurnEvent>,
    adapter_for: &mut AdapterFor<'_>,
) -> Result<ChatTurnResult, String> {
    let repo = required_form(form, "repo")?;
    let session_id = form
        .get("session_id")
        .map(String::as_str)
        .filter(|value| !value.is_empty());
    let engine = chat_engine(config_for_repo_with_model_options(&repo, form)?);
    // The *Ask again* prompt is built by the engine from the stopped task's own
    // request (spec 61 §5.7), not by the client: the original request may have
    // scrolled out of what the client can see, and only `task.user_prompt` still
    // holds it verbatim.
    let prompt = match form
        .get("ask_again_task_id")
        .map(String::as_str)
        .filter(|value| !value.is_empty())
    {
        Some(task_id) => {
            let session = session_id.ok_or_else(|| {
                "ask_again_task_id requires the session the stopped task belongs to".to_string()
            })?;
            engine
                .chat_orchestrator
                .tool_budget_ask_again_prompt(session, task_id)
                .map_err(|error| error.to_string())?
        }
        None => required_form(form, "prompt")?,
    };
    let context_files = form
        .get("context_files")
        .map(|value| validate_context_files(&engine, &repo, value))
        .transpose()?
        .unwrap_or_default();

    let mut adapter = adapter_for(&engine.config)?;
    let mut on_token = |token: &str| {
        let _ = events.send(TurnEvent::Token(token.to_string()));
    };
    let mut on_progress = |progress: TurnProgress| {
        if let Some(event) = turn_progress_event(progress) {
            let _ = events.send(event);
        }
    };
    let mut sink = TurnSink {
        on_token: &mut on_token,
        on_progress: &mut on_progress,
        cancel,
        steering: None,
    };
    let turn_options = ChatTurnOptions {
        // The desktop app is the one executor with a user present: the question
        // card and `/api/answer-question-stream` deliver the answer. Every
        // other driver leaves this false and is never offered `ask_user`.
        user_present: true,
        ..ChatTurnOptions::default()
    };
    engine
        .chat_orchestrator
        .ask_with_session_with_options(
            &repo,
            &prompt,
            &context_files,
            session_id,
            adapter.as_mut(),
            &mut sink,
            turn_options,
        )
        .map_err(|error| error.to_string())
}

/// The engine a chat turn runs on. The shell's turns and a worker's both build
/// it here (spec 57 §5.2, "same turn"), so neither host can configure a turn
/// the other would not.
fn chat_engine(config_and_report: (Config, RepositoryConfigReport)) -> WorkspaceEngine {
    let (config, report) = config_and_report;
    let mut engine = WorkspaceEngine::new_with_report(config, &report);
    configure_chat_integrations(&mut engine);
    engine
}

/// The model adapter a chat turn calls, for both hosts. `api_key` is asked only
/// when a real adapter is built: the shell resolves it, and a worker hands over
/// the key it was given on stdin, so a worker never reaches the Keychain.
///
/// `DAMAIAN_MOCK_MODEL_RESPONSE` answers every model call with its text, as it
/// does for the CLI, and then no key is asked for at all.
fn chat_model_adapter(
    config: &Config,
    api_key: impl FnOnce() -> Result<String, String>,
) -> Result<Box<dyn ModelAdapter>, String> {
    if let Ok(response) = env::var("DAMAIAN_MOCK_MODEL_RESPONSE") {
        return Ok(Box::new(workspace_engine::MockModelAdapter::new(response)));
    }
    let transport = CurlModelTransport::new(
        &config.model_base_url,
        api_key()?,
        ProcessRegistry::open(&config.data_dir).map_err(|error| error.to_string())?,
        &config.data_dir,
    );
    Ok(Box::new(OpenAICompatibleAdapter::with_provider(
        &config.model_provider,
        &config.model_name,
        transport,
    )))
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
    let api_key = resolve_shell_model_api_key(&engine.config)?;
    let transport = CurlModelTransport::new(
        &engine.config.model_base_url,
        api_key,
        ProcessRegistry::open(&engine.config.data_dir).map_err(|error| error.to_string())?,
        &engine.config.data_dir,
    );
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
        steering: None,
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
    let (config, report) =
        Config::load_for_repository_reporting(None).map_err(|error| error.to_string())?;
    Ok(WorkspaceEngine::new_with_report(config, &report))
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

/// `GET /api/effective-policy`: spec 31 Task 6's `EffectivePolicy`, which
/// Settings › General draws as a table. With a session, the header shows the
/// profile intersected with that session's mode (proposal §5.6).
fn handle_effective_policy(stream: &mut TcpStream, request: &Request) -> Result<(), String> {
    let repo = request.param("repo").unwrap_or_default();
    let session_id = request.param("session").unwrap_or_default();
    let body = effective_policy_json(&repo, &session_id)?;
    write_response(stream, request, 200, "application/json", &body)
}

/// `POST /api/permission-profile`: the steps `damaian profile-set` runs.
/// Config is loaded without the repository, so a checkout whose custom profile
/// file has gone missing can still be switched away from it (spec 31 Task 3,
/// deviation 2).
fn handle_permission_profile(stream: &mut TcpStream, request: &Request) -> Result<(), String> {
    let form = parse_form(&request.body);
    let repo = required_form(&form, "repo")?;
    let requested = required_form(&form, "profile")?;
    let session_id = form.get("session").cloned().unwrap_or_default();
    if !Path::new(&repo).is_dir() {
        return Err(format!("Not a repository folder: {repo}"));
    }
    let id = ProfileId::parse(&requested).map_err(|error| error.to_string())?;
    let config = Config::load_for_repository(None).map_err(|error| error.to_string())?;
    let engine = WorkspaceEngine::new(config.clone());
    select_profile(&config, Path::new(&repo), id, &engine.audit_log)
        .map_err(|error| error.to_string())?;
    let body = effective_policy_json(&repo, &session_id)?;
    write_response(stream, request, 200, "application/json", &body)
}

/// `GET /api/permission-profiles`: the custom profiles the picker offers
/// beside the built-ins (spec 31 Task 7, deviation 4).
fn handle_permission_profiles(stream: &mut TcpStream, request: &Request) -> Result<(), String> {
    let config = Config::load_for_repository(None).map_err(|error| error.to_string())?;
    let custom: Vec<String> = custom_profile_ids(&config.data_dir)
        .map_err(|error| error.to_string())?
        .iter()
        .map(|id| id.as_str().to_string())
        .collect();
    let body = serde_json::json!({ "custom": custom }).to_string();
    write_response(stream, request, 200, "application/json", &body)
}

/// `GET /api/permission-profile-export?profile=<id>`: the profile as a file
/// someone can share, carrying only profile keys (spec 31, `context.md` §9).
fn handle_permission_profile_export(
    stream: &mut TcpStream,
    request: &Request,
) -> Result<(), String> {
    let requested = request.param("profile").unwrap_or_default();
    let id = ProfileId::parse(&requested).map_err(|error| error.to_string())?;
    let config = Config::load_for_repository(None).map_err(|error| error.to_string())?;
    let text = export_profile(&id, &config.data_dir).map_err(|error| error.to_string())?;
    let body = serde_json::json!({ "profile": id.as_str(), "text": text }).to_string();
    write_response(stream, request, 200, "application/json", &body)
}

/// `POST /api/permission-profile-import` with `name`, `text`, and optional
/// `repo`, `replace` and `preview`. With `preview=true` it only reviews, so
/// Settings can show what will not apply before anything is written
/// (`context.md` §9). The import writes a profile file, never user config,
/// so the editor's copy of `user.conf` cannot go stale (Task 7, deviation 6).
fn handle_permission_profile_import(
    stream: &mut TcpStream,
    request: &Request,
) -> Result<(), String> {
    let form = parse_form(&request.body);
    let name = required_form(&form, "name")?;
    let text = required_form(&form, "text")?;
    let repo = form.get("repo").cloned().unwrap_or_default();
    let flag = |key: &str| form.get(key).is_some_and(|value| value == "true");
    let root = (!repo.is_empty()).then(|| Path::new(&repo));
    let base = profile_import_base(root).map_err(|error| error.to_string())?;
    let (review, written, replaced) = if flag("preview") {
        // The name is checked even for a preview, so a reserved or invalid
        // name is refused before the user confirms anything.
        ProfileId::custom(&name).map_err(|error| error.to_string())?;
        (review_profile_import(&base, &text), false, false)
    } else {
        let engine = WorkspaceEngine::new(base.clone());
        let imported = import_profile(&base, &text, &name, flag("replace"), &engine.audit_log)
            .map_err(|error| error.to_string())?;
        (imported.review, true, imported.replaced)
    };
    let refusals = |keys: &[workspace_engine::RejectedConfigKey]| {
        keys.iter()
            .map(|rejected| serde_json::json!({ "key": rejected.key, "class": rejected.class.as_str() }))
            .collect::<Vec<_>>()
    };
    let exists = ProfileId::custom(&name)
        .ok()
        .and_then(|id| id.custom_path(&base.data_dir))
        .is_some_and(|path| path.is_file());
    let body = serde_json::json!({
        "profile": name,
        "written": written,
        "replaced": replaced,
        "exists": exists,
        "carried": review.carried,
        "notCarried": refusals(&review.not_carried),
        "loosening": refusals(&review.loosening),
    })
    .to_string();
    write_response(stream, request, 200, "application/json", &body)
}

/// The attributed policy for `repo`, from one fresh load. A custom profile's
/// refused keys are audited here, once per key, because this is where the
/// shell shows them (spec 31 criterion 4).
fn effective_policy_json(repo: &str, session_id: &str) -> Result<String, String> {
    let root = (!repo.is_empty()).then(|| Path::new(repo));
    let (config, report) =
        Config::load_for_repository_reporting(root).map_err(|error| error.to_string())?;
    let engine = WorkspaceEngine::new(config.clone());
    review_profile_rejections(&config.data_dir, &report, &engine.audit_log)
        .map_err(|error| error.to_string())?;
    let mode = if session_id.is_empty() {
        None
    } else if repo.is_empty() {
        return Err("A session's policy needs its repository".to_string());
    } else {
        let session = session_in_repository(&engine, repo, session_id)?;
        Some(engine.session_store.session_mode(&session.id))
    };
    serde_json::to_string(&EffectivePolicy::from_load(&config, &report, mode))
        .map_err(|error| error.to_string())
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

/// The shell's key resolution for model calls.
///
/// A missing key is fatal for a remote endpoint, but a loopback runtime such
/// as Ollama serves without credentials. There the key is empty and the
/// transport sends no `Authorization` header, rather than making the user
/// invent and store a credential that is never checked.
fn resolve_shell_model_api_key(config: &Config) -> Result<String, String> {
    match resolve_model_api_key(&config.model_api_key_env) {
        Ok(key) => Ok(key),
        Err(_) if config.model_endpoint_is_loopback() => Ok(String::new()),
        Err(error) => Err(error),
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
        // Diagnostics run outside any turn, so there is no session to attribute
        // these servers to — the entry's kind still identifies them.
        let registry = ProcessRegistry::open(&self.data_dir)?;
        for server in &self.servers {
            let mut client = match McpClient::connect(
                &server.config,
                server.auth_token.clone(),
                &registry,
                "",
            ) {
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

/// Copies artifacts into the data dir and records which server answered.
/// A structured report carries that in `via` and is rendered by the engine;
/// only unstructured output gets the prose prefix.
fn browser_report_from_tool_result(
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
fn web_diagnostic_reveal_target(config: &Config, path: &str) -> Result<PathBuf, String> {
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
fn web_diagnostic_json(record: &WebDiagnosticRecord) -> String {
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
fn mcp_test_connection(
    form: &HashMap<String, String>,
    data_dir: &Path,
) -> Result<Vec<String>, String> {
    let (config, token) = mcp_config_from_form(form)?;
    if config.transport == McpTransport::Stdio && config.command.trim().is_empty() {
        return Err("A command is required for a local (stdio) server.".to_string());
    }
    if config.transport == McpTransport::Http && config.url.trim().is_empty() {
        return Err("A URL is required for a remote (http) server.".to_string());
    }
    // Testing a server still spawns one, so it is registered like any other.
    // No session: the user is on the settings screen, not in a turn.
    let registry = ProcessRegistry::open(data_dir).map_err(|error| error.to_string())?;
    let mut client =
        McpClient::connect(&config, token, &registry, "").map_err(|error| error.to_string())?;
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
    let (config, _) = config_for_repo_with_provider(repo, provider)?;
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

/// Spec 58 requirement 13. Only `repo` is read from the form: the URL is
/// rebuilt from the validated `origin` (`GitHubRepo::url`), never taken from
/// the client or from the raw remote. `open` is injected so tests can see the
/// URL without launching a browser.
fn open_github_for_form(
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

/// The effective data directory, for callers outside this crate that need one
/// without building a whole engine — the Tauri app opening a terminal, say.
pub fn effective_data_dir() -> Result<PathBuf, String> {
    Config::load_for_repository(None)
        .map(|config| config.data_dir)
        .map_err(|error| error.to_string())
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

/// What the selected repository's config asked for and did not get, plus any
/// `command_allowlist` entries still awaiting the user's keep-or-discard.
/// Called when a repository is selected; both halves are reported once per
/// repository, so an unremarkable repository answers with empty lists.
fn repository_config_review_json(repo: &str) -> Result<String, String> {
    if repo.trim().is_empty() {
        return Ok("{\"rejectedKeys\":[],\"allowlistEntries\":[],\"hooks\":[]}".to_string());
    }
    let (config, report) = Config::load_for_repository_reporting(Some(Path::new(repo)))
        .map_err(|error| error.to_string())?;
    // A repository hook that is waiting on the user, or whose definition has
    // changed since they approved it, is an itemised review entry: the notice
    // shows the full command line, its events and its fail direction.
    let hooks: Vec<_> = list_hooks(&config, Path::new(repo))
        .into_iter()
        .filter(|hook| {
            matches!(
                hook.activation,
                HookActivation::PendingReview | HookActivation::ChangedSinceReview
            )
        })
        .collect();
    let engine = WorkspaceEngine::new(config);
    let notice = engine
        .repository_trust
        .review(&report, &engine.audit_log)
        .map_err(|error| error.to_string())?;
    let migration = engine
        .repository_trust
        .pending_allowlist_migration(&report)
        .map_err(|error| error.to_string())?;
    // Audited only. A profile's refusals are the user's own choice of file,
    // not the repository's, so they stay out of this notice (spec 31 Task 3).
    review_profile_rejections(&engine.config.data_dir, &report, &engine.audit_log)
        .map_err(|error| error.to_string())?;

    let rejected = notice
        .as_ref()
        .map(|notice| {
            notice
                .rejected
                .iter()
                .map(|rejected| {
                    serde_json::json!({
                        "key": rejected.key,
                        "class": rejected.class.as_str(),
                    })
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let entries = migration
        .as_ref()
        .map(|migration| migration.entries.clone())
        .unwrap_or_default();
    serde_json::to_string(&serde_json::json!({
        "repositoryPath": repo,
        "rejectedKeys": rejected,
        "allowlistEntries": entries,
        "hooks": hooks,
    }))
    .map_err(|error| error.to_string())
}

fn resolve_repository_allowlist_migration(repo: &str, keep: &[String]) -> Result<String, String> {
    let (config, report) = Config::load_for_repository_reporting(Some(Path::new(repo)))
        .map_err(|error| error.to_string())?;
    let engine = WorkspaceEngine::new(config);
    let path = engine
        .repository_trust
        .resolve_allowlist_migration(&report, keep, &engine.audit_log)
        .map_err(|error| error.to_string())?;
    Ok(format!(
        "{{\"path\":\"{}\",\"keptCount\":{}}}",
        escape_json(&path.to_string_lossy()),
        keep.len()
    ))
}

fn engine_for_repo(repo: &str) -> Result<WorkspaceEngine, String> {
    let (config, report) = config_and_report_for_repo(repo)?;
    Ok(WorkspaceEngine::new_with_report(config, &report))
}

fn config_for_repo_with_model_options(
    repo: &str,
    form: &HashMap<String, String>,
) -> Result<(Config, RepositoryConfigReport), String> {
    let (mut config, report) = config_and_report_for_repo(repo)?;
    apply_model_form_options(&mut config, form)?;
    Ok((config, report))
}

fn config_for_repo(repo: &str) -> Result<Config, String> {
    config_and_report_for_repo(repo).map(|(config, _)| config)
}

/// The loaded config and the report that says which scope set each key. Chat
/// and command-approval paths build their engine with the report so an
/// approval raised by a setting can name who set it (spec 62 Task 3).
fn config_and_report_for_repo(repo: &str) -> Result<(Config, RepositoryConfigReport), String> {
    let repo_path = if repo.is_empty() {
        None
    } else {
        Some(Path::new(repo))
    };
    Config::load_for_repository_reporting(repo_path).map_err(|error| error.to_string())
}

fn config_for_repo_with_provider(
    repo: &str,
    provider: Option<&str>,
) -> Result<(Config, RepositoryConfigReport), String> {
    let (mut config, report) = config_and_report_for_repo(repo)?;
    if let Some(provider) = provider.map(str::trim).filter(|value| !value.is_empty()) {
        switch_model_provider(&mut config, provider)?;
    }
    Ok((config, report))
}

/// Makes `provider` the active one, in memory only.
fn switch_model_provider(config: &mut Config, provider: &str) -> Result<(), String> {
    config.model_provider =
        normalize_model_provider(provider).map_err(|error| error.to_string())?;
    config.apply_model_provider_defaults();
    Ok(())
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
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        409 => "Conflict",
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
    /// Boxed because a `TaskPlan` is much larger than the other variants, and
    /// an unboxed one would make every `TurnEvent` — one per streamed token —
    /// carry its footprint. `active` is false only for the final event of a
    /// turn that ended (spec 66 §5.4).
    Plan {
        plan: Box<TaskPlan>,
        active: bool,
    },
    /// Boxed for the same reason as `Plan`: a record carries a whole report.
    WebDiagnostic(Box<WebDiagnosticRecord>),
    Token(String),
    Done(Box<ChatTurnResult>),
    Failed(String),
}

/// Adds `explorationRound`/`maxExplorationRounds` beside the action counts;
/// older readers ignore the new fields (spec 59 §5.5). `notesChars` is the
/// turn's notes size for the progress label (spec 65 §5.8).
fn phase_json(phase: &TurnPhase) -> String {
    format!(
        "{{\"phase\":\"{}\",\"label\":\"{}\",\"round\":{},\"maxRounds\":{},\"explorationRound\":{},\"maxExplorationRounds\":{},\"notesChars\":{}}}",
        phase.kind.as_str(),
        escape_json(&phase.label),
        phase.round,
        phase.max_rounds,
        phase.exploration_round,
        phase.max_exploration_rounds,
        phase.notes_chars
    )
}

/// The plan as the panel needs it: the steps as the engine holds them, plus
/// the two things the engine has already decided and the frontend must not
/// decide again — each step's outcome and the plan's own summary line.
///
/// `violatesSingleInProgress` is sent rather than left for the panel to work
/// out, because a panel that silently rendered the first of two in-progress
/// steps would conceal exactly the invariant requirement 2 exists to catch.
pub(crate) fn plan_json(plan: &TaskPlan, active: bool) -> String {
    let report = plan.report();
    let steps = plan
        .steps
        .iter()
        .zip(report.steps.iter())
        .map(|(step, reported)| {
            format!(
                "{{\"id\":\"{}\",\"title\":\"{}\",\"detail\":{},\"status\":\"{}\",\"outcome\":\"{}\",\"evidence\":{}}}",
                escape_json(&step.id),
                escape_json(&step.title),
                step.detail
                    .as_ref()
                    .map(|detail| format!("\"{}\"", escape_json(detail)))
                    .unwrap_or_else(|| "null".to_string()),
                serde_status(step.status),
                reported.outcome.as_str(),
                serde_json::to_string(&step.evidence).unwrap_or_else(|_| "[]".to_string())
            )
        })
        .collect::<Vec<_>>()
        .join(",");
    format!(
        "{{\"taskId\":\"{}\",\"steps\":[{}],\"summary\":\"{}\",\"active\":{},\"isComplete\":{},\"verified\":{},\"unverified\":{},\"blocked\":{},\"skipped\":{},\"outstanding\":{},\"violatesSingleInProgress\":{}}}",
        escape_json(&plan.task_id),
        steps,
        escape_json(&report.summary(active)),
        active,
        report.is_complete,
        report.verified,
        report.unverified,
        report.blocked,
        report.skipped,
        report.outstanding,
        plan.violates_single_in_progress()
    )
}

/// The wire spelling of a step status — the same `snake_case` serde writes into
/// the session log, so the panel and a hand-read log agree.
fn serde_status(status: StepStatus) -> &'static str {
    match status {
        StepStatus::Pending => "pending",
        StepStatus::InProgress => "in_progress",
        StepStatus::Completed => "completed",
        StepStatus::Blocked => "blocked",
        StepStatus::Skipped => "skipped",
    }
}

/// A plan stopped for review (spec 21 §5.5). Shaped like `plan_json` plus the
/// proposal id and what the approval unblocks, so the review card and the live
/// panel render from the same fields.
fn plan_proposal_json(proposal: Option<&AgentPlanProposal>) -> String {
    let Some(proposal) = proposal else {
        return "null".to_string();
    };
    format!(
        "{{\"proposalId\":\"{}\",\"deferredAction\":\"{}\",\"plan\":{}}}",
        escape_json(&proposal.id),
        escape_json(&proposal.deferred_action),
        // The review card is a turn waiting for a human, not one that ended.
        plan_json(&proposal.plan, true)
    )
}

/// Spec 56's offer: both providers, the refusal, and the provider's own words.
/// Deliberately has no `allowAlways` — the one field the command card would
/// read to offer a standing decision.
fn fallback_proposal_json(proposal: Option<&AgentFallbackProposal>) -> String {
    let Some(proposal) = proposal else {
        return "null".to_string();
    };
    format!(
        "{{\"proposalId\":\"{}\",\"refusedProvider\":\"{}\",\"refusedModel\":\"{}\",\"fallbackProvider\":\"{}\",\"fallbackModel\":\"{}\",\"refusalCode\":\"{}\",\"message\":\"{}\"}}",
        escape_json(&proposal.id),
        escape_json(&proposal.refused_provider),
        escape_json(&proposal.refused_model),
        escape_json(&proposal.fallback_provider),
        escape_json(&proposal.fallback_model),
        escape_json(&proposal.refusal_code),
        escape_json(&proposal.message)
    )
}

/// Spec 50's question, shaped for the card. It carries no `proposalId`,
/// `command`, `patchId` or `requiresApproval`: a question has nothing to
/// approve, and a field the command or patch route recognises is how one would
/// become an approval in the client.
fn question_proposal_json(proposal: Option<&AgentQuestionProposal>) -> String {
    let Some(proposal) = proposal else {
        return "null".to_string();
    };
    format!(
        "{{\"questionId\":\"{}\",\"question\":\"{}\",\"options\":[{}],\"allowFreeText\":{}}}",
        escape_json(&proposal.question_id),
        escape_json(&proposal.question),
        json_string_array(&proposal.options),
        proposal.allow_free_text
    )
}

/// Spec 61's round-budget pause: the rounds used and granted, and whether the
/// turn is at the hard cap (so the shell can offer only *Answer now*).
fn round_budget_proposal_json(proposal: Option<&AgentRoundBudgetProposal>) -> String {
    let Some(proposal) = proposal else {
        return "null".to_string();
    };
    format!(
        "{{\"proposalId\":\"{}\",\"roundsUsed\":{},\"roundsTotal\":{},\"roundsGranted\":{},\"roundsMore\":{},\"atHardCap\":{}}}",
        escape_json(&proposal.id),
        proposal.rounds_used,
        proposal.rounds_total,
        proposal.rounds_granted,
        proposal.rounds_more,
        proposal.at_hard_cap,
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
                        TurnEvent::Plan { plan, active } => {
                            write_sse_event(out, "plan", &plan_json(plan, *active))
                        }
                        TurnEvent::WebDiagnostic(record) => {
                            write_sse_event(out, "web_diagnostic", &web_diagnostic_json(record))
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
    let worker = spawn_with_shell_stack("damaian-turn", move || {
        let event = match turn(&worker_cancel, &sender) {
            Ok(result) => TurnEvent::Done(Box::new(result)),
            Err(error) => TurnEvent::Failed(error),
        };
        let _ = sender.send(event);
        // `sender` drops here, which is what disconnects the channel and lets
        // the relay finish even if the terminal event could not be delivered.
    })
    .map_err(|error| format!("start the turn thread: {error}"))?;

    let outcome = relay_turn_events(stream, &cancel, receiver, KEEPALIVE_INTERVAL);
    // Joined unconditionally: the worker owns the curl child, and leaving it
    // unreaped is how a stopped turn would keep billing tokens.
    let _ = worker.join();
    outcome
}

fn chat_result_json(result: &ChatTurnResult) -> String {
    let turn_notes = match &result.turn_notes {
        Some(notes) => format!(
            "{{\"text\":\"{}\",\"carried\":{}}}",
            escape_json(&notes.text),
            notes.carried
        ),
        None => "null".to_string(),
    };
    format!(
        "{{\"response\":\"{}\",\"contextFiles\":[{}],\"sessionId\":\"{}\",\"taskId\":\"{}\",\"taskStatus\":\"{}\",\"modelRunId\":\"{}\",\"incomplete\":{},\"cancelled\":{},\"commandProposal\":{},\"patchProposal\":{},\"planProposal\":{},\"fallbackProposal\":{},\"roundBudgetProposal\":{},\"questionProposal\":{},\"checkBatch\":{},\"verificationReport\":{},\"turnNotes\":{},\"usage\":{}}}",
        escape_json(&result.response),
        json_string_array(&result.context_files),
        escape_json(&result.session.id),
        escape_json(&result.task.id),
        result.task.status.as_str(),
        escape_json(&result.model_run.run_id),
        result.model_run.incomplete,
        result.cancelled,
        command_proposal_json(result),
        patch_proposal_json(result),
        plan_proposal_json(result.plan_proposal.as_ref()),
        fallback_proposal_json(result.fallback_proposal.as_ref()),
        round_budget_proposal_json(result.round_budget_proposal.as_ref()),
        question_proposal_json(result.question_proposal.as_ref()),
        check_batch_json(result.check_batch.as_ref()),
        verification_report_json(result.verification_report.as_ref()),
        turn_notes,
        task_usage_json(result.usage.as_ref(), result.estimated_cost)
    )
}

/// One task's usage, or `null` when nothing was recorded. Shaped like the
/// per-task fields on `/api/session` so the frontend renders both with the
/// same code.
/// `estimated_cost` is the user's own rates applied to these tokens, and is a
/// separate field from `reportedCost` on purpose: one is what the provider
/// charged, the other is arithmetic the user configured.
///
/// Takes the whole [`CostEstimate`] rather than its amount so the upper-bound
/// label cannot be lost on the way to the client: this is the single funnel
/// every caller goes through. Spec 49 §5.3.
pub(crate) fn task_usage_json(
    usage: Option<&TaskUsage>,
    estimated_cost: Option<CostEstimate>,
) -> String {
    let Some(usage) = usage else {
        return "null".to_string();
    };
    // Omitted entirely when no run reported a split, for the same reason
    // `reportedCost` is: a zero renders as a measurement of nothing, and here
    // that reads as "caching is broken" rather than "we cannot see it".
    // `runsWithoutCacheReport` is always present, because it is what lets the
    // client say which of the two it is. Spec 49 §5.6.
    let cache = match usage.cached_input_tokens {
        Some(cached) => {
            // The denominator is the input tokens of the runs that reported,
            // not the task's — see `TaskUsage::cache_reported_input_tokens`.
            // Zero is reachable: a refused call records a measured zero.
            let rate = match usage.cache_reported_input_tokens {
                0 => String::new(),
                total => format!(",\"cacheHitRate\":{}", cached as f64 / total as f64),
            };
            format!(",\"cachedInputTokens\":{cached}{rate}")
        }
        None => String::new(),
    };
    format!(
        "{{\"inputTokens\":{},\"outputTokens\":{},\"usageSource\":\"{}\",\"runCount\":{}{}\
         ,\"runsWithoutCacheReport\":{}{}{}}}",
        usage.input_tokens,
        usage.output_tokens,
        usage.source.as_str(),
        usage.run_count,
        cache,
        usage.runs_without_cache_report,
        match usage.reported_cost {
            Some(cost) => format!(",\"reportedCost\":{cost}"),
            None => String::new(),
        },
        match estimated_cost {
            // `estimatedCostIsUpperBound` rides with the figure rather than
            // being derivable from it: a client cannot tell a ceiling from an
            // exact number by looking at the number.
            Some(estimate) => format!(
                ",\"estimatedCost\":{},\"estimatedCostIsUpperBound\":{}",
                estimate.amount(),
                estimate.is_upper_bound()
            ),
            None => String::new(),
        }
    )
}

fn command_proposal_json(result: &ChatTurnResult) -> String {
    result
        .command_proposal
        .as_ref()
        .map(agent_command_json)
        .unwrap_or_else(|| "null".to_string())
}

/// One check or command proposal as the card renders it. Shared by the single
/// command card and the check batch so a check cannot show a different risk,
/// cause or `Allow Always` answer from the command it names.
fn agent_command_json(proposal: &workspace_engine::AgentCommandProposal) -> String {
    format!(
        "{{\"proposalId\":\"{}\",\"command\":\"{}\",\"prompt\":\"{}\",\"risk\":\"{}\",\"requiresApproval\":{},\"blocked\":{},\"cause\":\"{}\",\"reason\":\"{}\",\"allowAlways\":{},\"allowBrowserDiagnosticsForSession\":{}}}",
        escape_json(&proposal.id),
        escape_json(&proposal.command),
        escape_json(&proposal.prompt),
        escape_json(&proposal.risk),
        proposal.requires_approval,
        proposal.blocked,
        escape_json(&proposal.cause),
        escape_json(&proposal.reason),
        proposal.allow_always,
        proposal.allow_browser_diagnostics_for_session
    )
}

/// One card listing every check the verification loop wants to run.
fn check_batch_json(batch: Option<&AgentCheckBatch>) -> String {
    let Some(batch) = batch else {
        return "null".to_string();
    };
    format!(
        "{{\"proposalId\":\"{}\",\"checks\":[{}]}}",
        escape_json(&batch.id),
        batch
            .checks
            .iter()
            .map(agent_command_json)
            .collect::<Vec<_>>()
            .join(",")
    )
}

/// The structured report the report card renders. The type is already
/// camelCase-serialised; the text copy is in `response`.
fn verification_report_json(report: Option<&VerificationReport>) -> String {
    report
        .and_then(|report| serde_json::to_string(report).ok())
        .unwrap_or_else(|| "null".to_string())
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
        .map(session_summary_json)
        .collect::<Vec<_>>()
        .join(",")
}

fn export_filename(
    engine: &WorkspaceEngine,
    session_id: &str,
    extension: &str,
) -> Result<String, String> {
    let title = engine
        .session_store
        .read_session(session_id)
        .map_err(|error| error.to_string())?
        .map(|session| session.title)
        .unwrap_or_else(|| session_id.to_string());
    let sanitized: String = title
        .chars()
        .map(|character| {
            if character.is_alphanumeric() || character == '-' || character == '_' {
                character
            } else {
                '_'
            }
        })
        .collect();
    let sanitized = sanitized.trim_matches('_');
    Ok(format!(
        "{}.{extension}",
        if sanitized.is_empty() {
            session_id.to_string()
        } else {
            sanitized.to_string()
        }
    ))
}

/// One session as the session list shows it. Carries no `mode`: the mode lives
/// in a separate event, and reading it for every listed session would read
/// every session log a second time. The open session's mode arrives through
/// [`session_json`] instead.
fn session_summary_json(session: &Session) -> String {
    format!("{{{}}}", session_fields_json(session))
}

/// `GET /api/findings`. Every findings route takes `repo`, because staleness
/// is derived against the repository's files, and refuses a session from
/// another repository (spec 22 `context.md` §16).
fn handle_findings(stream: &mut TcpStream, request: &Request) -> Result<(), String> {
    let repo = required_param(request, "repo")?;
    let session_id = required_param(request, "session_id")?;
    let engine = engine_for_repo(&repo)?;
    session_in_repository(&engine, &repo, &session_id)?;
    // Language-server diagnostics become findings here, from what the hub
    // already holds (spec 25 `context.md` §10). A failure must not cost the
    // panel what it already has.
    if let Err(error) = engine.record_language_server_findings(&session_id, &repo) {
        eprintln!("Could not record language-server findings for session {session_id}: {error}");
    }
    let body = findings_json(&engine, &repo, &session_id)?;
    write_response(stream, request, 200, "application/json", &body)
}

/// `POST /api/finding-status`, answered with the refreshed findings so the
/// panel redraws from what was stored.
fn handle_finding_status(stream: &mut TcpStream, request: &Request) -> Result<(), String> {
    let form = parse_form(&request.body);
    let repo = required_form(&form, "repo")?;
    let session_id = required_form(&form, "session_id")?;
    let finding_id = required_form(&form, "finding_id")?;
    let requested = required_form(&form, "status")?;
    // Rejected, never defaulted, as for `/api/session-mode`. `stale`
    // parses, and the store refuses it (spec 22 `context.md` §13.2).
    let status = FindingStatus::parse(&requested).ok_or_else(|| {
        format!("Unknown finding status: {requested}. Expected open, dismissed, or fixed.")
    })?;
    let engine = engine_for_repo(&repo)?;
    session_in_repository(&engine, &repo, &session_id)?;
    engine
        .session_store
        .set_finding_status(&session_id, &finding_id, status)
        .map_err(|error| error.to_string())?;
    let body = findings_json(&engine, &repo, &session_id)?;
    write_response(stream, request, 200, "application/json", &body)
}

/// `POST /api/findings-repair`. Marks nothing: it returns the request and its
/// rendered prompt, or `null` when nothing is left to repair (§15, §16).
fn handle_findings_repair(stream: &mut TcpStream, request: &Request) -> Result<(), String> {
    let form = parse_form(&request.body);
    let repo = required_form(&form, "repo")?;
    let session_id = required_form(&form, "session_id")?;
    let selected: Vec<String> = required_form(&form, "finding_ids")?
        .split(',')
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .map(str::to_string)
        .collect();
    let engine = engine_for_repo(&repo)?;
    session_in_repository(&engine, &repo, &session_id)?;
    // Built from the findings as they are now, so an edit since the
    // panel was drawn still makes its finding stale (§15).
    let findings = engine
        .session_store
        .read_findings(&session_id, Path::new(&repo))
        .map_err(|error| error.to_string())?;
    let repair = RepairRequest::select(&findings, &selected);
    let repair_json = serde_json::to_string(&repair).map_err(|error| error.to_string())?;
    let prompt = repair.render().map_or_else(
        || "null".to_string(),
        |text| format!("\"{}\"", escape_json(&text)),
    );
    write_response(
        stream,
        request,
        200,
        "application/json",
        &format!("{{\"request\":{repair_json},\"prompt\":{prompt}}}"),
    )
}

/// `GET /api/repository-map?repo=`: the repository's map, plus how the store
/// got it (spec 24 Task 8). Shaped like `damaian repo-map --json`, so the UI
/// and the CLI read the same JSON.
fn handle_repository_map(stream: &mut TcpStream, request: &Request) -> Result<(), String> {
    let repo = required_param(request, "repo")?;
    let engine = engine_for_repo(&repo)?;
    let (map, load) = engine
        .repository_map(&repo)
        .map_err(|error| error.to_string())?;
    let body = format!(
        "{{\"load\":{},\"map\":{}}}",
        map_load_json(&load),
        map.to_json()
    );
    write_response(stream, request, 200, "application/json", &body)
}

/// `POST /api/repository-roots`: add, remove or clear a user root override in
/// the repository's own config, then answer with the rebuilt map.
///
/// The write goes to `<repo>/.damaian/config.conf`, in the user's working
/// tree, so Git shows it; the UI says so before the first write. A repo that
/// is not a usable directory, or lies outside `allowed_roots`, is refused, so
/// an override is only ever written inside a repository the user may open.
fn handle_repository_roots(stream: &mut TcpStream, request: &Request) -> Result<(), String> {
    let form = parse_form(&request.body);
    let repo = required_form(&form, "repo")?;
    let action = required_form(&form, "action")?;
    let path = required_form(&form, "path")?;
    validate_working_folder(&repo)?;
    let config_path = repository_config_write_target(&engine_for_repo(&repo)?, &repo)?;
    let edit = match action.as_str() {
        "add" => workspace_engine::RootOverrideEdit::Add,
        "remove" => workspace_engine::RootOverrideEdit::Remove,
        "clear" => workspace_engine::RootOverrideEdit::Clear,
        other => {
            return Err(format!(
                "repository-roots action must be add, remove or clear, got {other}"
            ));
        }
    };
    let mut overlay = workspace_engine::ConfigOverlay::load_or_default(&config_path)
        .map_err(|error| error.to_string())?;
    workspace_engine::edit_root_overrides(&mut overlay, edit, &path);
    overlay
        .save(&config_path)
        .map_err(|error| error.to_string())?;

    // A fresh engine, so the map is built from the config just written.
    let engine = engine_for_repo(&repo)?;
    let (map, load) = engine
        .repository_map(&repo)
        .map_err(|error| error.to_string())?;
    let body = format!(
        "{{\"configPath\":\"{}\",\"load\":{},\"map\":{}}}",
        escape_json(&config_path.to_string_lossy()),
        map_load_json(&load),
        map.to_json()
    );
    write_response(stream, request, 200, "application/json", &body)
}

/// How many symbol matches `/api/symbols` returns. A search box wants a short
/// list; the CLI takes an explicit limit for anything larger.
const SYMBOL_SEARCH_LIMIT: usize = 50;

/// `GET /api/symbols?repo=&q=`: the repository's symbol lookup, each match
/// carrying its `Symbol`, match quality and `provenance`. A heuristic match is
/// labelled as one by the payload, so the UI never has to infer it.
fn handle_symbols(stream: &mut TcpStream, request: &Request) -> Result<(), String> {
    let repo = required_param(request, "repo")?;
    let query = required_param(request, "q")?;
    let engine = engine_for_repo(&repo)?;
    let root = symbol_repository(&engine, &repo)?;
    let matches = engine
        .symbol_search(&root, &query, SYMBOL_SEARCH_LIMIT)
        .map_err(|error| error.to_string())?;
    let body = format!(
        "{{\"matches\":{}}}",
        workspace_engine::symbol_matches_json(&matches)
    );
    write_response(stream, request, 200, "application/json", &body)
}

/// `GET /api/symbol-references?repo=&path=&line=&name=`: Task 9's on-demand
/// answer, unchanged, for the UI. `available: false` is a normal answer: the
/// heuristic tier does not guess references.
fn handle_symbol_references(stream: &mut TcpStream, request: &Request) -> Result<(), String> {
    let repo = required_param(request, "repo")?;
    let path = required_param(request, "path")?;
    let line = required_param(request, "line")?
        .parse::<u32>()
        .ok()
        .filter(|line| *line > 0)
        .ok_or_else(|| "line must be a positive integer".to_string())?;
    let name = required_param(request, "name")?;
    let engine = engine_for_repo(&repo)?;
    let root = symbol_repository(&engine, &repo)?;
    // The path arrives from the browser, so it goes through the same policy
    // every other browser-supplied path does before a server sees it.
    let target = engine
        .path_policy
        .resolve_existing(&root, &path, false)
        .map_err(|error| error.to_string())?;
    engine
        .path_policy
        .assert_not_restricted(&target.relative_path, false)
        .map_err(|error| error.to_string())?;
    let symbol = SymbolRef {
        path: target.relative_path,
        name,
        line,
    };
    let references = engine
        .symbol_references(&root, &symbol)
        .map_err(|error| error.to_string())?;
    write_response(
        stream,
        request,
        200,
        "application/json",
        &workspace_engine::symbol_references_json(&references),
    )
}

/// `GET /api/language-servers?repo=`: every language the user configured, plus
/// the hub's live state once a lookup has started one. Settings can therefore
/// show a configured server before anything has spawned it.
fn handle_language_servers(stream: &mut TcpStream, request: &Request) -> Result<(), String> {
    let repo = required_param(request, "repo")?;
    let engine = engine_for_repo(&repo)?;
    let root = symbol_repository(&engine, &repo)?;
    let statuses = engine
        .language_servers()
        .status(&repository_id_for_root(&root));
    let body = language_servers_json(&engine.config.language_servers, statuses);
    write_response(stream, request, 200, "application/json", &body)
}

/// The JSON `/api/language-servers` serves. A configured server that no lookup
/// has started yet is `idle`; a hub entry's own state replaces that once it
/// exists, so an unavailable or crashed server keeps its reason.
fn language_servers_json(
    configured: &BTreeMap<String, String>,
    statuses: Vec<LanguageServerStatus>,
) -> String {
    let mut servers: BTreeMap<String, serde_json::Value> = configured
        .iter()
        .map(|(language, command)| {
            (
                language.clone(),
                serde_json::json!({
                    "language": language,
                    "command": command,
                    "state": "idle",
                    "pid": serde_json::Value::Null,
                }),
            )
        })
        .collect();
    for status in statuses {
        // Strings, numbers and unit variants: none of this can fail.
        let value = serde_json::to_value(&status).expect("a language server status serialises");
        servers.insert(status.language.clone(), value);
    }
    let servers: Vec<serde_json::Value> = servers.into_values().collect();
    serde_json::to_string(&serde_json::json!({ "servers": servers }))
        .expect("a language server list serialises")
}

/// A symbol endpoint's repository root, canonical and inside `allowed_roots`.
/// Indexing does not consult `allowed_roots` either, so without this check the
/// endpoints would read any directory they are named.
fn symbol_repository(engine: &WorkspaceEngine, repo: &str) -> Result<PathBuf, String> {
    let root = validate_working_folder(repo)?;
    engine
        .path_policy
        .canonical_root(&root)
        .map_err(|error| error.to_string())
}

/// Where a root override for `repo` is written, once `repo` passes the path
/// policy. Indexing does not consult `allowed_roots`, so without this check
/// the endpoint would write into any directory it is named.
fn repository_config_write_target(engine: &WorkspaceEngine, repo: &str) -> Result<PathBuf, String> {
    engine
        .path_policy
        .canonical_root(repo)
        .map_err(|error| error.to_string())?;
    Ok(Config::repository_config_path(repo))
}

/// `GET /api/command-proposal?repo=&proposal_id=`: the stored proposal's
/// working directory and repository root, so a command-approval card can say
/// "Runs in `packages/api`".
///
/// The card's payload streams out of `chat.rs`'s `AgentCommandProposal`, which
/// carries neither (spec 24 Task 8); the shell reads them from the
/// `CommandStore` the proposal was written to instead of changing `chat.rs`.
fn handle_command_proposal(stream: &mut TcpStream, request: &Request) -> Result<(), String> {
    let repo = required_param(request, "repo")?;
    let proposal_id = required_param(request, "proposal_id")?;
    let engine = engine_for_repo(&repo)?;
    let proposal = engine
        .validation_orchestrator
        .load_proposal(&proposal_id)
        .map_err(|error| error.to_string())?;
    let body = format!(
        "{{\"proposalId\":\"{}\",\"workingDirectory\":\"{}\",\"repositoryRoot\":\"{}\"}}",
        escape_json(&proposal.id),
        escape_json(&proposal.working_directory),
        escape_json(&proposal.repository_root)
    );
    write_response(stream, request, 200, "application/json", &body)
}

fn map_load_json(load: &workspace_engine::MapLoad) -> String {
    use workspace_engine::MapLoad;
    match load {
        MapLoad::Reused => "{\"outcome\":\"reused\"}".to_string(),
        MapLoad::Built => "{\"outcome\":\"built\"}".to_string(),
        MapLoad::Rebuilt { reason } => match reason {
            workspace_engine::RebuildReason::SchemaMismatch { found } => format!(
                "{{\"outcome\":\"rebuilt\",\"reason\":\"{}\",\"foundSchemaVersion\":{found}}}",
                reason.as_str()
            ),
            _ => format!(
                "{{\"outcome\":\"rebuilt\",\"reason\":\"{}\"}}",
                reason.as_str()
            ),
        },
    }
}

/// The session, when it exists and belongs to `repo`. A finding's staleness
/// is judged against `repo`'s files, so a session from another checkout must
/// not be read against this one (spec 22 `context.md` §16).
fn session_in_repository(
    engine: &WorkspaceEngine,
    repo: &str,
    session_id: &str,
) -> Result<Session, String> {
    let session = engine
        .session_store
        .read_session(session_id)
        .map_err(|error| error.to_string())?
        .ok_or_else(|| format!("Unknown session: {session_id}"))?;
    let repository_id = engine
        .indexer
        .repository_id_for_path(repo)
        .map_err(|error| error.to_string())?;
    if session.repository_id != repository_id {
        return Err(format!(
            "Session {session_id} belongs to another repository"
        ));
    }
    Ok(session)
}

/// `{"findings":[…]}` in record order, with status derived against `repo`.
fn findings_json(engine: &WorkspaceEngine, repo: &str, session_id: &str) -> Result<String, String> {
    let findings = engine
        .session_store
        .read_findings(session_id, Path::new(repo))
        .map_err(|error| error.to_string())?;
    let findings = serde_json::to_string(&findings).map_err(|error| error.to_string())?;
    Ok(format!("{{\"findings\":{findings}}}"))
}

/// One session with its working mode (spec 20), for the responses that open,
/// create, rename, or change the mode of a single session.
fn session_json(session: &Session, mode: SessionMode) -> String {
    format!(
        "{{{},\"mode\":\"{}\"}}",
        session_fields_json(session),
        mode.as_str()
    )
}

fn session_fields_json(session: &Session) -> String {
    format!(
        "\"id\":\"{}\",\"repositoryId\":\"{}\",\"title\":\"{}\",\"createdAtMs\":{},\"updatedAtMs\":{},\"summary\":\"{}\",\"origin\":\"{}\"",
        escape_json(&session.id),
        escape_json(&session.repository_id),
        escape_json(&session.title),
        session.created_at_ms,
        session.updated_at_ms,
        escape_json(&session.summary),
        escape_json(&session.origin)
    )
}

/// Each task's status and, when it has any, what it spent.
///
/// Usage fields are **omitted** for a task with no usage events rather than
/// sent as zero: a session written before spec 19 has no figures, and a zero
/// would render as a turn that cost nothing. Same for `reportedCost`, which is
/// absent unless the provider actually reported one.
fn task_states_json(
    statuses: &HashMap<String, String>,
    usage: &HashMap<String, TaskUsage>,
    plans: &HashMap<String, TaskPlan>,
    failure_kinds: &HashMap<String, String>,
    web_diagnostics: &HashMap<String, Vec<WebDiagnosticRecord>>,
    notes: &BTreeMap<String, TurnNotes>,
    config: &Config,
) -> String {
    let mut entries: Vec<&String> = statuses.keys().collect();
    // Sorted so the payload is stable between requests.
    entries.sort();
    entries
        .iter()
        .map(|id| {
            let usage_json = match usage.get(*id) {
                Some(total) => {
                    let estimated_cost = config.estimated_cost(&TokenUsage {
                        input_tokens: total.input_tokens,
                        output_tokens: total.output_tokens,
                        cached_input_tokens: total.cached_input_tokens,
                        source: total.source,
                    });
                    let body = task_usage_json(Some(total), estimated_cost);
                    // Spliced into this entry rather than nested, so the shape
                    // matches what the turn response sends and the frontend
                    // reads both with the same code.
                    format!(",{}", body.trim_start_matches('{').trim_end_matches('}'))
                }
                None => String::new(),
            };
            // Absent rather than an empty plan when the turn had none: a
            // trivial turn and a plan that proposed nothing are different
            // facts, and the panel must not appear for the first.
            //
            // `active` comes from the task status already in this payload, so a
            // reloaded session agrees with the live stream about whether an
            // open step is "running" (spec 66 §5.4). An unknown or legacy
            // status is treated as ended: saying "running" about a turn we
            // cannot classify is the wrong direction.
            let active = statuses
                .get(*id)
                .and_then(|status| workspace_engine::TaskStatus::parse(status))
                .is_some_and(|status| status.is_active());
            let plan_json_field = match plans.get(*id) {
                Some(plan) => format!(",\"plan\":{}", plan_json(plan, active)),
                None => String::new(),
            };
            // A named failure reason, when the turn failed with one (spec 48
            // §5.4). Absent for every other outcome, so old sessions and
            // successful turns are unchanged.
            let failure_kind_field = match failure_kinds.get(*id) {
                Some(kind) => format!(",\"failureKind\":\"{}\"", escape_json(kind)),
                None => String::new(),
            };
            // Absent when the turn ran no diagnostic, like `plan`, so the
            // card appears only for turns that have one.
            let web_diagnostics_field = match web_diagnostics.get(*id) {
                Some(records) if !records.is_empty() => format!(
                    ",\"webDiagnostics\":[{}]",
                    records
                        .iter()
                        .map(web_diagnostic_json)
                        .collect::<Vec<_>>()
                        .join(",")
                ),
                _ => String::new(),
            };
            // The turn's final notes, joined by `taskId` (spec 65 §5.8).
            // Absent when the turn recorded none, so a reload shows the
            // disclosure only for turns that have one.
            let notes_field = match notes.get(*id) {
                Some(notes) if !notes.text.is_empty() => format!(
                    ",\"notes\":{{\"text\":\"{}\",\"carried\":{}}}",
                    escape_json(&notes.text),
                    notes.carried
                ),
                _ => String::new(),
            };
            format!(
                "{{\"id\":\"{}\",\"status\":\"{}\"{}{}{}{}{}}}",
                escape_json(id),
                escape_json(&statuses[*id]),
                usage_json,
                failure_kind_field,
                plan_json_field,
                notes_field,
                web_diagnostics_field
            )
        })
        .collect::<Vec<_>>()
        .join(",")
}

fn messages_json(
    messages: &[(u64, ChatMessage)],
    questions: &HashMap<String, QuestionState>,
) -> String {
    messages
        .iter()
        .map(|(seq, message)| message_json(*seq, message, questions.get(&message.id)))
        .collect::<Vec<_>>()
        .join(",")
}

fn message_json(seq: u64, message: &ChatMessage, question: Option<&QuestionState>) -> String {
    // A question card needs what the question was asked with; no other role
    // has anything to add, so the fields appear only on `question`.
    let question_fields = question
        .map(|state| {
            format!(
                ",\"options\":[{}],\"allowFreeText\":{}",
                json_string_array(&state.options),
                state.allow_free_text
            )
        })
        .unwrap_or_default();
    // `kind` is absent for an ordinary message, so the payload is unchanged
    // for every session written before the plan reminder existed.
    let kind_field = message
        .kind
        .as_ref()
        .map(|kind| format!(",\"kind\":\"{}\"", escape_json(kind)))
        .unwrap_or_default();
    format!(
        "{{\"id\":\"{}\",\"sessionId\":\"{}\",\"taskId\":{},\"role\":\"{}\",\"content\":\"{}\",\"createdAtMs\":{},\"seq\":{}{}{}}}",
        escape_json(&message.id),
        escape_json(&message.session_id),
        message
            .task_id
            .as_ref()
            .map(|value| format!("\"{}\"", escape_json(value)))
            .unwrap_or_else(|| "null".to_string()),
        escape_json(&message.role),
        escape_json(&message.content),
        message.created_at_ms,
        seq,
        kind_field,
        question_fields
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

/// What the checkpoint list view needs: when, what turn, how much it covers,
/// and whether it has been rewound to. Never file content — the manifest holds
/// none, and this must not become the place that changes.
fn checkpoint_list_json(manifests: &[workspace_engine::CheckpointManifest]) -> String {
    let entries = manifests
        .iter()
        .map(|manifest| {
            format!(
                "{{\"checkpointId\":\"{}\",\"sessionId\":\"{}\",\"createdAtMs\":{},\"summary\":\"{}\",\"userMessageId\":{},\"fileCount\":{},\"excludedCount\":{},\"commandEffectsCovered\":{},\"restoredAtMs\":{},\"files\":[{}],\"pendingApprovals\":[{}]}}",
                escape_json(&manifest.checkpoint_id),
                escape_json(&manifest.session_id),
                manifest.created_at_ms,
                escape_json(&manifest.summary),
                manifest
                    .user_message_id
                    .as_ref()
                    .map(|id| format!("\"{}\"", escape_json(id)))
                    .unwrap_or_else(|| "null".to_string()),
                manifest.files.len(),
                manifest.excluded.len(),
                manifest.command_effects_covered,
                manifest
                    .restored_at_ms
                    .map(|value| value.to_string())
                    .unwrap_or_else(|| "null".to_string()),
                checkpoint_files_json(&manifest.files),
                checkpoint_pending_approvals_json(&manifest.pending_approvals),
            )
        })
        .collect::<Vec<_>>()
        .join(",");
    format!("{{\"checkpoints\":[{entries}]}}")
}

/// The paths one checkpoint covers, so the UI can offer to restore a single
/// file. Path and origin only — a hash here would be noise, and content would
/// be a leak.
fn checkpoint_files_json(files: &[workspace_engine::CheckpointFile]) -> String {
    files
        .iter()
        .map(|file| {
            format!(
                "{{\"path\":\"{}\",\"origin\":\"{}\"}}",
                escape_json(&file.path),
                match file.origin {
                    workspace_engine::CheckpointOrigin::Patch => "patch",
                    workspace_engine::CheckpointOrigin::Command => "command",
                }
            )
        })
        .collect::<Vec<_>>()
        .join(",")
}

fn checkpoint_pending_approvals_json(approvals: &[workspace_engine::PendingApproval]) -> String {
    approvals
        .iter()
        .map(|approval| {
            format!(
                "{{\"kind\":\"{}\",\"proposalId\":\"{}\"}}",
                escape_json(&approval.kind),
                escape_json(&approval.proposal_id)
            )
        })
        .collect::<Vec<_>>()
        .join(",")
}

/// Skipped and conflicted stay apart all the way to the UI: "there was nothing
/// to do" and "the file changed under you" call for different next steps.
fn checkpoint_restore_json(result: &workspace_engine::CheckpointRestoreResult) -> String {
    format!(
        "{{\"checkpointId\":\"{}\",\"restoredFiles\":[{}],\"deletedFiles\":[{}],\"skippedFiles\":[{}],\"conflictedFiles\":[{}],\"conversationRestored\":{},\"warnings\":[{}]}}",
        escape_json(&result.checkpoint_id),
        json_string_array(&result.restored_files),
        json_string_array(&result.deleted_files),
        json_string_array(&result.skipped_files),
        json_string_array(&result.conflicted_files),
        result.conversation_restored,
        json_string_array(&result.warnings)
    )
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
        browser_report_from_tool_result, cached_model_api_key, chat_engine, checkpoint_list_json,
        checkpoint_restore_json, config_and_report_for_repo, default_engine,
        desktop_settings_config_path, effective_policy_for_repo, engine_for_repo,
        forget_model_api_key, generated_secret_warnings_json, handle_connection, index_html,
        json_error, json_optional_string, keychain, language_servers_json, mcp_browser_arguments,
        message_json, parse_form, parse_path_list, percent_decode, phase_json, plan_json,
        plan_proposal_json, question_proposal_json, relay_turn_events, remember_model_api_key,
        render_markdown_with_optional_file_links, repository_config_review_json,
        repository_config_write_target, require_api_token, resolve_shell_model_api_key,
        run_answer_question_request, run_server, run_terminal_command, save_config_file,
        spawn_with_shell_stack, sweep_orphaned_processes, symbol_repository, task_states_json,
        task_usage_json, terminal_cwd_for_repo, turn_progress_event, validate_context_files,
        validate_working_folder, validate_workspace_path, verify_data_dir_schema_at,
        web_diagnostic_json, web_diagnostic_reveal_target, write_basic_response, write_sse_event,
    };
    use std::collections::{BTreeMap, HashMap};
    use std::fs;
    use std::io::{Read, Write};
    use std::net::{TcpListener, TcpStream};
    use std::path::{Path, PathBuf};
    use std::sync::OnceLock;
    use std::time::{Duration, SystemTime, UNIX_EPOCH};
    use workspace_engine::CheckpointRestoreResult;
    use workspace_engine::{
        AgentPlanProposal, AgentQuestionProposal, AskUserInput, CancelToken, ChatMessage,
        ChatTurnOptions, ChatTurnResult, Config, Evidence, GeneratedSecretWarning,
        LanguageServerHub, LanguageServerLaunch, LanguageServerState, LanguageServerStatus,
        MockModelAdapter, PendingQuestionRef, PhaseKind, PlanStep, SecretScanner, SessionMode,
        StepStatus, TaskPlan, TaskStatus, TaskUsage, ToolCall, TurnNotes, TurnPhase, TurnProgress,
        TurnSink, UsageSource, WebDiagnosticRecord, WebDiagnosticReport, WorkspaceEngine,
        finding::{Finding, FindingDraft, FindingSource, Severity, SourceRange},
    };

    /// Points every engine built in this test binary at a throwaway data
    /// directory, and returns it.
    ///
    /// Repository config cannot set `data_dir` — spec 34 makes it forbidden, so
    /// a repository that arrives with a clone cannot redirect where data lands.
    /// That leaves the environment as the only lever a test has, and without it
    /// these tests write sessions, patches, and checkpoints straight into the
    /// user's real `~/Library/Application Support/DamaianClient`.
    fn isolated_data_dir() -> &'static std::path::Path {
        static DATA_DIR: OnceLock<PathBuf> = OnceLock::new();
        DATA_DIR.get_or_init(|| {
            let dir = std::env::temp_dir().join(format!(
                "damaian-shell-test-data-{}",
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            fs::create_dir_all(&dir).expect("test data dir");
            // Set once, inside the initializer, before any engine in this
            // binary is built: a concurrent test waits here rather than
            // racing, and a test binary shares its environment with nothing.
            unsafe { std::env::set_var("DAMAIAN_DATA_DIR", &dir) };
            dir
        })
    }

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

    /// A plan mid-turn: one step confirmed by a command, one still running.
    fn sample_plan() -> TaskPlan {
        let mut plan = TaskPlan::new("task_1", 7);
        plan.steps.push(PlanStep {
            id: "step_1".to_string(),
            title: "Read the retry helper".to_string(),
            detail: None,
            status: StepStatus::Completed,
            depends_on: Vec::new(),
            started_at_ms: Some(7),
            completed_at_ms: Some(8),
            evidence: vec![Evidence::CommandExit {
                marker_id: "action_1".to_string(),
                exit_code: Some(0),
                purpose: workspace_engine::CommandPurpose::Other,
            }],
        });
        plan.steps.push(PlanStep {
            id: "step_2".to_string(),
            title: "Add a bounded backoff".to_string(),
            detail: None,
            status: StepStatus::InProgress,
            depends_on: Vec::new(),
            started_at_ms: Some(8),
            completed_at_ms: None,
            evidence: Vec::new(),
        });
        plan
    }

    #[test]
    fn the_plan_event_carries_each_step_with_its_status_and_evidence() {
        // The panel has to distinguish a step confirmed by something Damaian
        // watched from one merely claimed, so the evidence has to cross the
        // wire — a status alone cannot tell the two apart (§5.3, §5.6).
        let (sender, receiver) = std::sync::mpsc::channel();
        let cancel = CancelToken::new();
        std::thread::spawn(move || {
            let _ = sender.send(TurnEvent::Plan {
                plan: Box::new(sample_plan()),
                active: true,
            });
        });

        let mut out = FlakyWriter {
            written: Vec::new(),
            writes_before_failure: usize::MAX,
        };
        relay_turn_events(&mut out, &cancel, receiver, Duration::from_millis(25)).expect("relay");

        let text = String::from_utf8_lossy(&out.written);
        assert!(text.contains("event: plan"), "got {text:?}");
        assert!(text.contains("\"title\":\"Read the retry helper\""));
        assert!(text.contains("\"status\":\"completed\""));
        assert!(text.contains("\"status\":\"in_progress\""));
        assert!(
            text.contains("\"kind\":\"commandExit\""),
            "the evidence must reach the panel, not just the status: {text:?}"
        );
        // And the outcome the report will print, so the panel does not have to
        // re-derive "completed with nothing behind it" for itself.
        assert!(text.contains("\"outcome\":\"verified\""), "got {text:?}");
        assert!(text.contains("\"outcome\":\"outstanding\""), "got {text:?}");
    }

    #[test]
    fn an_ended_turn_s_plan_is_not_active() {
        let mut plans = HashMap::new();
        plans.insert("task_1".to_string(), sample_plan());
        let mut statuses = HashMap::new();
        statuses.insert("task_1".to_string(), "complete".to_string());

        // `sample_plan` has one completed step and one in progress, so an ended
        // turn must say the second was never marked done rather than "running".
        let json = task_states_json(
            &statuses,
            &HashMap::new(),
            &plans,
            &HashMap::new(),
            &HashMap::new(),
            &BTreeMap::new(),
            &Config::default(),
        );
        assert!(json.contains("\"active\":false"), "{json}");
        assert!(
            json.contains("Turn ended with 1 step not marked done"),
            "{json}"
        );

        statuses.insert("task_1".to_string(), "running_tool".to_string());
        let json = task_states_json(
            &statuses,
            &HashMap::new(),
            &plans,
            &HashMap::new(),
            &HashMap::new(),
            &BTreeMap::new(),
            &Config::default(),
        );
        assert!(json.contains("\"active\":true"), "{json}");
        assert!(json.contains("1 step outstanding"), "{json}");
    }

    #[test]
    fn the_reminder_is_a_tagged_system_line_in_a_reloaded_session() {
        // The conversation view needs the tag to render the engine's nudge as a
        // system line rather than as something the user typed (spec 66 §5.5).
        let tagged = ChatMessage {
            id: "msg_1".to_string(),
            session_id: "session_1".to_string(),
            task_id: Some("task_1".to_string()),
            role: "user".to_string(),
            content: "Your plan still has open steps.".to_string(),
            kind: Some("plan_reminder".to_string()),
            created_at_ms: 5,
        };
        let json = message_json(1, &tagged, None);
        assert!(json.contains("\"kind\":\"plan_reminder\""), "{json}");

        let ordinary = ChatMessage {
            id: "msg_2".to_string(),
            session_id: "session_1".to_string(),
            task_id: None,
            role: "user".to_string(),
            content: "hello".to_string(),
            kind: None,
            created_at_ms: 6,
        };
        let json = message_json(2, &ordinary, None);
        assert!(!json.contains("\"kind\""), "{json}");
    }

    #[test]
    fn the_plan_event_carries_whether_the_turn_is_active() {
        match turn_progress_event(TurnProgress::Plan(sample_plan())) {
            Some(TurnEvent::Plan { active, .. }) => assert!(active),
            _ => panic!("the plan update must become a live plan event"),
        }
        match turn_progress_event(TurnProgress::PlanFinished(sample_plan())) {
            Some(TurnEvent::Plan { active, .. }) => assert!(!active),
            _ => panic!("the finished plan must become a live plan event"),
        }
    }

    #[test]
    fn a_plan_with_two_steps_in_progress_says_so_rather_than_picking_one() {
        // Requirement 2 is an invariant, and a panel that quietly rendered the
        // first of two in-progress steps would hide exactly the bug the
        // invariant exists to catch.
        let mut plan = sample_plan();
        plan.steps[0].status = StepStatus::InProgress;
        assert!(plan.violates_single_in_progress());

        let json = plan_json(&plan, true);
        assert!(
            json.contains("\"violatesSingleInProgress\":true"),
            "got {json}"
        );

        let json = plan_json(&sample_plan(), true);
        assert!(json.contains("\"violatesSingleInProgress\":false"));
    }

    #[test]
    fn a_blocked_plan_is_never_summarised_as_complete_over_the_wire() {
        let mut plan = sample_plan();
        plan.steps[1].status = StepStatus::Blocked;

        let json = plan_json(&plan, true);
        assert!(json.contains("\"isComplete\":false"), "got {json}");
        assert!(
            !json.to_lowercase().contains("\"summary\":\"complete"),
            "got {json}"
        );
        assert!(json.contains("\"outcome\":\"blocked\""));
    }

    #[test]
    fn the_chat_result_carries_a_question_put_up_for_an_answer() {
        // Spec 50 requirement 3, Task 6: the live card needs the question and
        // its options, and the payload must give no approval route anything to
        // act on. A `proposalId` here is how a question would become an
        // approval in the client.
        let json = question_proposal_json(Some(&AgentQuestionProposal {
            question_id: "question_7f".to_string(),
            question: "Did you mean the CLI or the shell?".to_string(),
            options: vec!["The CLI".to_string(), "The shell".to_string()],
            allow_free_text: true,
        }));
        let value: serde_json::Value = serde_json::from_str(&json).expect("question JSON");
        assert_eq!(value["questionId"], "question_7f");
        assert_eq!(value["question"], "Did you mean the CLI or the shell?");
        assert_eq!(value["options"][1], "The shell");
        assert_eq!(value["allowFreeText"], true);
        assert!(
            value.get("proposalId").is_none(),
            "a question has no proposal id to approve: {value}"
        );
        assert!(
            value.get("requiresApproval").is_none(),
            "a question is not an approval: {value}"
        );
        assert_eq!(question_proposal_json(None), "null");
    }

    fn form_fields(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(key, value)| (key.to_string(), value.to_string()))
            .collect()
    }

    /// A repository whose engine loads the isolated test data directory, which
    /// is the same directory a shell route reads when it builds its own engine.
    fn question_repo(name: &str) -> PathBuf {
        isolated_data_dir();
        let repo = temp_path(name);
        fs::create_dir_all(&repo).expect("repository");
        fs::write(repo.join("README.md"), "# Questions\n").expect("readme");
        repo
    }

    fn engine_for_question_test(repo: &std::path::Path) -> WorkspaceEngine {
        let (mut config, report) =
            config_and_report_for_repo(repo.to_str().expect("repository path")).expect("config");
        // Fixtures are indexed once and discarded; the macOS watcher buys them
        // nothing and costs startup time.
        config.enable_index_watcher = false;
        // The test must not inherit a cap of 0 from the developer's own config.
        config.agent_max_questions_per_turn = 2;
        // `ask_user` is offered only through native tools. The developer's own
        // config may name a provider that does not support them, so the fixture
        // pins one that does rather than depending on the machine.
        let provider_id = config.model_provider.clone();
        let model = config.model_name.clone();
        config
            .model_providers
            .retain(|provider| provider.id != provider_id);
        config
            .model_providers
            .push(workspace_engine::ModelProviderConfig {
                id: provider_id.clone(),
                label: provider_id.clone(),
                base_url: "https://example.test".to_string(),
                api_key_env: "DAMAIAN_TEST_KEY".to_string(),
                models: vec![model],
                supports_native_tools: true,
                max_output_tokens: None,
                context_token_budget: None,
                provider_reports_usage: true,
                price_per_million_input_tokens: None,
                price_per_million_output_tokens: None,
                price_per_million_cached_input_tokens: None,
                supports_explicit_cache_breakpoints: false,
            });
        assert!(config.supports_native_tools());
        chat_engine((config, report))
    }

    fn turn_sink<'a>(
        cancel: &'a CancelToken,
        on_token: &'a mut dyn FnMut(&str),
        on_progress: &'a mut dyn FnMut(TurnProgress),
    ) -> TurnSink<'a> {
        TurnSink {
            on_token,
            on_progress,
            cancel,
            steering: None,
        }
    }

    /// A turn paused on a command that needs approval. The command is the
    /// marker file every refusal test looks for.
    fn paused_on_command(repo: &std::path::Path) -> ChatTurnResult {
        let engine = engine_for_question_test(repo);
        let mut adapter = MockModelAdapter::new_sequence_with_tool_calls(
            vec![String::new()],
            vec![vec![ToolCall {
                id: "call_marker".to_string(),
                name: "run_command".to_string(),
                arguments_json:
                    r#"{"command":"touch should-not-run.txt","reason":"Write a marker file."}"#
                        .to_string(),
            }]],
        );
        let cancel = CancelToken::new();
        let mut on_token = |_token: &str| {};
        let mut on_progress = |_event: TurnProgress| {};
        let mut sink = turn_sink(&cancel, &mut on_token, &mut on_progress);
        let result = engine
            .chat_orchestrator
            .ask_with_session_with_options(
                repo,
                "Write the marker",
                &[],
                None,
                &mut adapter,
                &mut sink,
                ChatTurnOptions {
                    user_present: true,
                    ..ChatTurnOptions::default()
                },
            )
            .expect("the command pauses for approval");
        assert_eq!(result.task.status, TaskStatus::WaitingForApproval);
        result
    }

    /// A turn paused on the model's question.
    fn paused_on_question(repo: &std::path::Path) -> ChatTurnResult {
        let engine = engine_for_question_test(repo);
        let mut adapter = MockModelAdapter::new_sequence_with_tool_calls(
            vec![String::new()],
            vec![vec![ToolCall {
                id: "call_question".to_string(),
                name: "ask_user".to_string(),
                arguments_json:
                    r#"{"question":"Did you mean the CLI or the shell?","options":["The CLI","The shell"],"allowFreeText":true}"#
                        .to_string(),
            }]],
        );
        let cancel = CancelToken::new();
        let mut on_token = |_token: &str| {};
        let mut on_progress = |_event: TurnProgress| {};
        let mut sink = turn_sink(&cancel, &mut on_token, &mut on_progress);
        let result = engine
            .chat_orchestrator
            .ask_with_session_with_options(
                repo,
                "Which one?",
                &[],
                None,
                &mut adapter,
                &mut sink,
                ChatTurnOptions {
                    user_present: true,
                    ..ChatTurnOptions::default()
                },
            )
            .expect("the turn pauses on a question");
        assert_eq!(result.task.status, TaskStatus::WaitingForApproval);
        assert!(result.question_proposal.is_some());
        result
    }

    #[test]
    fn a_reopened_session_carries_the_question_and_answer_messages_in_order() {
        // Requirement 3's reload half. `read_messages` already yields the two
        // roles; the card also needs the options it was asked with, which the
        // role alone cannot carry.
        isolated_data_dir();
        let engine = default_engine().expect("default engine");
        let session = engine
            .session_store
            .create_session("repo_question_transcript", "question transcript")
            .expect("session");
        let question = PendingQuestionRef::new();
        engine
            .session_store
            .record_question_asked(
                &session.id,
                "task_question",
                &question,
                &AskUserInput {
                    question: "Did you mean the CLI or the shell?".to_string(),
                    options: vec!["The CLI".to_string(), "The shell".to_string()],
                    allow_free_text: true,
                },
            )
            .expect("question recorded");
        engine
            .session_store
            .record_question_answered(&session.id, question.question_id(), "The CLI", Some(0))
            .expect("answer recorded");

        let (port, token) = serve_for_test();
        let response = get_session_for_test(port, &token, &session.id);
        assert!(response.starts_with("HTTP/1.1 200"), "{response}");
        let payload = json_of(&response);
        let messages = payload["messages"].as_array().expect("messages");
        let roles: Vec<&str> = messages
            .iter()
            .map(|message| message["role"].as_str().expect("role"))
            .collect();
        assert_eq!(
            roles,
            vec!["question", "answer"],
            "a reload must show the exchange in the order it happened: {payload}"
        );
        assert_eq!(messages[0]["id"], question.question_id());
        assert_eq!(messages[0]["content"], "Did you mean the CLI or the shell?");
        assert_eq!(messages[0]["options"][0], "The CLI");
        assert_eq!(messages[0]["options"][1], "The shell");
        assert_eq!(messages[0]["allowFreeText"], true);
        assert_eq!(messages[1]["content"], "The CLI");
    }

    #[test]
    fn the_answer_route_refuses_a_command_proposal_id() {
        // Requirement 2 at the shell's answer route: a command proposal id can
        // never be answered as though an answer were consent. Nothing runs and
        // the command is still waiting for its own decision.
        let repo = question_repo("answer-route-command-id");
        let paused = paused_on_command(&repo);
        let proposal_id = paused
            .command_proposal
            .as_ref()
            .expect("a command proposal")
            .id
            .clone();
        let (events, _receiver) = std::sync::mpsc::channel::<TurnEvent>();
        let error = run_answer_question_request(
            &form_fields(&[
                ("repo", repo.to_str().expect("repository path")),
                ("question_id", &proposal_id),
                ("answer", "Yes"),
                ("selected_option", ""),
            ]),
            &CancelToken::new(),
            &events,
        )
        .expect_err("a command proposal id is not answerable");
        assert!(
            error.contains("not a question"),
            "the route must say what is wrong: {error}"
        );
        assert!(
            !repo.join("should-not-run.txt").exists(),
            "answering a command id must run nothing"
        );
        let engine = engine_for_repo(repo.to_str().expect("repository path")).expect("engine");
        assert!(
            engine
                .chat_orchestrator
                .has_pending_chat_command(&proposal_id),
            "the command is still waiting for its own decision"
        );
        fs::remove_dir_all(repo).ok();
    }

    #[test]
    fn run_command_with_a_question_id_runs_nothing() {
        // Requirement 2's other direction: a question id reaching the command
        // route is not an approval, and there is no proposal behind it to run.
        let repo = question_repo("run-command-question-id");
        let paused = paused_on_question(&repo);
        let question_id = paused
            .question_proposal
            .as_ref()
            .expect("a question")
            .question_id
            .clone();
        let (port, token) = serve_for_test();
        let response = post_fields_for_test(
            port,
            &token,
            "/api/run-command",
            &[
                ("repo", repo.to_str().expect("repository path")),
                ("proposal_id", &question_id),
            ],
        );
        assert!(
            !response.starts_with("HTTP/1.1 200"),
            "a question id is not a command proposal: {response}"
        );
        assert!(
            !repo.join("should-not-run.txt").exists(),
            "nothing may run for a question id"
        );
        let engine = engine_for_repo(repo.to_str().expect("repository path")).expect("engine");
        assert!(
            !engine
                .chat_orchestrator
                .has_pending_chat_command(&question_id),
            "a question is never a pending command"
        );
        fs::remove_dir_all(repo).ok();
    }

    #[test]
    fn the_chat_result_carries_a_plan_put_up_for_review() {
        // Task 11's gate stops the turn with a `plan_proposal`; without this
        // field the frontend has no way to know it happened, and the turn
        // reads as an ordinary answer that mysteriously did nothing.
        let json = plan_proposal_json(Some(&AgentPlanProposal {
            id: "planreview_1".to_string(),
            plan: sample_plan(),
            deferred_action: "Preparing a patch".to_string(),
        }));
        assert!(
            json.contains("\"proposalId\":\"planreview_1\""),
            "got {json}"
        );
        assert!(json.contains("\"deferredAction\":\"Preparing a patch\""));
        assert!(json.contains("\"title\":\"Add a bounded backoff\""));
        assert_eq!(plan_proposal_json(None), "null");
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

    /// Answers every diagnostic with a companion report whose screenshot is a
    /// real file under `/…/runs/`, shaped by the same function the MCP runner
    /// uses, so the test sees exactly what the engine would.
    struct CompanionShapedRunner {
        text: String,
        data_dir: PathBuf,
    }

    impl workspace_engine::WebDiagnosticsRunner for CompanionShapedRunner {
        fn inspect(
            &self,
            call: &workspace_engine::WebDiagnosticCall,
        ) -> workspace_engine::Result<workspace_engine::WebDiagnosticReport> {
            Ok(browser_report_from_tool_result(
                workspace_engine::WebDiagnosticReport::from_text(self.text.clone(), false),
                call,
                &self.data_dir,
                "MCP server `browser` tool `inspect_page`",
            ))
        }

        fn run_scenario(
            &self,
            call: &workspace_engine::WebDiagnosticCall,
        ) -> workspace_engine::Result<workspace_engine::WebDiagnosticReport> {
            self.inspect(call)
        }
    }

    #[test]
    fn a_structured_browser_report_is_rendered_and_lists_the_materialised_artifact() {
        let repo = temp_path("browser-artifact-e2e");
        let runs = repo.join("runs");
        fs::create_dir_all(&runs).unwrap();
        fs::write(repo.join("README.md"), "# Web app\n").unwrap();
        let screenshot = runs.join("20260925-1-page.png");
        fs::write(&screenshot, b"\x89PNG\r\n\x1a\n").unwrap();
        let screenshot = screenshot.to_string_lossy().to_string();
        let text = format!(
            r#"{{"final_url": "http://localhost:5001/", "title": "Snake Game", "status": 200,
                "page_errors": ["ReferenceError: boom"],
                "artifacts": ["{screenshot}"],
                "artifact_metadata": [{{"kind": "screenshot", "path": "{screenshot}",
                  "mime_type": "image/png", "width": 1280, "height": 720}}]}}"#
        );

        let mut config = Config {
            data_dir: isolated_data_dir().to_path_buf(),
            enable_index_watcher: false,
            ..Config::default()
        };
        config
            .model_providers
            .push(workspace_engine::ModelProviderConfig {
                id: "openai".to_string(),
                label: "OpenAI".to_string(),
                base_url: String::new(),
                api_key_env: String::new(),
                models: Vec::new(),
                supports_native_tools: true,
                max_output_tokens: None,
                context_token_budget: None,
                provider_reports_usage: true,
                price_per_million_input_tokens: None,
                price_per_million_output_tokens: None,
                price_per_million_cached_input_tokens: None,
                supports_explicit_cache_breakpoints: false,
            });
        let mut engine = WorkspaceEngine::new(config);
        engine.chat_orchestrator.set_web_diagnostics_runner(
            workspace_engine::WebDiagnosticsRunnerHandle::new(CompanionShapedRunner {
                text,
                data_dir: isolated_data_dir().to_path_buf(),
            }),
        );
        let mut adapter = MockModelAdapter::new_sequence_with_tool_calls(
            vec![String::new(), "Found it.".to_string()],
            vec![
                vec![ToolCall {
                    id: "call_1".to_string(),
                    name: "inspect_web_page".to_string(),
                    arguments_json: r#"{"url":"http://localhost:5001/"}"#.to_string(),
                }],
                Vec::new(),
            ],
        );
        let mut on_token = |_token: &str| {};
        let session_id = engine
            .chat_orchestrator
            .ask(&repo, "Inspect the page.", &[], &mut adapter, &mut on_token)
            .unwrap()
            .session
            .id;

        let result = engine
            .session_store
            .read_messages(&session_id)
            .unwrap()
            .into_iter()
            .find(|message| message.role == "tool")
            .expect("the diagnostic's tool result")
            .content;
        assert!(
            result.starts_with("Browser diagnostic found 1 page error."),
            "{result}"
        );
        assert!(
            !result.contains("Browser diagnostic result via"),
            "{result}"
        );
        assert!(
            result.contains("- Source: MCP server `browser` tool `inspect_page`"),
            "{result}"
        );
        let listed = result
            .split("\n\nArtifacts:\n")
            .nth(1)
            .unwrap_or_else(|| panic!("an Artifacts: list in {result}"));
        let prefix = format!("- screenshot: web-diagnostics/{session_id}/");
        assert!(listed.starts_with(&prefix), "{listed}");
        assert!(
            listed.ends_with("/20260925-1-page.png (1280x720)"),
            "{listed}"
        );
        assert!(
            !result.contains(&screenshot),
            "the companion's path leaked: {result}"
        );

        fs::remove_dir_all(&repo).ok();
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
        spawn_with_shell_stack("test-shell", move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { break };
                let _ = handle_connection(&mut stream, &options);
            }
        })
        .expect("spawn the test shell");

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

    /// Actually launches Finder, so this is excluded from normal `cargo
    /// test` runs, like `reveal_in_finder_endpoint_opens_the_requested_repository_root`.
    /// Run manually with `cargo test -p desktop-shell -- --ignored
    /// reveal_web_diagnostic_artifact_endpoint_selects_the_file_in_finder` to
    /// verify end-to-end. It writes only under the isolated temp data dir.
    #[test]
    #[ignore]
    fn reveal_web_diagnostic_artifact_endpoint_selects_the_file_in_finder() {
        let data_dir = isolated_data_dir();
        let relative = "web-diagnostics/session_reveal/task_reveal/run-1/page.png";
        let file = data_dir.join(relative);
        fs::create_dir_all(file.parent().unwrap()).unwrap();
        fs::write(&file, b"png").unwrap();
        let (port, token) = serve_for_test();

        let repo = std::env::temp_dir();
        let body = format!("repo={}&path={relative}", repo.to_string_lossy());
        let response = send_for_test(
            port,
            format!(
                "POST /api/reveal-web-diagnostic-artifact HTTP/1.1\r\nHost: 127.0.0.1\r\ncontent-type: application/x-www-form-urlencoded\r\nx-damaian-api-token: {token}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                body.len()
            ),
        );

        assert!(
            response.starts_with("HTTP/1.1 200"),
            "unexpected response: {response}"
        );
        assert!(response.contains("page.png"), "{response}");
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
        isolated_data_dir();
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
        spawn_with_shell_stack("test-shell", move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { break };
                let _ = handle_connection(&mut stream, &options);
            }
        })
        .expect("spawn the test shell");

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

    /// The done event of an SSE turn, whichever `data:` line carries it.
    fn sse_done(response: &str) -> serde_json::Value {
        for line in response.lines() {
            let Some(data) = line.strip_prefix("data: ") else {
                continue;
            };
            let Ok(value) = serde_json::from_str::<serde_json::Value>(data) else {
                continue;
            };
            if value.get("response").is_some() && value.get("sessionId").is_some() {
                return value;
            }
        }
        panic!("no done event in response: {response}");
    }

    /// Creates a session whose patch is applied and therefore verification is
    /// due, and returns the session id.
    fn due_session(repo: &std::path::Path, changes: &[(&str, &str)]) -> String {
        let repo_arg = repo.to_string_lossy().to_string();
        let engine = engine_for_repo(&repo_arg).expect("engine for test repo");
        for (path, before) in changes {
            let file = repo.join(path);
            fs::create_dir_all(file.parent().unwrap()).unwrap();
            fs::write(file, before).unwrap();
        }
        let session = engine
            .session_store
            .create_session("shell-test", "verification")
            .unwrap();
        let task = engine
            .session_store
            .create_task(&session.id, "patch", "provider", "model")
            .unwrap();
        let proposed = changes
            .iter()
            .map(|(path, _)| workspace_engine::ProposedChange {
                path: (*path).to_string(),
                new_content: format!("// fixed {path}\n"),
                status: None,
                allow_restricted: false,
            })
            .collect::<Vec<_>>();
        let mut patch = engine
            .patch_engine
            .create_patch(repo, &proposed, Some(&task.id), "test change")
            .unwrap();
        patch.session_id = session.id.clone();
        engine.patch_store.save(&patch).unwrap();
        engine
            .edit_orchestrator
            .apply_stored_patch(repo, &patch.id, None, None, "tester", false)
            .unwrap();
        session.id
    }

    fn temp_shell_repo(name: &str) -> std::path::PathBuf {
        let repo = std::env::temp_dir().join(format!(
            "damaian-{name}-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(repo.join(".damaian")).unwrap();
        isolated_data_dir();
        repo
    }

    #[test]
    fn verify_endpoint_reports_no_checks_available() {
        // The route builds an adapter before the engine decides nothing is due.
        unsafe { std::env::set_var("DAMAIAN_MOCK_MODEL_RESPONSE", "No repair.") };
        let repo = temp_shell_repo("verify-endpoint");
        let session = due_session(&repo, &[("src/a.rs", "fn a() {}\n")]);
        let (port, token) = serve_for_test();
        let repo_arg = repo.to_string_lossy().to_string();

        let response = post_fields_for_test(
            port,
            &token,
            "/api/verify-stream",
            &[("repo", &repo_arg), ("session_id", &session)],
        );

        assert!(response.starts_with("HTTP/1.1 200"), "{response}");
        let done = sse_done(&response);
        let report = &done["verificationReport"];
        assert_eq!(report["outcome"], "no_checks_available", "{done}");
        assert!(
            done["response"]
                .as_str()
                .unwrap_or_default()
                .contains("No checks were available"),
            "{done}"
        );

        fs::remove_dir_all(&repo).ok();
    }

    #[test]
    fn resume_checks_endpoint_records_declined_checks() {
        unsafe { std::env::set_var("DAMAIAN_MOCK_MODEL_RESPONSE", "No repair.") };
        let repo = temp_shell_repo("resume-checks-endpoint");
        fs::create_dir_all(repo.join("packages/web/src")).unwrap();
        fs::write(repo.join("Cargo.toml"), "[workspace]\nmembers = []\n").unwrap();
        fs::write(
            repo.join("packages/web/package.json"),
            "{\"scripts\":{\"test\":\"node test.js\"}}\n",
        )
        .unwrap();
        let session = due_session(
            &repo,
            &[
                ("src/a.rs", "fn a() {}\n"),
                ("packages/web/src/index.ts", "export {};\n"),
            ],
        );
        let (port, token) = serve_for_test();
        let repo_arg = repo.to_string_lossy().to_string();

        let paused = post_fields_for_test(
            port,
            &token,
            "/api/verify-stream",
            &[("repo", &repo_arg), ("session_id", &session)],
        );
        let paused = sse_done(&paused);
        let checks = paused["checkBatch"]["checks"]
            .as_array()
            .expect("both checks need approval");
        assert_eq!(checks.len(), 2, "{paused}");
        let decisions = checks
            .iter()
            .map(|check| {
                serde_json::json!({
                    "proposalId": check["proposalId"],
                    "approved": false,
                })
            })
            .collect::<Vec<_>>();
        let decisions = serde_json::Value::Array(decisions).to_string();

        let resumed = post_fields_for_test(
            port,
            &token,
            "/api/resume-checks-stream",
            &[
                ("repo", &repo_arg),
                (
                    "proposal_id",
                    paused["checkBatch"]["proposalId"]
                        .as_str()
                        .expect("batch id"),
                ),
                ("decisions", &decisions),
            ],
        );

        assert!(resumed.starts_with("HTTP/1.1 200"), "{resumed}");
        let done = sse_done(&resumed);
        let report = &done["verificationReport"];
        assert_eq!(report["outcome"], "passed", "{done}");
        let skipped = report["skipped"].as_array().expect("skipped checks");
        let declined = skipped
            .iter()
            .filter(|item| item["reason"]["reason"] == "declined_by_user")
            .count();
        assert_eq!(declined, 2, "{done}");
        assert!(
            skipped.iter().all(|item| {
                let reason = &item["reason"]["reason"];
                reason == "declined_by_user" || reason == "no_running_target"
            }),
            "{done}"
        );

        fs::remove_dir_all(&repo).ok();
    }

    /// Spec 32 Task 6: a `before_patch_apply` hook's denial is the error text
    /// the UI already shows, so its reason reaches the user, and a warning is
    /// returned beside the applied files.
    #[test]
    fn apply_patch_endpoint_reports_hook_denials_and_warnings() {
        let repo = std::env::temp_dir().join(format!(
            "damaian-apply-hook-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(repo.join(".damaian")).unwrap();
        fs::write(repo.join("notes.txt"), "one\n").unwrap();

        let data_dir = isolated_data_dir().to_path_buf();
        let script = data_dir.join("patch-hook.sh");
        let deny = "cat <<'JSON'\n{\"hookApiVersion\":1,\"verdict\":\"deny\",\"reason\":\"migrations/ is applied by the release process\"}\nJSON\n";
        fs::write(&script, deny).unwrap();
        let user_config = data_dir.join("config").join("user.conf");
        fs::create_dir_all(user_config.parent().unwrap()).unwrap();
        let hook_line = format!(
            "hook.no_migrations.command=/bin/sh\nhook.no_migrations.args={}\nhook.no_migrations.events=before_patch_apply\n",
            script.to_string_lossy()
        );
        fs::write(&user_config, &hook_line).unwrap();

        let repo_arg = repo.to_string_lossy().to_string();
        let engine = engine_for_repo(&repo_arg).expect("engine for test repo");
        let patch = engine
            .patch_engine
            .create_patch(
                &repo,
                &[workspace_engine::ProposedChange {
                    path: "notes.txt".to_string(),
                    new_content: "two\n".to_string(),
                    status: None,
                    allow_restricted: false,
                }],
                None,
                "hook apply",
            )
            .expect("create patch");
        engine.patch_store.save(&patch).expect("store patch");

        let (port, token) = serve_for_test();
        let fields = [("repo", repo_arg.as_str()), ("patch_id", patch.id.as_str())];
        let denied = post_fields_for_test(port, &token, "/api/apply-patch", &fields);
        assert!(denied.starts_with("HTTP/1.1 500"), "{denied}");
        assert!(denied.contains("no_migrations"), "{denied}");
        assert!(denied.contains("release process"), "{denied}");
        assert_eq!(
            fs::read_to_string(repo.join("notes.txt")).unwrap(),
            "one\n",
            "a denied apply wrote the file"
        );

        let warn = "cat <<'JSON'\n{\"hookApiVersion\":1,\"verdict\":\"warn\",\"reason\":\"the patch was checked by the hook\"}\nJSON\n";
        fs::write(&script, warn).unwrap();
        let applied = post_fields_for_test(port, &token, "/api/apply-patch", &fields);
        let _ = fs::remove_file(&user_config);
        assert!(applied.starts_with("HTTP/1.1 200"), "{applied}");
        assert!(
            applied.contains("\"hookWarnings\":[\"the patch was checked by the hook\"]"),
            "{applied}"
        );
        assert_eq!(fs::read_to_string(repo.join("notes.txt")).unwrap(), "two\n");

        fs::remove_dir_all(&repo).ok();
    }

    /// The two rewind routes over real HTTP, because the UI reaches them that
    /// way and a route that compiles is not a route that answers.
    #[test]
    fn checkpoint_routes_list_and_rewind_over_http() {
        let repo = std::env::temp_dir().join(format!(
            "damaian-rewind-route-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(repo.join(".damaian")).unwrap();
        isolated_data_dir();
        fs::write(repo.join("app.js"), "export const a = 1;\n").unwrap();
        fs::write(repo.join("other.js"), "export const b = 1;\n").unwrap();

        let repo_arg = repo.to_string_lossy().to_string();
        let engine = engine_for_repo(&repo_arg).expect("engine for test repo");
        let manifest = engine
            .checkpoint_store
            .create_checkpoint(
                &repo,
                workspace_engine::CheckpointRequest {
                    session_id: "session_route",
                    task_id: Some("task_route"),
                    user_message_id: Some("msg_route"),
                    summary: "Before: bump the constant",
                    conversation: workspace_engine::CheckpointConversation {
                        last_event_seq: 1,
                        task_status: "running".to_string(),
                    },
                    pending_approvals: Vec::new(),
                    paths: vec![
                        workspace_engine::CheckpointPath {
                            path: "app.js".to_string(),
                            origin: workspace_engine::CheckpointOrigin::Patch,
                        },
                        workspace_engine::CheckpointPath {
                            path: "other.js".to_string(),
                            origin: workspace_engine::CheckpointOrigin::Patch,
                        },
                    ],
                },
            )
            .expect("create checkpoint");
        fs::write(repo.join("app.js"), "export const a = 2;\n").unwrap();
        fs::write(repo.join("other.js"), "export const b = 2;\n").unwrap();
        engine
            .checkpoint_store
            .seal_checkpoint(&repo, &manifest)
            .expect("seal checkpoint");

        let options = ShellOptions::new(0, None);
        let token = options.api_token.clone();
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind test listener");
        let port = listener.local_addr().expect("local addr").port();
        spawn_with_shell_stack("test-shell", move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { break };
                let _ = handle_connection(&mut stream, &options);
            }
        })
        .expect("spawn the test shell");

        let send = |request: String| {
            let mut stream =
                TcpStream::connect(("127.0.0.1", port)).expect("connect to test server");
            stream.write_all(request.as_bytes()).expect("write request");
            let mut response = String::new();
            stream.read_to_string(&mut response).expect("read response");
            response
        };

        let listed = send(format!(
            "GET /api/checkpoints?repo={}&session_id=session_route HTTP/1.1\r\nHost: 127.0.0.1\r\nx-damaian-api-token: {token}\r\nconnection: close\r\n\r\n",
            percent_encode_for_test(&repo_arg)
        ));
        assert!(listed.contains(&manifest.checkpoint_id), "{listed}");
        assert!(listed.contains("\"fileCount\":2"), "{listed}");
        assert!(listed.contains("Before: bump the constant"), "{listed}");
        assert!(
            listed.contains("\"path\":\"app.js\",\"origin\":\"patch\""),
            "{listed}"
        );

        let rewind = |extra: &str| {
            let body = format!(
                "repo={}&checkpoint_id={}&files=true&conversation=false{extra}",
                percent_encode_for_test(&repo_arg),
                manifest.checkpoint_id
            );
            send(format!(
                "POST /api/rewind HTTP/1.1\r\nHost: 127.0.0.1\r\ncontent-type: application/x-www-form-urlencoded\r\nx-damaian-api-token: {token}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                body.len()
            ))
        };

        // One file: the fourth restore operation, over the wire.
        let single = rewind("&path=app.js");
        assert!(
            single.contains("\"restoredFiles\":[\"app.js\"]"),
            "{single}"
        );
        assert_eq!(
            fs::read_to_string(repo.join("app.js")).unwrap(),
            "export const a = 1;\n"
        );
        assert_eq!(
            fs::read_to_string(repo.join("other.js")).unwrap(),
            "export const b = 2;\n"
        );

        let both = rewind("");
        assert!(both.contains("\"other.js\""), "{both}");
        assert!(both.contains("\"conversationRestored\":false"), "{both}");
        assert_eq!(
            fs::read_to_string(repo.join("other.js")).unwrap(),
            "export const b = 1;\n"
        );

        let _ = fs::remove_dir_all(&repo);
    }

    /// Serves the real UI with a known API token, seeded with a session whose
    /// turn can be rewound and with three tasks left mid-flight, so the
    /// checkpoint controls and both shapes of the crash recovery card can
    /// actually be looked at. The web UI takes its token from the Tauri
    /// bootstrap, so a browser cannot otherwise authenticate — and a full Tauri
    /// build is minutes.
    ///
    /// `#[ignore]`d: it binds a port and serves until it is stopped. Run it by
    /// hand and open the URL it prints:
    ///
    /// ```sh
    /// cargo test -p desktop-shell --lib -- --ignored --nocapture serves_the_ui
    /// ```
    ///
    /// Then, in the browser console (the token gates every `/api/` route):
    ///
    /// ```js
    /// apiToken = "damaian-ui-inspection-token";
    /// setRepository("<the repository path it printed>", false);
    /// ```
    #[test]
    #[ignore]
    fn serves_the_ui_for_manual_inspection() {
        let data_dir = isolated_data_dir().to_path_buf();
        let repo = std::env::temp_dir().join(format!(
            "damaian-ui-inspection-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&repo).unwrap();
        fs::write(repo.join("upload.rs"), "fn upload() {\n    todo!()\n}\n").unwrap();
        fs::write(repo.join("retry.rs"), "// no retry yet\n").unwrap();

        let repo_arg = repo.to_string_lossy().to_string();
        let engine = engine_for_repo(&repo_arg).expect("engine");
        let repository_id = engine
            .indexer
            .repository_id_for_path(&repo_arg)
            .expect("repository id");
        let session = engine
            .session_store
            .create_session(&repository_id, "Add retry to the upload client")
            .unwrap();
        let task = engine
            .session_store
            .create_task(
                &session.id,
                "add retry to the upload client",
                "mock",
                "mock",
            )
            .unwrap();
        let user_message = engine
            .session_store
            .append_message(
                &session.id,
                Some(&task.id),
                "user",
                "add retry to the upload client",
            )
            .unwrap();
        engine
            .session_store
            .append_message(
                &session.id,
                Some(&task.id),
                "assistant",
                "Added a retry helper and wired it into `upload`.",
            )
            .unwrap();
        let manifest = engine
            .checkpoint_store
            .create_checkpoint(
                &repo,
                workspace_engine::CheckpointRequest {
                    session_id: &session.id,
                    task_id: Some(&task.id),
                    user_message_id: Some(&user_message.id),
                    summary: "Before: add retry to the upload client",
                    conversation: workspace_engine::CheckpointConversation {
                        last_event_seq: 1,
                        task_status: "complete".to_string(),
                    },
                    pending_approvals: Vec::new(),
                    paths: vec![
                        workspace_engine::CheckpointPath {
                            path: "upload.rs".to_string(),
                            origin: workspace_engine::CheckpointOrigin::Patch,
                        },
                        workspace_engine::CheckpointPath {
                            path: "retry.rs".to_string(),
                            origin: workspace_engine::CheckpointOrigin::Command,
                        },
                    ],
                },
            )
            .unwrap();
        fs::write(
            repo.join("upload.rs"),
            "fn upload() {\n    with_retry(send)\n}\n",
        )
        .unwrap();
        fs::write(repo.join("retry.rs"), "pub fn with_retry() {}\n").unwrap();
        engine
            .checkpoint_store
            .seal_checkpoint(&repo, &manifest)
            .unwrap();

        // Both shapes of the crash recovery card
        // (`docs/specs/45_crash_recovery_prompt.md` §5.6) plus a reattached
        // approval, so the surface can be driven without staging a real crash.
        // The turn above stays non-terminal on purpose: its card is the one
        // with a checkpoint for `Inspect` to link to.
        let unknown = engine
            .session_store
            .create_task(&session.id, "run the database migration", "mock", "mock")
            .unwrap();
        let unknown = engine
            .session_store
            .update_task_status(&unknown, workspace_engine::TaskStatus::RunningTool, None)
            .unwrap();
        let _unknown_marker = engine
            .session_store
            .start_action(&unknown, "run_command", "psql -f migrate.sql", true)
            .unwrap();
        let interrupted = engine
            .session_store
            .create_task(&session.id, "explain the retry helper", "mock", "mock")
            .unwrap();
        // What spec 19's sweep records for a model call cut off by the crash,
        // so the card's spend line can be looked at (spec 45 §5.9).
        engine
            .session_store
            .record_task_usage_for_task_id(
                &session.id,
                &interrupted.id,
                "lost_marker_inspection",
                Some("marker_inspection"),
                workspace_engine::TokenUsage::estimated(4820, 0),
                None,
                Some("lost_to_crash"),
            )
            .unwrap();
        let interrupted = engine
            .session_store
            .update_task_status(
                &interrupted,
                workspace_engine::TaskStatus::PreparingContext,
                None,
            )
            .unwrap();
        let _interrupted_marker = engine
            .session_store
            .start_action(&interrupted, "read_file", "retry.rs", false)
            .unwrap();
        let awaiting = engine
            .session_store
            .create_task(&session.id, "list the crate", "mock", "mock")
            .unwrap();
        let proposal = engine
            .validation_orchestrator
            .propose_command(&repo, "ls -la", "Desktop command proposal")
            .unwrap();
        engine
            .session_store
            .await_approval(
                &awaiting,
                &workspace_engine::PendingApprovalRef {
                    kind: "command".to_string(),
                    proposal_id: proposal.id.clone(),
                },
            )
            .unwrap();

        // A plan review caught mid-crash: the plan in the log, its paused turn
        // on disk where the orchestrator parks one.
        let reviewing = engine
            .session_store
            .create_task(
                &session.id,
                "add retry to the upload client",
                "mock",
                "mock",
            )
            .unwrap();
        let mut plan = workspace_engine::TaskPlan::new(&reviewing.id, 1);
        for (id, title) in [
            ("step_1", "Read the upload client"),
            ("step_2", "Add a retry helper"),
            ("step_3", "Wire it into upload()"),
        ] {
            plan.steps.push(workspace_engine::PlanStep {
                id: id.to_string(),
                title: title.to_string(),
                detail: None,
                status: workspace_engine::StepStatus::Pending,
                depends_on: Vec::new(),
                started_at_ms: None,
                completed_at_ms: None,
                evidence: Vec::new(),
            });
        }
        engine.session_store.create_plan(&reviewing, &plan).unwrap();
        let pending_dir = data_dir.join("chat").join("pending");
        fs::create_dir_all(&pending_dir).unwrap();
        fs::write(
            pending_dir.join("planprop_inspection.json"),
            r#"{"proposal_id":"planprop_inspection","plan_review":{"deferred_action":"apply a patch to upload.rs"}}"#,
        )
        .unwrap();
        engine
            .session_store
            .await_approval(
                &reviewing,
                &workspace_engine::PendingApprovalRef {
                    kind: "plan".to_string(),
                    proposal_id: "planprop_inspection".to_string(),
                },
            )
            .unwrap();

        // A second session left in Ask mode, holding a turn the mode refused
        // (spec 20 §5.6), so the mode control and a refusal can be looked at.
        let (refused_session, _) = refused_turn_in_ask_mode(&repo);

        let (diagnosed_session, expected_card_header) =
            recorded_web_diagnostic_session(&engine, &repository_id, &data_dir);
        let legacy_session = legacy_web_diagnostic_session(&engine, &repository_id, &data_dir);
        let findings_session = findings_to_review_session(&engine, &repository_id, &repo);

        let options = ShellOptions {
            port: 4899,
            default_repo: Some(repo_arg.clone()),
            api_token: "damaian-ui-inspection-token".to_string(),
        };
        println!("UI inspection server: http://127.0.0.1:{}/", options.port);
        println!("  apiToken = \"{}\"", options.api_token);
        println!("  repository = {repo_arg}");
        println!("  session = {}", session.id);
        println!("  refused-in-Ask session = {refused_session}");
        println!("  recorded web diagnostic session = {diagnosed_session}");
        println!("    card header should read: {expected_card_header}");
        println!("  legacy web diagnostic session (regex thumbnails) = {legacy_session}");
        println!(
            "  findings session (the composer's repo bar should offer \"Fix 2 errors\") = {findings_session}"
        );
        println!("  data_dir = {}", data_dir.display());
        run_server(options).expect("serve the UI");
    }

    /// A browser diagnostic turn as spec 12 Task 3 records it: the tool
    /// message listing the materialised screenshot, plus the redacted record
    /// the card renders. Returns the session id and `render_for_model`'s header
    /// line, which the card must reproduce word for word.
    fn recorded_web_diagnostic_session(
        engine: &WorkspaceEngine,
        repository_id: &str,
        data_dir: &std::path::Path,
    ) -> (String, String) {
        let store = &engine.session_store;
        let session = store
            .create_session(repository_id, "Why does the snake game not start?")
            .unwrap();
        let task = store
            .create_task(
                &session.id,
                "why does the snake game not start?",
                "mock",
                "mock",
            )
            .unwrap();
        let relative = format!("web-diagnostics/{}/{}/run-1/page.png", session.id, task.id);
        write_inspection_png(&data_dir.join(&relative), [163, 55, 55]);

        let warnings: Vec<String> = (1..=6)
            .map(|n| format!(r#"{{"type": "warning", "text": "Deprecated API call {n}"}}"#))
            .collect();
        let companion = format!(
            r#"{{"url": "http://localhost:5001/", "final_url": "http://localhost:5001/play",
                "title": "Snake Game", "status": 200,
                "page_errors": ["ReferenceError: Cannot access 'game' before initialization"],
                "console": [
                  {{"type": "log", "text": "booting"}},
                  {{"type": "error", "text": "Failed to load resource: 404",
                    "location": {{"url": "http://localhost:5001/js/app.js", "lineNumber": 41, "columnNumber": 7}}}},
                  {}
                ],
                "failed_requests": [
                  {{"url": "http://localhost:5001/api/me", "method": "GET", "status": 404,
                    "status_text": "Not Found"}},
                  {{"url": "http://localhost:5001/ws", "method": "GET",
                    "failure": "net::ERR_CONNECTION_REFUSED"}}
                ],
                "results": [
                  {{"step": 0, "action": "goto", "success": true}},
                  {{"step": 1, "action": "click", "success": false,
                    "error": "locator('#start') is not visible"}}
                ],
                "dom_summary": {{"forms": 1, "buttons": ["Start", "Log in"],
                  "status_text": "Loading…", "visible_text_excerpt": "Snake\nScore: 0\nStart"}}}}"#,
            warnings.join(",\n")
        );
        let mut report = WebDiagnosticReport::from_text(companion, false);
        report.artifacts = vec![workspace_engine::WebDiagnosticArtifact {
            kind: "screenshot".to_string(),
            path: relative.clone(),
            mime_type: Some("image/png".to_string()),
            width: Some(160),
            height: Some(90),
        }];
        report.via = Some("MCP server `browser` tool `run_scenario`".to_string());
        let header = report
            .render_for_model()
            .and_then(|text| text.lines().next().map(str::to_string))
            .expect("a structured report renders");

        store
            .append_message(
                &session.id,
                Some(&task.id),
                "user",
                "why does the snake game not start?",
            )
            .unwrap();
        store
            .append_message(
                &session.id,
                Some(&task.id),
                "assistant",
                "Running a browser scenario against the game.",
            )
            .unwrap();
        store
            .append_message(
                &session.id,
                Some(&task.id),
                "tool",
                &format!("{header}\n\nArtifacts:\n- screenshot: {relative} (160x90)"),
            )
            .unwrap();
        store
            .append_web_diagnostic(
                &session.id,
                &WebDiagnosticRecord {
                    id: "webdiagrec_inspection".to_string(),
                    task_id: task.id.clone(),
                    tool: "run_web_scenario".to_string(),
                    url: "http://localhost:5001/".to_string(),
                    recorded_at_ms: 1_759_000_000_000,
                    report: report.redacted(&engine.scanner),
                },
            )
            .unwrap();
        store
            .append_message(
                &session.id,
                Some(&task.id),
                "assistant",
                "`game` is read before it is declared in `app.js`, so the start button never renders.",
            )
            .unwrap();
        // Finished, so the crash recovery sweep leaves the turn alone.
        store
            .update_task_status(&task, workspace_engine::TaskStatus::Complete, None)
            .unwrap();
        (session.id, header)
    }

    /// The findings panel's fixture (spec 22 Task 11): two open errors, one
    /// ranged and one not, an open warning, a stale lint finding and a
    /// dismissed test failure, across four sources. Returns the session id.
    fn findings_to_review_session(
        engine: &WorkspaceEngine,
        repository_id: &str,
        repo: &std::path::Path,
    ) -> String {
        let store = &engine.session_store;
        let session = store
            .create_session(repository_id, "Findings to review")
            .unwrap();
        let task = store
            .create_task(&session.id, "check the upload client", "mock", "mock")
            .unwrap();
        store
            .append_message(
                &session.id,
                Some(&task.id),
                "user",
                "check the upload client",
            )
            .unwrap();
        store
            .append_message(
                &session.id,
                Some(&task.id),
                "assistant",
                "`cargo check` and `npm test` both failed; the findings panel lists what they reported.",
            )
            .unwrap();

        let range = |path: &str, line: u32, column: u32| SourceRange {
            path: path.to_string(),
            start_line: line,
            start_column: Some(column),
            end_line: None,
            end_column: None,
        };
        let hashed = |path: &str| workspace_engine::hash::file_hash(repo.join(path)).unwrap();
        let record = |draft: FindingDraft, file_hash: Option<String>| {
            let mut finding = Finding::new(draft, &engine.scanner).with_task_id(&task.id);
            if let Some(file_hash) = file_hash {
                finding = finding.with_file_hash(file_hash);
            }
            store.record_finding(&session.id, &finding).unwrap();
            finding.id().to_string()
        };

        record(
            FindingDraft {
                source: FindingSource::Compiler,
                severity: Severity::Error,
                summary: "mismatched types".to_string(),
                details: Some(
                    "error[E0308]: mismatched types\n --> upload.rs:3:5\n  |\n3 |     with_retry(send)\n  |     ^^^^^^^^^^^^^^^^ expected `Result<(), Error>`, found `()`"
                        .to_string(),
                ),
                range: Some(range("upload.rs", 3, 5)),
                code: Some("E0308".to_string()),
            },
            Some(hashed("upload.rs")),
        );
        record(
            FindingDraft {
                source: FindingSource::Command,
                severity: Severity::Error,
                summary: "npm test: 1 failing".to_string(),
                details: None,
                range: None,
                code: None,
            },
            None,
        );
        record(
            FindingDraft {
                source: FindingSource::Compiler,
                severity: Severity::Warning,
                summary: "unused variable: `retries`".to_string(),
                details: None,
                range: Some(range("upload.rs", 7, 9)),
                code: Some("unused_variables".to_string()),
            },
            Some(hashed("upload.rs")),
        );
        // A hash no file has, so the finding derives stale on every read.
        record(
            FindingDraft {
                source: FindingSource::Lint,
                severity: Severity::Warning,
                summary: "unneeded `return` statement".to_string(),
                details: None,
                range: Some(range("retry.rs", 2, 5)),
                code: Some("clippy::needless_return".to_string()),
            },
            Some("sha256:0".to_string()),
        );
        let dismissed = record(
            FindingDraft {
                source: FindingSource::Test,
                severity: Severity::Error,
                summary: "tests::retries_three_times failed".to_string(),
                details: None,
                range: Some(range("retry.rs", 10, 1)),
                code: None,
            },
            Some(hashed("retry.rs")),
        );
        store
            .set_finding_status(
                &session.id,
                &dismissed,
                workspace_engine::finding::FindingStatus::Dismissed,
            )
            .unwrap();

        // Finished, so the crash recovery sweep leaves the turn alone.
        store
            .update_task_status(&task, workspace_engine::TaskStatus::Complete, None)
            .unwrap();
        session.id
    }

    /// A session from before spec 12 Task 3: no recorded diagnostic, only a
    /// tool message whose text names the screenshot, which the legacy regex
    /// thumbnails must keep showing.
    fn legacy_web_diagnostic_session(
        engine: &WorkspaceEngine,
        repository_id: &str,
        data_dir: &std::path::Path,
    ) -> String {
        let store = &engine.session_store;
        let session = store
            .create_session(
                repository_id,
                "Check the landing page (before recorded diagnostics)",
            )
            .unwrap();
        let task = store
            .create_task(&session.id, "check the landing page", "mock", "mock")
            .unwrap();
        let relative = format!("web-diagnostics/{}/{}/run-1/page.png", session.id, task.id);
        write_inspection_png(&data_dir.join(&relative), [161, 92, 24]);
        for (role, content) in [
            ("user", "check the landing page".to_string()),
            ("assistant", "Inspecting the page.".to_string()),
            (
                "tool",
                format!(
                    "Browser diagnostic result via MCP server `browser` tool `inspect_page`:\nNo problems found.\n\nArtifacts:\n- screenshot: {relative} (160x90)"
                ),
            ),
            ("assistant", "The landing page loads cleanly.".to_string()),
        ] {
            store
                .append_message(&session.id, Some(&task.id), role, &content)
                .unwrap();
        }
        store
            .update_task_status(&task, workspace_engine::TaskStatus::Complete, None)
            .unwrap();
        session.id
    }

    /// Writes a real 160×90 PNG, a stand-in page with an `accent`-coloured
    /// banner, so the thumbnails have something to show. Stored (uncompressed)
    /// deflate keeps it free of an image dependency.
    fn write_inspection_png(path: &std::path::Path, accent: [u8; 3]) {
        const WIDTH: u32 = 160;
        const HEIGHT: u32 = 90;
        let mut pixels = Vec::new();
        for y in 0..HEIGHT {
            pixels.push(0); // filter type: none
            for x in 0..WIDTH {
                let rgb = if y < 14 {
                    [23, 107, 93]
                } else if (26..40).contains(&y) && (12..148).contains(&x) {
                    accent
                } else {
                    [255, 255, 255]
                };
                pixels.extend_from_slice(&rgb);
            }
        }
        // zlib header, one stored block per 65535 bytes, then Adler-32.
        let mut zlib = vec![0x78, 0x01];
        let blocks: Vec<&[u8]> = pixels.chunks(65_535).collect();
        for (index, block) in blocks.iter().enumerate() {
            zlib.push(u8::from(index + 1 == blocks.len()));
            let len = block.len() as u16;
            zlib.extend_from_slice(&len.to_le_bytes());
            zlib.extend_from_slice(&(!len).to_le_bytes());
            zlib.extend_from_slice(block);
        }
        let (mut a, mut b) = (1u32, 0u32);
        for byte in &pixels {
            a = (a + u32::from(*byte)) % 65_521;
            b = (b + a) % 65_521;
        }
        zlib.extend_from_slice(&((b << 16) | a).to_be_bytes());

        let mut ihdr = Vec::new();
        ihdr.extend_from_slice(&WIDTH.to_be_bytes());
        ihdr.extend_from_slice(&HEIGHT.to_be_bytes());
        ihdr.extend_from_slice(&[8, 2, 0, 0, 0]); // 8-bit RGB
        let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
        for (kind, data) in [(&b"IHDR"[..], ihdr), (b"IDAT", zlib), (b"IEND", Vec::new())] {
            png.extend_from_slice(&(data.len() as u32).to_be_bytes());
            let start = png.len();
            png.extend_from_slice(kind);
            png.extend_from_slice(&data);
            let crc = png_crc(&png[start..]);
            png.extend_from_slice(&crc.to_be_bytes());
        }
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, png).unwrap();
    }

    fn png_crc(bytes: &[u8]) -> u32 {
        let mut crc = 0xffff_ffffu32;
        for byte in bytes {
            crc ^= u32::from(*byte);
            for _ in 0..8 {
                crc = if crc & 1 == 1 {
                    (crc >> 1) ^ 0xedb8_8320
                } else {
                    crc >> 1
                };
            }
        }
        !crc
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
        spawn_with_shell_stack("test-shell", move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { break };
                let _ = handle_connection(&mut stream, &options);
            }
        })
        .expect("spawn the test shell");

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

    /// Serves `handle_connection` on an ephemeral port and answers a failed
    /// route the way `run_server` does, so a test can see an error response
    /// rather than a closed socket. Returns the port and the API token.
    fn serve_for_test() -> (u16, String) {
        let options = ShellOptions::new(0, None);
        let token = options.api_token.clone();
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind test listener");
        let port = listener.local_addr().expect("local addr").port();
        spawn_with_shell_stack("test-shell", move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { break };
                if let Err(error) = handle_connection(&mut stream, &options) {
                    let _ = write_basic_response(
                        &mut stream,
                        500,
                        "application/json",
                        &json_error(&error),
                    );
                }
            }
        })
        .expect("spawn the test shell");
        (port, token)
    }

    fn send_for_test(port: u16, request: String) -> String {
        let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("connect to test server");
        stream.write_all(request.as_bytes()).expect("write request");
        let mut response = String::new();
        stream.read_to_string(&mut response).expect("read response");
        response
    }

    fn form_body_for_test(fields: &[(&str, &str)]) -> String {
        fields
            .iter()
            .map(|(key, value)| format!("{key}={}", percent_encode_for_test(value)))
            .collect::<Vec<_>>()
            .join("&")
    }

    fn response_json(response: &str) -> serde_json::Value {
        let body = response
            .split_once("\r\n\r\n")
            .map(|(_, body)| body)
            .unwrap_or(response);
        serde_json::from_str(body).unwrap_or_else(|error| panic!("{error}: {body}"))
    }

    fn post_fields_for_test(port: u16, token: &str, path: &str, fields: &[(&str, &str)]) -> String {
        let body = form_body_for_test(fields);
        send_for_test(
            port,
            format!(
                "POST {path} HTTP/1.1\r\nHost: 127.0.0.1\r\ncontent-type: application/x-www-form-urlencoded\r\nx-damaian-api-token: {token}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                body.len()
            ),
        )
    }

    /// A monorepo with two npm packages and a directory that only becomes a
    /// root when the user adds it (no manifest of its own).
    fn repository_map_fixture(name: &str) -> PathBuf {
        let repo = temp_path(name);
        for directory in ["packages/api/src", "packages/web/src", "tools/scripts"] {
            fs::create_dir_all(repo.join(directory)).unwrap();
        }
        let scripts = "{\"scripts\":{\"test\":\"node test.js\"}}";
        fs::write(repo.join("packages/api/package.json"), scripts).unwrap();
        fs::write(repo.join("packages/web/package.json"), scripts).unwrap();
        fs::write(repo.join("packages/api/src/index.ts"), "export {};\n").unwrap();
        fs::write(repo.join("packages/web/src/index.ts"), "export {};\n").unwrap();
        fs::write(repo.join("tools/scripts/run.ts"), "export {};\n").unwrap();
        repo
    }

    /// A Rust declaration at the repository root and one under an npm root, so
    /// the symbol endpoints have both a provenance and a `rootPath` to report.
    fn symbol_fixture(name: &str) -> PathBuf {
        isolated_data_dir();
        let repo = repository_map_fixture(name);
        fs::create_dir_all(repo.join("src")).unwrap();
        fs::write(repo.join("src/upload.rs"), "pub fn upload_retry() {}\n").unwrap();
        fs::write(
            repo.join("packages/api/src/retry.ts"),
            "export function uploadRetry() {}\n",
        )
        .unwrap();
        repo
    }

    fn find_root<'a>(payload: &'a serde_json::Value, path: &str) -> Option<&'a serde_json::Value> {
        payload["map"]["roots"]
            .as_array()?
            .iter()
            .find(|root| root["path"] == path)
    }

    #[test]
    fn repository_map_endpoint_serves_the_map_and_its_load_shape() {
        let repo = repository_map_fixture("repomap-shape");
        let (port, token) = serve_for_test();
        let url = format!(
            "/api/repository-map?repo={}",
            percent_encode_for_test(&repo.to_string_lossy())
        );
        let response = get_for_test(port, &token, &url);
        assert!(response.starts_with("HTTP/1.1 200"), "{response}");

        let payload = json_of(&response);
        assert_eq!(payload["load"]["outcome"], "built", "{payload}");
        assert_eq!(payload["map"]["schemaVersion"], 1, "{payload}");

        let api = find_root(&payload, "packages/api")
            .unwrap_or_else(|| panic!("packages/api root missing: {payload}"));
        assert_eq!(api["detectedBy"]["kind"], "manifest");
        assert_eq!(api["detectedBy"]["path"], "packages/api/package.json");
        let commands = api["commands"].as_array().expect("commands array");
        assert!(
            commands
                .iter()
                .any(|command| command["command"] == "npm test"),
            "{api}"
        );
        assert!(
            commands
                .iter()
                .all(|command| command["workingDirectory"] == "packages/api"),
            "{api}"
        );
    }

    #[test]
    fn repository_roots_endpoint_writes_and_re_reads_an_added_root() {
        let repo = repository_map_fixture("repomap-add");
        let repo_str = repo.to_string_lossy().to_string();
        let (port, token) = serve_for_test();

        let response = post_fields_for_test(
            port,
            &token,
            "/api/repository-roots",
            &[
                ("repo", &repo_str),
                ("action", "add"),
                ("path", "tools/scripts"),
            ],
        );
        assert!(response.starts_with("HTTP/1.1 200"), "{response}");
        let payload = json_of(&response);
        assert_eq!(
            payload["configPath"],
            repo.join(".damaian/config.conf").to_string_lossy().as_ref(),
            "{payload}"
        );

        let written = fs::read_to_string(repo.join(".damaian/config.conf")).unwrap();
        assert!(
            written.contains("project_roots_added=tools/scripts"),
            "{written}"
        );

        let added = find_root(&payload, "tools/scripts")
            .unwrap_or_else(|| panic!("added root missing: {payload}"));
        assert_eq!(added["detectedBy"]["kind"], "userOverride");
        assert_eq!(added["userOverride"], "added");

        let url = format!(
            "/api/repository-map?repo={}",
            percent_encode_for_test(&repo_str)
        );
        let reread = json_of(&get_for_test(port, &token, &url));
        assert!(
            find_root(&reread, "tools/scripts").is_some_and(|root| root["userOverride"] == "added"),
            "{reread}"
        );
    }

    #[test]
    fn repository_roots_endpoint_reports_an_invalid_path_as_an_invalid_override() {
        let repo = repository_map_fixture("repomap-invalid");
        let repo_str = repo.to_string_lossy().to_string();
        let (port, token) = serve_for_test();

        let response = post_fields_for_test(
            port,
            &token,
            "/api/repository-roots",
            &[
                ("repo", &repo_str),
                ("action", "add"),
                ("path", "../outside"),
            ],
        );
        assert!(response.starts_with("HTTP/1.1 200"), "{response}");
        let payload = json_of(&response);

        let excluded = payload["map"]["excluded"]
            .as_array()
            .expect("excluded array");
        assert!(
            excluded
                .iter()
                .any(|entry| entry["path"] == "../outside" && entry["reason"] == "invalidOverride"),
            "{payload}"
        );
        assert!(
            find_root(&payload, "../outside").is_none(),
            "an invalid override must not become a root: {payload}"
        );
    }

    #[test]
    fn repository_roots_endpoint_refuses_a_foreign_repository() {
        let (port, token) = serve_for_test();
        let not_a_repository = temp_path("repomap-foreign");
        fs::write(&not_a_repository, "not a directory").unwrap();
        let path = not_a_repository.to_string_lossy().to_string();

        let response = post_fields_for_test(
            port,
            &token,
            "/api/repository-roots",
            &[
                ("repo", &path),
                ("action", "add"),
                ("path", "tools/scripts"),
            ],
        );
        assert!(response.starts_with("HTTP/1.1 500"), "{response}");
    }

    #[test]
    fn a_root_override_is_never_written_outside_the_allowed_roots() {
        let allowed = temp_path("repomap-allowed");
        let inside = allowed.join("repo");
        let outside = temp_path("repomap-outside");
        fs::create_dir_all(&inside).unwrap();
        fs::create_dir_all(&outside).unwrap();
        let engine = WorkspaceEngine::new(Config {
            data_dir: allowed.join(".damaian-data"),
            allowed_roots: vec![allowed.clone()],
            enable_index_watcher: false,
            ..Config::default()
        });

        let inside_str = inside.to_string_lossy().to_string();
        assert_eq!(
            repository_config_write_target(&engine, &inside_str).unwrap(),
            Config::repository_config_path(&inside_str)
        );
        let refused = repository_config_write_target(&engine, &outside.to_string_lossy());
        assert!(
            refused
                .as_ref()
                .is_err_and(|error| error.contains("not allowed")),
            "{refused:?}"
        );
        assert!(!outside.join(".damaian").exists());
    }

    #[test]
    fn symbols_endpoint_serves_matches_with_provenance_and_their_root() {
        let repo = symbol_fixture("symbols-shape");
        let (port, token) = serve_for_test();
        let url = format!(
            "/api/symbols?repo={}&q=upload_retry",
            percent_encode_for_test(&repo.to_string_lossy())
        );
        let response = get_for_test(port, &token, &url);
        assert!(response.starts_with("HTTP/1.1 200"), "{response}");

        let payload = json_of(&response);
        let matches = payload["matches"].as_array().expect("matches");
        assert_eq!(matches.len(), 1, "{payload}");
        assert_eq!(matches[0]["quality"], "exact");
        let symbol = &matches[0]["symbol"];
        assert_eq!(symbol["name"], "upload_retry");
        assert_eq!(symbol["kind"], "function");
        assert_eq!(symbol["path"], "src/upload.rs");
        assert_eq!(symbol["line"], 1);
        assert_eq!(symbol["rootPath"], "");
        assert_eq!(symbol["provenance"], "heuristic");

        // The root is the payload's, not something the UI infers.
        let rooted = format!(
            "/api/symbols?repo={}&q=uploadRetry",
            percent_encode_for_test(&repo.to_string_lossy())
        );
        let response = get_for_test(port, &token, &rooted);
        let payload = json_of(&response);
        assert_eq!(
            payload["matches"][0]["symbol"]["rootPath"], "packages/api",
            "{payload}"
        );
    }

    #[test]
    fn symbol_references_endpoint_answers_without_a_language_server() {
        let repo = symbol_fixture("symbols-references");
        let (port, token) = serve_for_test();
        let url = format!(
            "/api/symbol-references?repo={}&path=src%2Fupload.rs&line=1&name=upload_retry",
            percent_encode_for_test(&repo.to_string_lossy())
        );
        let response = get_for_test(port, &token, &url);
        assert!(response.starts_with("HTTP/1.1 200"), "{response}");

        let payload = json_of(&response);
        assert_eq!(payload["kind"], "references");
        assert_eq!(payload["symbol"]["path"], "src/upload.rs");
        assert_eq!(payload["symbol"]["line"], 1);
        assert_eq!(payload["symbol"]["name"], "upload_retry");
        assert_eq!(payload["language"], "rust");
        assert_eq!(payload["available"], false);
        assert!(
            payload["reason"]
                .as_str()
                .is_some_and(|reason| reason.contains("no language server for rust")),
            "{payload}"
        );
        assert_eq!(payload["relations"], serde_json::json!([]));
    }

    #[test]
    fn symbol_endpoints_refuse_a_repository_that_is_not_a_directory() {
        let (port, token) = serve_for_test();
        let not_a_repository = temp_path("symbols-foreign");
        fs::write(&not_a_repository, "not a directory").unwrap();
        let repo = percent_encode_for_test(&not_a_repository.to_string_lossy());

        for path in [
            format!("/api/symbols?repo={repo}&q=upload_retry"),
            format!("/api/symbol-references?repo={repo}&path=x.rs&line=1&name=x"),
            format!("/api/language-servers?repo={repo}"),
        ] {
            let response = get_for_test(port, &token, &path);
            assert!(response.starts_with("HTTP/1.1 500"), "{path}: {response}");
        }
        fs::remove_file(not_a_repository).unwrap();
    }

    #[test]
    fn symbol_repository_stays_inside_the_allowed_roots() {
        let allowed = temp_path("symbols-allowed");
        let inside = allowed.join("repo");
        let outside = temp_path("symbols-outside");
        fs::create_dir_all(&inside).unwrap();
        fs::create_dir_all(&outside).unwrap();
        let engine = WorkspaceEngine::new(Config {
            data_dir: allowed.join(".damaian-data"),
            allowed_roots: vec![allowed.clone()],
            enable_index_watcher: false,
            ..Config::default()
        });

        assert_eq!(
            symbol_repository(&engine, &inside.to_string_lossy()).unwrap(),
            fs::canonicalize(&inside).unwrap()
        );
        let refused = symbol_repository(&engine, &outside.to_string_lossy());
        assert!(
            refused
                .as_ref()
                .is_err_and(|error| error.contains("not allowed")),
            "{refused:?}"
        );
        fs::remove_dir_all(allowed).unwrap();
        fs::remove_dir_all(outside).unwrap();
    }

    #[test]
    fn language_servers_endpoint_serves_the_hub_shape() {
        let repo = symbol_fixture("symbols-servers");
        let (port, token) = serve_for_test();
        let url = format!(
            "/api/language-servers?repo={}",
            percent_encode_for_test(&repo.to_string_lossy())
        );
        let response = get_for_test(port, &token, &url);
        assert!(response.starts_with("HTTP/1.1 200"), "{response}");
        // Nothing is configured in this test's user config, so the list is
        // empty rather than absent.
        assert_eq!(json_of(&response)["servers"], serde_json::json!([]));
    }

    #[test]
    fn language_servers_json_lists_configured_servers_before_they_start() {
        let mut configured = BTreeMap::new();
        configured.insert("rust".to_string(), "rust-analyzer".to_string());
        configured.insert(
            "python".to_string(),
            "pyright-langserver --stdio".to_string(),
        );
        let statuses = vec![
            LanguageServerStatus {
                language: "rust".to_string(),
                command: "rust-analyzer".to_string(),
                state: LanguageServerState::Crashed { restarts: 1 },
                pid: None,
            },
            LanguageServerStatus {
                language: "python".to_string(),
                command: "pyright-langserver --stdio".to_string(),
                state: LanguageServerState::Unavailable {
                    reason: "spawn failed: No such file or directory".to_string(),
                },
                pid: None,
            },
        ];

        let payload: serde_json::Value =
            serde_json::from_str(&language_servers_json(&configured, statuses)).unwrap();
        let servers = payload["servers"].as_array().expect("servers");
        assert_eq!(servers.len(), 2, "{payload}");
        assert_eq!(servers[0]["language"], "python");
        assert_eq!(servers[0]["state"], "unavailable");
        assert_eq!(
            servers[0]["reason"],
            "spawn failed: No such file or directory"
        );
        assert_eq!(servers[1]["language"], "rust");
        assert_eq!(servers[1]["state"], "crashed");
        assert_eq!(servers[1]["restarts"], 1);
        assert!(servers[1]["reason"].is_null(), "{payload}");
    }

    #[test]
    fn a_configured_language_server_is_idle_until_a_lookup_starts_it() {
        let mut configured = BTreeMap::new();
        configured.insert("go".to_string(), "gopls".to_string());

        let payload: serde_json::Value =
            serde_json::from_str(&language_servers_json(&configured, Vec::new())).unwrap();
        let server = &payload["servers"][0];
        assert_eq!(server["language"], "go");
        assert_eq!(server["command"], "gopls");
        assert_eq!(server["state"], "idle");
        assert!(server["pid"].is_null());
    }

    #[test]
    fn command_proposal_endpoint_serves_a_stored_proposals_directories() {
        let repo = repository_map_fixture("repomap-proposal");
        let repo_str = repo.to_string_lossy().to_string();
        let engine = engine_for_repo(&repo_str).unwrap();
        let proposal = engine
            .validation_orchestrator
            .propose_command_at(
                &repo,
                repo.join("packages/api"),
                "npm test",
                "test proposal",
            )
            .unwrap();

        let (port, token) = serve_for_test();
        let url = format!(
            "/api/command-proposal?repo={}&proposal_id={}",
            percent_encode_for_test(&repo_str),
            proposal.id
        );
        let response = get_for_test(port, &token, &url);
        assert!(response.starts_with("HTTP/1.1 200"), "{response}");
        let payload = json_of(&response);
        assert_eq!(
            payload["workingDirectory"],
            repo.join("packages/api").to_string_lossy().as_ref(),
            "{payload}"
        );
        assert_eq!(payload["repositoryRoot"], repo_str, "{payload}");
    }

    fn get_session_for_test(port: u16, token: &str, session_id: &str) -> String {
        send_for_test(
            port,
            format!(
                "GET /api/session?session_id={session_id} HTTP/1.1\r\nHost: 127.0.0.1\r\nx-damaian-api-token: {token}\r\nconnection: close\r\n\r\n"
            ),
        )
    }

    fn post_session_mode_for_test(port: u16, token: &str, session_id: &str, mode: &str) -> String {
        let body = format!("session_id={session_id}&mode={mode}");
        send_for_test(
            port,
            format!(
                "POST /api/session-mode HTTP/1.1\r\nHost: 127.0.0.1\r\ncontent-type: application/x-www-form-urlencoded\r\nx-damaian-api-token: {token}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                body.len()
            ),
        )
    }

    fn new_session_for_test(title: &str) -> String {
        isolated_data_dir();
        default_engine()
            .expect("default engine")
            .session_store
            .create_session("repo_session_mode_test", title)
            .expect("create session")
            .id
    }

    /// Requirement 7's default, made visible over the wire: a session nobody
    /// has set a mode on reads as Code.
    #[test]
    fn session_json_includes_the_mode() {
        let session_id = new_session_for_test("mode default");
        let (port, token) = serve_for_test();

        let response = get_session_for_test(port, &token, &session_id);

        assert!(response.starts_with("HTTP/1.1 200"), "{response}");
        assert!(response.contains("\"mode\":\"code\""), "{response}");
    }

    /// The follow-up GET is what proves the write persisted: an endpoint that
    /// only echoed the requested mode back would pass the first assertion.
    #[test]
    fn post_session_mode_changes_it_and_returns_the_updated_session() {
        let session_id = new_session_for_test("mode switch");
        let (port, token) = serve_for_test();

        let changed = post_session_mode_for_test(port, &token, &session_id, "ask");
        assert!(changed.starts_with("HTTP/1.1 200"), "{changed}");
        assert!(
            changed.contains(&format!("\"session\":{{\"id\":\"{session_id}\"")),
            "{changed}"
        );
        assert!(changed.contains("\"mode\":\"ask\""), "{changed}");

        let reread = get_session_for_test(port, &token, &session_id);
        assert!(reread.contains("\"mode\":\"ask\""), "{reread}");
    }

    /// An unknown mode must not fall back to Code: silently landing in the
    /// most permissive mode is exactly the widening requirement 4 forbids.
    #[test]
    fn post_session_mode_rejects_an_unknown_mode_string() {
        let session_id = new_session_for_test("mode reject");
        let (port, token) = serve_for_test();
        post_session_mode_for_test(port, &token, &session_id, "review");

        let rejected = post_session_mode_for_test(port, &token, &session_id, "sideways");

        assert!(!rejected.starts_with("HTTP/1.1 200"), "{rejected}");
        assert!(rejected.contains("sideways"), "{rejected}");
        let reread = get_session_for_test(port, &token, &session_id);
        assert!(reread.contains("\"mode\":\"review\""), "{reread}");
    }

    /// A repository with one source file, an engine for it, and a session
    /// that belongs to it.
    fn findings_fixture(name: &str) -> (PathBuf, WorkspaceEngine, String) {
        isolated_data_dir();
        let repo = std::env::temp_dir().join(format!(
            "damaian-shell-findings-{name}-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(repo.join("src")).expect("repository");
        fs::write(repo.join("src/lib.rs"), "fn a() {}\n").expect("source file");
        let engine = engine_for_repo(repo.to_str().unwrap()).expect("engine");
        let repository_id = engine
            .indexer
            .repository_id_for_path(&repo)
            .expect("repository id");
        let session_id = engine
            .session_store
            .create_session(&repository_id, "Findings")
            .expect("session")
            .id;
        (repo, engine, session_id)
    }

    /// Records a finding. With a path, it is ranged and hashed the way
    /// recording does it (spec 22 Task 8), so editing the file makes it stale.
    fn record_finding_for_test(
        engine: &WorkspaceEngine,
        repo: &std::path::Path,
        session_id: &str,
        summary: &str,
        path: Option<&str>,
    ) -> String {
        let mut finding = Finding::new(
            FindingDraft {
                source: FindingSource::Compiler,
                severity: Severity::Error,
                summary: summary.to_string(),
                details: None,
                range: path.map(|path| SourceRange {
                    path: path.to_string(),
                    start_line: 1,
                    start_column: None,
                    end_line: None,
                    end_column: None,
                }),
                code: None,
            },
            &SecretScanner::default(),
        );
        if let Some(path) = path {
            finding =
                finding.with_file_hash(workspace_engine::hash::file_hash(repo.join(path)).unwrap());
        }
        engine
            .session_store
            .record_finding(session_id, &finding)
            .expect("record finding");
        finding.id().to_string()
    }

    fn get_findings_for_test(
        port: u16,
        token: &str,
        repo: &std::path::Path,
        session_id: &str,
    ) -> String {
        send_for_test(
            port,
            format!(
                "GET /api/findings?repo={}&session_id={session_id} HTTP/1.1\r\nHost: 127.0.0.1\r\nx-damaian-api-token: {token}\r\nconnection: close\r\n\r\n",
                repo.display()
            ),
        )
    }

    fn post_form_for_test(port: u16, token: &str, path: &str, body: &str) -> String {
        send_for_test(
            port,
            format!(
                "POST {path} HTTP/1.1\r\nHost: 127.0.0.1\r\ncontent-type: application/x-www-form-urlencoded\r\nx-damaian-api-token: {token}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                body.len()
            ),
        )
    }

    fn json_of(response: &str) -> serde_json::Value {
        let body = response.split("\r\n\r\n").nth(1).expect("a body");
        serde_json::from_str(body).expect("JSON body")
    }

    fn status_of(json: &serde_json::Value, id: &str) -> String {
        json["findings"]
            .as_array()
            .expect("findings")
            .iter()
            .find(|finding| finding["id"] == id)
            .unwrap_or_else(|| panic!("{id} missing from {json}"))["status"]
            .as_str()
            .unwrap()
            .to_string()
    }

    /// Staleness is derived against `repo`'s files (spec 22 `context.md`
    /// §16), so an edit shows up in the next GET. The untouched ranged finding
    /// is what pins the root: read against any other directory its file is
    /// missing or different, and it would be stale too.
    #[test]
    fn get_findings_returns_them_with_staleness_derived_against_the_repository() {
        let (repo, engine, session_id) = findings_fixture("get");
        fs::write(repo.join("src/kept.rs"), "fn kept() {}\n").expect("second file");
        let unranged = record_finding_for_test(&engine, &repo, &session_id, "no file", None);
        let ranged =
            record_finding_for_test(&engine, &repo, &session_id, "on a file", Some("src/lib.rs"));
        let untouched =
            record_finding_for_test(&engine, &repo, &session_id, "kept", Some("src/kept.rs"));
        fs::write(repo.join("src/lib.rs"), "fn a() { edited() }\n").unwrap();
        let (port, token) = serve_for_test();

        let response = get_findings_for_test(port, &token, &repo, &session_id);

        assert!(response.starts_with("HTTP/1.1 200"), "{response}");
        let json = json_of(&response);
        assert_eq!(json["findings"][0]["id"], unranged.as_str(), "record order");
        assert_eq!(status_of(&json, &unranged), "open");
        assert_eq!(status_of(&json, &ranged), "stale");
        assert_eq!(status_of(&json, &untouched), "open");
    }

    /// §16: reading a session against another checkout would judge
    /// staleness on the wrong files.
    #[test]
    fn findings_endpoints_refuse_a_session_from_another_repository() {
        let (repo, engine, _) = findings_fixture("other-repo");
        let foreign = engine
            .session_store
            .create_session("repo_somewhere_else", "Foreign")
            .unwrap()
            .id;
        let (port, token) = serve_for_test();

        let response = get_findings_for_test(port, &token, &repo, &foreign);
        assert!(!response.starts_with("HTTP/1.1 200"), "{response}");
        assert!(response.contains("another repository"), "{response}");

        let body = format!("repo={}&session_id={foreign}&finding_ids=x", repo.display());
        let repair = post_form_for_test(port, &token, "/api/findings-repair", &body);
        assert!(!repair.starts_with("HTTP/1.1 200"), "{repair}");
    }

    /// The `language_server.rust` command line for this test binary's fake
    /// server, `fake_language_server_for_findings_helper`. libtest prints on
    /// stdout before the helper runs, so the helper speaks LSP on fd 3.
    fn fake_language_server_command() -> String {
        format!(
            "/bin/sh -c \"export DAMAIAN_SHELL_FAKE_LSP=1; exec \\\"\\$0\\\" --ignored --exact \
             tests::fake_language_server_for_findings_helper 3>&1 1>/dev/null\" '{}'",
            std::env::current_exe().unwrap().display()
        )
    }

    /// Held by each test that uses the global hub, because each ends with
    /// `shutdown_global`, which would stop the other's server under a
    /// threaded `cargo test`.
    static GLOBAL_HUB_TESTS: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// Opens `src/lib.rs` on the global hub, the one `GET /api/findings`
    /// reads, and waits for a `documentSymbol` answer. The fake server
    /// publishes on `didOpen`, before it reads the request, so once the
    /// answer is in the hub holds the diagnostic.
    fn publish_a_language_server_error(engine: &WorkspaceEngine, repo: &std::path::Path) {
        let mut config = engine.config.clone();
        config
            .language_servers
            .insert("rust".to_string(), fake_language_server_command());
        let launch = LanguageServerLaunch::from_config(&config, repo, "rust", "ses_shell")
            .expect("rust is configured");
        let hub = LanguageServerHub::global();
        hub.notify(&launch, |client, uris| {
            client.did_open(&uris.uri_for("src/lib.rs"), "rust", 1, "fn a() {}\n")
        })
        .expect("didOpen");
        hub.request(&launch, |client, uris| {
            client.document_symbol(&uris.uri_for("src/lib.rs"))
        })
        .expect("documentSymbol");
    }

    /// Spec 25 Task 7: the panel's GET records what the hub holds first.
    #[test]
    fn get_findings_returns_a_language_server_diagnostic() {
        let _hub = GLOBAL_HUB_TESTS
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let (repo, engine, session_id) = findings_fixture("language-server");
        publish_a_language_server_error(&engine, &repo);
        let (port, token) = serve_for_test();

        let response = get_findings_for_test(port, &token, &repo, &session_id);
        let again = get_findings_for_test(port, &token, &repo, &session_id);
        LanguageServerHub::shutdown_global();

        assert!(response.starts_with("HTTP/1.1 200"), "{response}");
        let json = json_of(&response);
        let findings = json["findings"].as_array().expect("findings");
        assert_eq!(findings.len(), 1, "{json}");
        let finding = &findings[0];
        assert_eq!(finding["source"], "language_server");
        assert_eq!(finding["severity"], "error");
        assert_eq!(finding["summary"], "the fake server says no");
        assert_eq!(finding["code"], "E1");
        assert_eq!(finding["taskId"], serde_json::Value::Null);
        assert_eq!(finding["range"]["path"], "src/lib.rs");
        assert_eq!(finding["range"]["startLine"], 1);
        assert_eq!(finding["range"]["startColumn"], 4);
        assert_eq!(finding["status"], "open");
        assert_eq!(
            json_of(&again)["findings"].as_array().unwrap().len(),
            1,
            "a second GET records nothing new"
        );
    }

    /// A recording failure is logged, and the panel still gets what the
    /// session holds. Here the session log is read-only, so the append fails.
    #[test]
    fn get_findings_lists_what_it_has_when_recording_fails() {
        use std::os::unix::fs::PermissionsExt;

        let _hub = GLOBAL_HUB_TESTS
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let (repo, engine, session_id) = findings_fixture("language-server-failing");
        let kept = record_finding_for_test(&engine, &repo, &session_id, "kept", None);
        publish_a_language_server_error(&engine, &repo);
        let log = engine
            .config
            .data_dir
            .join("sessions")
            .join(format!("{session_id}.jsonl"));
        fs::set_permissions(&log, fs::Permissions::from_mode(0o444)).unwrap();
        let (port, token) = serve_for_test();

        let response = get_findings_for_test(port, &token, &repo, &session_id);
        fs::set_permissions(&log, fs::Permissions::from_mode(0o644)).unwrap();
        LanguageServerHub::shutdown_global();

        assert!(response.starts_with("HTTP/1.1 200"), "{response}");
        let json = json_of(&response);
        let ids: Vec<&str> = json["findings"]
            .as_array()
            .expect("findings")
            .iter()
            .map(|finding| finding["id"].as_str().unwrap())
            .collect();
        assert_eq!(ids, vec![kept.as_str()], "{json}");
    }

    /// The fake language server for the two tests above. Never run on its
    /// own: they re-execute this test binary into it, and run directly it
    /// returns at once. It answers `initialize`, `documentSymbol` (`[]`) and
    /// `shutdown`, and on each `didOpen` publishes one error on the opened
    /// file: line 0, characters 3–4, code `E1`.
    #[test]
    #[ignore = "the fake language server; re-executed by the findings tests"]
    fn fake_language_server_for_findings_helper() {
        use std::os::fd::FromRawFd;
        use workspace_engine::{read_message, write_message};

        if std::env::var("DAMAIAN_SHELL_FAKE_LSP").is_err() {
            return;
        }
        // SAFETY: fd 3 is the pipe the command line moved there, and nothing
        // else in this process owns it.
        let mut output = unsafe { fs::File::from_raw_fd(3) };
        let stdin = std::io::stdin();
        let mut input = stdin.lock();
        loop {
            let Ok(Some(message)) = read_message(&mut input) else {
                return;
            };
            let id = message.get("id").cloned();
            let reply = match (message["method"].as_str().unwrap_or_default(), id) {
                ("initialize", Some(id)) => {
                    serde_json::json!({ "jsonrpc": "2.0", "id": id, "result": { "capabilities": {} } })
                }
                ("shutdown", Some(id)) => {
                    serde_json::json!({ "jsonrpc": "2.0", "id": id, "result": null })
                }
                (_, Some(id)) => serde_json::json!({ "jsonrpc": "2.0", "id": id, "result": [] }),
                ("exit", None) => return,
                ("textDocument/didOpen", None) => serde_json::json!({
                    "jsonrpc": "2.0",
                    "method": "textDocument/publishDiagnostics",
                    "params": {
                        "uri": message["params"]["textDocument"]["uri"],
                        "diagnostics": [{
                            "range": {
                                "start": { "line": 0, "character": 3 },
                                "end": { "line": 0, "character": 4 },
                            },
                            "severity": 1,
                            "code": "E1",
                            "message": "the fake server says no",
                        }],
                    },
                }),
                _ => continue,
            };
            write_message(&mut output, &reply).unwrap();
        }
    }

    /// The follow-up GET proves the write persisted. An endpoint that only
    /// echoed the request would pass the first assertion.
    #[test]
    fn post_finding_status_dismisses_and_a_later_get_shows_it() {
        let (repo, engine, session_id) = findings_fixture("dismiss");
        let id = record_finding_for_test(&engine, &repo, &session_id, "waved away", None);
        let (port, token) = serve_for_test();

        let body = format!(
            "repo={}&session_id={session_id}&finding_id={id}&status=dismissed",
            repo.display()
        );
        let changed = post_form_for_test(port, &token, "/api/finding-status", &body);
        assert!(changed.starts_with("HTTP/1.1 200"), "{changed}");
        assert_eq!(status_of(&json_of(&changed), &id), "dismissed");

        let reread = get_findings_for_test(port, &token, &repo, &session_id);
        assert_eq!(status_of(&json_of(&reread), &id), "dismissed");
    }

    /// Rejected, never defaulted (`/api/session-mode`'s rule), and `stale`
    /// is refused by the store (spec 22 `context.md` §13.2).
    #[test]
    fn post_finding_status_rejects_an_unknown_status_and_refuses_stale() {
        let (repo, engine, session_id) = findings_fixture("reject");
        let id = record_finding_for_test(&engine, &repo, &session_id, "x", None);
        let (port, token) = serve_for_test();

        for status in ["sideways", "stale"] {
            let body = format!(
                "repo={}&session_id={session_id}&finding_id={id}&status={status}",
                repo.display()
            );
            let rejected = post_form_for_test(port, &token, "/api/finding-status", &body);
            assert!(
                !rejected.starts_with("HTTP/1.1 200"),
                "{status}: {rejected}"
            );
        }
        let reread = get_findings_for_test(port, &token, &repo, &session_id);
        assert_eq!(status_of(&json_of(&reread), &id), "open");
    }

    #[test]
    fn post_findings_repair_returns_the_request_and_its_prompt() {
        let (repo, engine, session_id) = findings_fixture("repair");
        let open = record_finding_for_test(&engine, &repo, &session_id, "fix me", None);
        let stale =
            record_finding_for_test(&engine, &repo, &session_id, "moved", Some("src/lib.rs"));
        fs::write(repo.join("src/lib.rs"), "fn a() { edited() }\n").unwrap();
        let (port, token) = serve_for_test();

        let body = format!(
            "repo={}&session_id={session_id}&finding_ids={open},{stale}",
            repo.display()
        );
        let response = post_form_for_test(port, &token, "/api/findings-repair", &body);

        assert!(response.starts_with("HTTP/1.1 200"), "{response}");
        let json = json_of(&response);
        assert_eq!(json["request"]["findings"][0]["id"], open.as_str());
        assert_eq!(json["request"]["findings"].as_array().unwrap().len(), 1);
        assert_eq!(json["request"]["excluded"][0]["findingId"], stale.as_str());
        assert_eq!(json["request"]["excluded"][0]["reason"], "stale");
        let prompt = json["prompt"].as_str().expect("a prompt");
        assert!(prompt.contains(&format!("(finding {open})")), "{prompt}");
        assert!(prompt.contains(&format!("- {stale}: stale")), "{prompt}");
    }

    /// spec 22 `context.md` §15: a prompt that fixes nothing is not sent.
    #[test]
    fn post_findings_repair_with_nothing_open_returns_a_null_prompt() {
        let (repo, engine, session_id) = findings_fixture("repair-empty");
        let id = record_finding_for_test(&engine, &repo, &session_id, "done", None);
        engine
            .session_store
            .set_finding_status(
                &session_id,
                &id,
                workspace_engine::finding::FindingStatus::Fixed,
            )
            .unwrap();
        let (port, token) = serve_for_test();

        let body = format!(
            "repo={}&session_id={session_id}&finding_ids={id}",
            repo.display()
        );
        let response = post_form_for_test(port, &token, "/api/findings-repair", &body);

        assert!(response.starts_with("HTTP/1.1 200"), "{response}");
        let json = json_of(&response);
        assert!(json["prompt"].is_null(), "{json}");
        assert_eq!(json["request"]["excluded"][0]["reason"], "fixed");
    }

    const REFUSED_TURN_ANSWER: &str =
        "Ask mode does not let me change files; switch to Code and I will.";

    /// Runs a real turn through the orchestrator in which an Ask-mode session's
    /// model calls `propose_patch`, and returns the session id and the adapter
    /// (whose `requests` show what was replayed to the model). Shared by the
    /// end-to-end refusal test and the UI inspection server.
    fn refused_turn_in_ask_mode(repo: &std::path::Path) -> (String, MockModelAdapter) {
        let mut config = Config {
            data_dir: isolated_data_dir().to_path_buf(),
            enable_index_watcher: false,
            ..Config::default()
        };
        config
            .model_providers
            .push(workspace_engine::ModelProviderConfig {
                id: "openai".to_string(),
                label: "OpenAI".to_string(),
                base_url: String::new(),
                api_key_env: String::new(),
                models: Vec::new(),
                supports_native_tools: true,
                max_output_tokens: None,
                context_token_budget: None,
                provider_reports_usage: true,
                price_per_million_input_tokens: None,
                price_per_million_output_tokens: None,
                price_per_million_cached_input_tokens: None,
                supports_explicit_cache_breakpoints: false,
            });
        let engine = WorkspaceEngine::new(config);
        let mut on_token = |_token: &str| {};
        let session_id = engine
            .chat_orchestrator
            .ask(
                repo,
                "warm up",
                &[],
                &mut MockModelAdapter::new("Ready."),
                &mut on_token,
            )
            .unwrap()
            .session
            .id;
        engine
            .session_store
            .set_session_mode(&session_id, SessionMode::Ask, "user")
            .unwrap();

        let mut adapter = MockModelAdapter::new_sequence_with_tool_calls(
            vec![String::new(), REFUSED_TURN_ANSWER.to_string()],
            vec![
                vec![ToolCall {
                    id: "call_1".to_string(),
                    name: "propose_patch".to_string(),
                    arguments_json: r#"{"summary":"Add a file","files":[{"path":"new.txt","content":"hello\n"}]}"#
                        .to_string(),
                }],
                Vec::new(),
            ],
        );
        let cancel = CancelToken::new();
        let mut on_progress = |_event: TurnProgress| {};
        let mut sink = TurnSink {
            on_token: &mut on_token,
            on_progress: &mut on_progress,
            cancel: &cancel,
            steering: None,
        };
        engine
            .chat_orchestrator
            .ask_with_session(
                repo,
                "Add new.txt",
                &[],
                Some(&session_id),
                &mut adapter,
                &mut sink,
            )
            .unwrap();
        (session_id, adapter)
    }

    /// Spec 20 §5.6, "the turn says which mode blocked it", end to end: a
    /// refusal needs no rendering of its own because it reaches the user by
    /// two existing routes. It is replayed to the model as the refused call's
    /// tool result, so the model's next answer can explain it, and the session
    /// payload the UI renders on load carries that tool result too.
    #[test]
    fn a_refused_call_reaches_the_model_and_the_session_payload() {
        let repo = temp_path("mode-refusal-e2e");
        fs::create_dir_all(&repo).unwrap();
        fs::write(repo.join("README.md"), "# Refusal\n").unwrap();
        let (session_id, adapter) = refused_turn_in_ask_mode(&repo);

        let refusal = "Refused: Ask mode does not allow this. Switch to Code mode to allow it.";
        let replayed = adapter.requests[1]
            .messages
            .iter()
            .find(|message| message.role == "tool")
            .expect("the refused call's result is replayed to the model");
        assert_eq!(replayed.tool_call_id.as_deref(), Some("call_1"));
        assert!(replayed.content.contains(refusal), "{}", replayed.content);

        let (port, token) = serve_for_test();
        let payload = get_session_for_test(port, &token, &session_id);
        assert!(payload.contains(REFUSED_TURN_ANSWER), "{payload}");
        assert!(payload.contains("\"role\":\"tool\""), "{payload}");
        assert!(payload.contains(refusal), "{payload}");
        assert!(!repo.join("new.txt").exists());

        fs::remove_dir_all(&repo).ok();
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

    /// Local runtimes do not authenticate. A missing key must not stop a
    /// request to a loopback endpoint, while a remote endpoint still fails
    /// loudly instead of sending an unauthenticated request.
    #[test]
    fn a_missing_model_key_is_allowed_only_for_a_loopback_endpoint() {
        let mut config = Config {
            model_api_key_env: format!("DAMAIAN_UNSET_TEST_KEY_{}", std::process::id()),
            model_base_url: "http://localhost:11434".to_string(),
            ..Config::default()
        };
        assert_eq!(
            resolve_shell_model_api_key(&config),
            Ok(String::new()),
            "local Ollama serves without a key"
        );

        config.model_base_url = "https://api.example.test".to_string();
        assert!(
            resolve_shell_model_api_key(&config).is_err(),
            "a remote endpoint without a key is still a configuration error"
        );
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

    /// A checkout whose `.damaian/config.conf` holds `repository_config`.
    fn policy_fixture(name: &str, repository_config: &str) -> PathBuf {
        isolated_data_dir();
        let repo = temp_path(name);
        fs::create_dir_all(repo.join(".damaian")).expect("repository");
        fs::write(repo.join(".damaian").join("config.conf"), repository_config)
            .expect("repository config");
        repo
    }

    fn get_effective_policy_for_test(port: u16, token: &str, query: &str) -> String {
        send_for_test(
            port,
            format!(
                "GET /api/effective-policy?{query} HTTP/1.1\r\nHost: 127.0.0.1\r\nx-damaian-api-token: {token}\r\nconnection: close\r\n\r\n"
            ),
        )
    }

    /// Every refusal in the policy as `(key, class, by)`, from the rules and
    /// from `otherRefused`.
    fn refusals_of(json: &serde_json::Value) -> Vec<(String, String, String)> {
        let rules = json["rules"].as_array().expect("rules");
        rules
            .iter()
            .flat_map(|rule| rule["refused"].as_array().expect("refused").iter())
            .chain(json["otherRefused"].as_array().expect("otherRefused"))
            .map(|refused| {
                (
                    refused["key"].as_str().unwrap().to_string(),
                    refused["class"].as_str().unwrap().to_string(),
                    refused["by"].as_str().unwrap().to_string(),
                )
            })
            .collect()
    }

    fn rule_of<'a>(json: &'a serde_json::Value, key: &str) -> &'a serde_json::Value {
        json["rules"]
            .as_array()
            .expect("rules")
            .iter()
            .find(|rule| rule["key"] == key)
            .unwrap_or_else(|| panic!("{key} has no rule in {json}"))
    }

    /// Task 6's shape over the wire: per-entry sources, a refusal by key and
    /// class, and the refused value nowhere in the response (`context.md` §8).
    /// The rules must also be the lines of the text the provider syncing
    /// parses, loaded for the same repository.
    #[test]
    fn get_effective_policy_serves_the_attributed_policy_without_a_refused_value() {
        let repo = policy_fixture(
            "policy-view",
            "restricted_patterns=secrets/**\n\
             require_approval_for_file_edits=false\n\
             shell=/tmp/evil-shell-task7\n",
        );
        let (port, token) = serve_for_test();

        let response =
            get_effective_policy_for_test(port, &token, &format!("repo={}", repo.display()));

        assert!(response.starts_with("HTTP/1.1 200"), "{response}");
        assert!(!response.contains("evil-shell-task7"), "{response}");
        let json = json_of(&response);
        assert_eq!(json["profile"], "full");
        assert_eq!(json["profileSelected"], false);
        assert!(json["mode"].is_null(), "{json}");
        assert_eq!(json["header"], "Full repository development");
        let entries = rule_of(&json, "restricted_patterns")["entries"]
            .as_array()
            .expect("a list rule carries entries");
        assert!(
            entries
                .iter()
                .any(|entry| entry["value"] == "secrets/**"
                    && entry["source"]["kind"] == "repository"),
            "{entries:?}"
        );
        let refusals = refusals_of(&json);
        for expected in [
            (
                "require_approval_for_file_edits",
                "restrict_only",
                "repository",
            ),
            ("shell", "forbidden", "repository"),
        ] {
            assert!(
                refusals.iter().any(
                    |(key, class, by)| (key.as_str(), class.as_str(), by.as_str()) == expected
                ),
                "{expected:?} missing from {refusals:?}"
            );
        }
        let (text, error) = effective_policy_for_repo(repo.to_str().unwrap());
        assert!(error.is_empty(), "{error}");
        let lines: Vec<String> = json["rules"]
            .as_array()
            .unwrap()
            .iter()
            .map(|rule| {
                format!(
                    "{}={}",
                    rule["key"].as_str().unwrap(),
                    rule["value"].as_str().unwrap()
                )
            })
            .collect();
        assert_eq!(lines, text.lines().collect::<Vec<_>>());
    }

    /// Proposal §5.6 in the header: the session's mode, intersected. A session
    /// from another checkout is refused rather than shown against this one.
    #[test]
    fn get_effective_policy_intersects_the_sessions_mode() {
        let repo = policy_fixture("policy-mode", "");
        let engine = engine_for_repo(repo.to_str().unwrap()).expect("engine");
        let repository_id = engine.indexer.repository_id_for_path(&repo).unwrap();
        let session_id = engine
            .session_store
            .create_session(&repository_id, "Policy")
            .unwrap()
            .id;
        engine
            .session_store
            .set_session_mode(&session_id, SessionMode::Ask, "user")
            .unwrap();
        let foreign = engine
            .session_store
            .create_session("repo_somewhere_else", "Foreign")
            .unwrap()
            .id;
        let (port, token) = serve_for_test();

        let response = get_effective_policy_for_test(
            port,
            &token,
            &format!("repo={}&session={session_id}", repo.display()),
        );
        assert!(response.starts_with("HTTP/1.1 200"), "{response}");
        let json = json_of(&response);
        assert_eq!(json["mode"], "ask");
        assert_eq!(json["header"], "Full repository development ∩ Ask mode");

        let refused = get_effective_policy_for_test(
            port,
            &token,
            &format!("repo={}&session={foreign}", repo.display()),
        );
        assert!(!refused.starts_with("HTTP/1.1 200"), "{refused}");
        assert!(refused.contains("another repository"), "{refused}");
    }

    /// The selection is written where `damaian profile-set` writes it, and
    /// audited. The follow-up GET is what proves it persisted: an endpoint that
    /// only answered with the requested profile would pass the first half.
    #[test]
    fn post_permission_profile_writes_audits_and_returns_the_new_policy() {
        let repo = policy_fixture("policy-select", "");
        let (port, token) = serve_for_test();
        let query = format!("repo={}", repo.display());

        let selected = post_form_for_test(
            port,
            &token,
            "/api/permission-profile",
            &format!("{query}&profile=safe_local"),
        );

        assert!(selected.starts_with("HTTP/1.1 200"), "{selected}");
        let json = json_of(&selected);
        assert_eq!(json["profile"], "safe_local");
        assert_eq!(json["profileSelected"], true);
        let edits = rule_of(&json, "require_approval_for_file_edits");
        assert_eq!(edits["value"], "true");
        assert_eq!(edits["sources"][0]["kind"], "profile");
        assert_eq!(rule_of(&json, "command_access")["value"], "local");

        let repository_id = workspace_engine::hash::repository_id_for_root(&repo);
        let user_config =
            fs::read_to_string(isolated_data_dir().join("config").join("user.conf")).unwrap();
        assert!(
            user_config.contains(&format!("permission_profile.{repository_id}=safe_local")),
            "{user_config}"
        );
        let audit =
            fs::read_to_string(isolated_data_dir().join("audit").join("events.jsonl")).unwrap();
        assert!(
            audit
                .lines()
                .any(|line| line.contains("permission_profile_set")
                    && line.contains(&repository_id)
                    && line.contains("safe_local")),
            "{audit}"
        );
        let reread = json_of(&get_effective_policy_for_test(port, &token, &query));
        assert_eq!(reread["profile"], "safe_local");

        let rejected = post_form_for_test(
            port,
            &token,
            "/api/permission-profile",
            &format!("{query}&profile=sideways"),
        );
        assert!(!rejected.starts_with("HTTP/1.1 200"), "{rejected}");
        assert!(rejected.contains("sideways"), "{rejected}");
        let reread = json_of(&get_effective_policy_for_test(port, &token, &query));
        assert_eq!(
            reread["profile"], "safe_local",
            "a refused name must not be written"
        );
    }

    fn get_for_test(port: u16, token: &str, path_and_query: &str) -> String {
        send_for_test(
            port,
            format!(
                "GET {path_and_query} HTTP/1.1\r\nHost: 127.0.0.1\r\nx-damaian-api-token: {token}\r\nconnection: close\r\n\r\n"
            ),
        )
    }

    /// Spec 31 Task 8 in the shell: export a built-in, preview the import
    /// (nothing written), import it under a new name, find it in the custom
    /// listing and select it. The user's own `user.conf` lines survive, since
    /// an import writes a profile file and never user config (Task 7,
    /// deviation 6).
    #[test]
    fn a_built_in_exported_and_imported_under_a_new_name_can_be_selected() {
        let repo = policy_fixture("profile-import", "");
        let user_config = isolated_data_dir().join("config").join("user.conf");
        fs::create_dir_all(user_config.parent().unwrap()).unwrap();
        fs::write(
            &user_config,
            "max_file_bytes=4096\nignore_patterns=target/\n",
        )
        .unwrap();
        let (port, token) = serve_for_test();

        let exported = get_for_test(
            port,
            &token,
            "/api/permission-profile-export?profile=safe_local",
        );
        assert!(exported.starts_with("HTTP/1.1 200"), "{exported}");
        let text = json_of(&exported)["text"].as_str().unwrap().to_string();
        assert!(text.contains("command_access=local"), "{text}");
        let import_form = |extra: &str| {
            format!(
                "repo={}&name=my_safe&text={}{extra}",
                repo.display(),
                percent_encode_for_test(&text)
            )
        };
        let profile_file = isolated_data_dir()
            .join("config")
            .join("profiles")
            .join("my_safe.conf");
        // Read-only is in force here, but the import is compared with the
        // config a profile would narrow, so `local` is not called loosening.
        let narrowed = post_form_for_test(
            port,
            &token,
            "/api/permission-profile",
            &format!("repo={}&profile=read_only", repo.display()),
        );
        assert!(narrowed.starts_with("HTTP/1.1 200"), "{narrowed}");

        let preview = post_form_for_test(
            port,
            &token,
            "/api/permission-profile-import",
            &import_form("&preview=true"),
        );
        assert!(preview.starts_with("HTTP/1.1 200"), "{preview}");
        let preview = json_of(&preview);
        assert_eq!(preview["written"], false);
        assert_eq!(preview["exists"], false);
        assert!(
            preview["notCarried"].as_array().unwrap().is_empty(),
            "{preview}"
        );
        assert!(
            preview["loosening"].as_array().unwrap().is_empty(),
            "{preview}"
        );
        assert!(!profile_file.exists(), "a preview wrote the profile");

        let imported = post_form_for_test(
            port,
            &token,
            "/api/permission-profile-import",
            &import_form(""),
        );
        assert!(imported.starts_with("HTTP/1.1 200"), "{imported}");
        assert_eq!(json_of(&imported)["written"], true);
        assert!(profile_file.is_file());

        let listed = json_of(&get_for_test(port, &token, "/api/permission-profiles"));
        assert_eq!(listed["custom"], serde_json::json!(["my_safe"]));

        let selected = post_form_for_test(
            port,
            &token,
            "/api/permission-profile",
            &format!("repo={}&profile=my_safe", repo.display()),
        );
        assert!(selected.starts_with("HTTP/1.1 200"), "{selected}");
        let json = json_of(&selected);
        assert_eq!(json["profile"], "my_safe");
        assert_eq!(rule_of(&json, "command_access")["value"], "local");
        assert_eq!(
            rule_of(&json, "require_approval_for_file_edits")["value"],
            "true"
        );
        let user = fs::read_to_string(&user_config).unwrap();
        for line in ["max_file_bytes=4096", "ignore_patterns=target/", "=my_safe"] {
            assert!(user.contains(line), "{line} missing from {user}");
        }
    }

    /// The preview lists, by key and class, what a profile cannot carry and
    /// what would loosen this checkout's config, and writes nothing. The
    /// refused value is not in the response. A reserved name is refused at
    /// preview, and an existing one at import unless replacing.
    #[test]
    fn a_profile_import_preview_lists_what_will_not_apply_without_its_values() {
        let repo = policy_fixture("profile-import-review", "command_access=read_only\n");
        let (port, token) = serve_for_test();
        let text = "shell=/tmp/evil-shell-task8\ncommand_access=all\nallow_file_edits=false\n";
        let post = |name: &str, extra: &str| {
            post_form_for_test(
                port,
                &token,
                "/api/permission-profile-import",
                &format!(
                    "repo={}&name={name}&text={}{extra}",
                    repo.display(),
                    percent_encode_for_test(text)
                ),
            )
        };

        let preview = post("reviewed", "&preview=true");

        assert!(preview.starts_with("HTTP/1.1 200"), "{preview}");
        assert!(!preview.contains("evil-shell-task8"), "{preview}");
        let json = json_of(&preview);
        assert_eq!(
            json["notCarried"],
            serde_json::json!([{ "key": "shell", "class": "forbidden" }])
        );
        assert_eq!(
            json["loosening"],
            serde_json::json!([{ "key": "command_access", "class": "restrict_only" }])
        );
        assert_eq!(
            json["carried"],
            serde_json::json!(["allow_file_edits", "command_access"])
        );
        let profiles = isolated_data_dir().join("config").join("profiles");
        assert!(!profiles.join("reviewed.conf").exists());

        let reserved = post("full", "&preview=true");
        assert!(!reserved.starts_with("HTTP/1.1 200"), "{reserved}");
        assert!(reserved.contains("built-in"), "{reserved}");

        assert!(post("reviewed", "").starts_with("HTTP/1.1 200"));
        let again = post("reviewed", "");
        assert!(again.contains("already exists"), "{again}");
        let replaced = post("reviewed", "&replace=true");
        assert_eq!(json_of(&replaced)["replaced"], true, "{replaced}");
        let audit =
            fs::read_to_string(isolated_data_dir().join("audit").join("events.jsonl")).unwrap();
        assert!(audit.contains("permission_profile_imported"), "{audit}");
        assert!(!audit.contains("evil-shell-task8"), "{audit}");
    }

    /// Criterion 4 for a hand-edited custom profile (Task 3's note for Task 7):
    /// the shell audits what the profile could not apply, by key and class.
    /// The refused value reaches neither the response nor the audit log.
    #[test]
    fn a_custom_profiles_refused_keys_are_audited_when_the_shell_shows_it() {
        let repo = policy_fixture("policy-custom", "");
        let profiles = isolated_data_dir().join("config").join("profiles");
        fs::create_dir_all(&profiles).unwrap();
        fs::write(
            profiles.join("task7_custom.conf"),
            "command_access=read_only\nshell=/tmp/custom-shell-task7\n",
        )
        .unwrap();
        let (port, token) = serve_for_test();

        let selected = post_form_for_test(
            port,
            &token,
            "/api/permission-profile",
            &format!("repo={}&profile=task7_custom", repo.display()),
        );

        assert!(selected.starts_with("HTTP/1.1 200"), "{selected}");
        assert!(!selected.contains("custom-shell-task7"), "{selected}");
        let json = json_of(&selected);
        assert_eq!(rule_of(&json, "command_access")["value"], "read_only");
        assert!(
            refusals_of(&json).contains(&("shell".into(), "forbidden".into(), "profile".into())),
            "{json}"
        );
        let audit =
            fs::read_to_string(isolated_data_dir().join("audit").join("events.jsonl")).unwrap();
        assert!(
            audit
                .lines()
                .any(|line| line.contains("permission_profile_key_rejected")
                    && line.contains("task7_custom")
                    && line.contains("\"shell\"")),
            "{audit}"
        );
        assert!(!audit.contains("custom-shell-task7"), "{audit}");
    }

    /// Repository config is untrusted input, so a key it cannot parse is
    /// skipped and reported rather than failing the load (spec 34 §7). This
    /// panel therefore still shows the effective policy — a repository cannot
    /// blank it out by shipping one bad line.
    #[test]
    fn effective_policy_survives_an_unparsable_repository_key() {
        let repo = temp_path("invalid-effective-policy");
        fs::create_dir_all(repo.join(".damaian")).unwrap();
        fs::write(
            repo.join(".damaian").join("config.conf"),
            "unknown_key=value\nignore_patterns=vendor/\n",
        )
        .unwrap();

        let (policy, error) = effective_policy_for_repo(repo.to_str().unwrap());

        assert!(error.is_empty(), "unexpected error: {error}");
        assert!(policy.contains("vendor/"), "{policy}");
        assert!(!policy.contains("unknown_key"), "{policy}");
    }

    // With no repository selected there is nothing to review, and the answer
    // must not touch the user's real data directory to say so — every other
    // path through this endpoint loads and writes user-scope state, which is
    // why the engine-level behaviour is tested in
    // `workspace-engine/tests/repository_config_trust.rs` instead.
    #[test]
    fn repository_config_review_is_empty_without_a_repository() {
        assert_eq!(
            repository_config_review_json("  ").unwrap(),
            "{\"rejectedKeys\":[],\"allowlistEntries\":[],\"hooks\":[]}"
        );
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

    // Spec 19: a reloaded conversation shows what each past turn spent, and
    // says when the figure is an approximation.
    #[test]
    fn the_session_payload_carries_each_tasks_usage() {
        let statuses = HashMap::from([("task_1".to_string(), "complete".to_string())]);
        let usage = HashMap::from([(
            "task_1".to_string(),
            TaskUsage {
                input_tokens: 1200,
                output_tokens: 340,
                source: UsageSource::Estimated,
                reported_cost: None,
                run_count: 1,
                cached_input_tokens: None,
                cache_reported_input_tokens: 0,
                runs_without_cache_report: 1,
            },
        )]);

        let json = task_states_json(
            &statuses,
            &usage,
            &HashMap::new(),
            &HashMap::new(),
            &HashMap::new(),
            &BTreeMap::new(),
            &Config::default(),
        );

        assert!(json.contains("\"inputTokens\":1200"), "{json}");
        assert!(json.contains("\"outputTokens\":340"), "{json}");
        // The marker requirement 4 turns on: the shell has to be able to tell
        // the user this is an approximation rather than a measurement.
        assert!(json.contains("\"usageSource\":\"estimated\""), "{json}");
        assert!(json.contains("\"runCount\":1"), "{json}");
        // Nothing reported a cost, so no cost field at all — a zero would read
        // as "this turn was free".
        assert!(!json.contains("reportedCost"), "{json}");
        // Spec 49: this provider reported no cache split, so no cache fields
        // at all. A `0` here renders as "caching is broken" when the truth is
        // "we cannot see it".
        assert!(!json.contains("cachedInputTokens"), "{json}");
        assert!(!json.contains("cacheHitRate"), "{json}");
    }

    /// Spec 49 task 6: what the client is told about the cache.
    mod cache_fields {
        use super::*;
        use workspace_engine::CostEstimate;

        fn usage_with(cached: Option<u64>, reported_input: u64, silent_runs: u32) -> TaskUsage {
            TaskUsage {
                input_tokens: 10_000,
                output_tokens: 500,
                source: UsageSource::Measured,
                reported_cost: None,
                run_count: 2,
                cached_input_tokens: cached,
                cache_reported_input_tokens: reported_input,
                runs_without_cache_report: silent_runs,
            }
        }

        fn json_for(usage: TaskUsage, estimate: Option<CostEstimate>) -> String {
            task_usage_json(Some(&usage), estimate)
        }

        /// A real upper-bound estimate, built through `Config::estimated_cost`
        /// rather than a constructor: `CostEstimate`'s are crate-private to
        /// the engine on purpose, so nothing outside it can mint a figure and
        /// call it exact.
        fn upper_bound_estimate() -> CostEstimate {
            let mut config = Config::default();
            config.apply_overlay(
                workspace_engine::ConfigOverlay::parse(concat!(
                    "model_provider=deepseek\n",
                    "model_provider.deepseek.price_per_million_input_tokens=1.0\n",
                    "model_provider.deepseek.price_per_million_output_tokens=1.0\n",
                ))
                .unwrap(),
            );
            let estimate = config
                .estimated_cost(&workspace_engine::TokenUsage {
                    input_tokens: 1_000_000,
                    output_tokens: 0,
                    cached_input_tokens: Some(500_000),
                    source: UsageSource::Measured,
                })
                .expect("both base rates are set");
            assert!(estimate.is_upper_bound(), "no cached rate is configured");
            estimate
        }

        #[test]
        fn a_reported_split_carries_the_count_and_the_rate() {
            let json = json_for(usage_with(Some(6_000), 8_000, 0), None);

            assert!(json.contains("\"cachedInputTokens\":6000"), "{json}");
            // 6000 of the 8000 input tokens whose runs reported a split — not
            // of the task's 10000, which would dilute the rate with runs
            // nobody can see into.
            assert!(json.contains("\"cacheHitRate\":0.75"), "{json}");
            assert!(json.contains("\"runsWithoutCacheReport\":0"), "{json}");
        }

        #[test]
        fn an_unreported_split_omits_the_fields_rather_than_zeroing_them() {
            let json = json_for(usage_with(None, 0, 2), None);

            assert!(!json.contains("cachedInputTokens"), "{json}");
            assert!(!json.contains("cacheHitRate"), "{json}");
            // The count of silent runs is still carried: it is what lets the
            // UI say "not reported" rather than guess.
            assert!(json.contains("\"runsWithoutCacheReport\":2"), "{json}");
        }

        #[test]
        fn a_reported_zero_is_a_rate_of_zero_not_an_absent_field() {
            // The provider measured that none of it hit. That is a fact and
            // renders as 0%, unlike silence, which renders as "not reported".
            let json = json_for(usage_with(Some(0), 8_000, 0), None);

            assert!(json.contains("\"cachedInputTokens\":0"), "{json}");
            assert!(json.contains("\"cacheHitRate\":0"), "{json}");
        }

        #[test]
        fn a_partial_report_says_how_many_runs_it_covers() {
            let json = json_for(usage_with(Some(500), 1_000, 3), None);

            assert!(json.contains("\"cacheHitRate\":0.5"), "{json}");
            assert!(json.contains("\"runsWithoutCacheReport\":3"), "{json}");
        }

        #[test]
        fn a_zero_denominator_yields_no_rate_rather_than_a_division_by_zero() {
            // Reachable: a run refused by the provider records measured zero,
            // whose `cached_input_tokens` is `Some(0)` with no input tokens.
            let json = json_for(usage_with(Some(0), 0, 0), None);

            assert!(!json.contains("cacheHitRate"), "{json}");
            assert!(json.contains("\"cachedInputTokens\":0"), "{json}");
        }

        #[test]
        fn an_upper_bound_cost_says_so() {
            let json = json_for(
                usage_with(Some(6_000), 8_000, 0),
                Some(upper_bound_estimate()),
            );

            assert!(json.contains("\"estimatedCost\":1"), "{json}");
            assert!(
                json.contains("\"estimatedCostIsUpperBound\":true"),
                "{json}"
            );
        }
    }

    #[test]
    fn a_reopened_session_carries_each_turns_notes() {
        let statuses = HashMap::from([("task_1".to_string(), "complete".to_string())]);
        let notes = BTreeMap::from([(
            "task_1".to_string(),
            TurnNotes {
                text: "Approvals live in command_policy.rs.".to_string(),
                carried: true,
                chars: 38,
                total_chars: 38,
            },
        )]);

        let json = task_states_json(
            &statuses,
            &HashMap::new(),
            &HashMap::new(),
            &HashMap::new(),
            &HashMap::new(),
            &notes,
            &Config::default(),
        );

        assert!(
            json.contains(
                "\"notes\":{\"text\":\"Approvals live in command_policy.rs.\",\"carried\":true}"
            ),
            "{json}"
        );
    }

    #[test]
    fn phases_carry_the_notes_size() {
        let phase = TurnPhase {
            kind: PhaseKind::Model,
            label: String::new(),
            round: 1,
            max_rounds: 8,
            exploration_round: 2,
            max_exploration_rounds: 24,
            notes_chars: 2_100,
        };

        let json = phase_json(&phase);

        assert!(json.contains("\"explorationRound\":2"), "{json}");
        assert!(json.contains("\"notesChars\":2100"), "{json}");
    }

    #[test]
    fn a_task_with_no_usage_reports_none_rather_than_zero() {
        // A session written before usage existed. Absent, not zero: a zero is
        // indistinguishable from a real measurement of nothing.
        let statuses = HashMap::from([("task_1".to_string(), "complete".to_string())]);

        let json = task_states_json(
            &statuses,
            &HashMap::new(),
            &HashMap::new(),
            &HashMap::new(),
            &HashMap::new(),
            &BTreeMap::new(),
            &Config::default(),
        );

        assert!(json.contains("\"id\":\"task_1\""), "{json}");
        assert!(!json.contains("inputTokens"), "{json}");
    }

    #[test]
    fn a_reopened_session_still_carries_each_turns_plan() {
        // The completion report is the part a user comes back for, and it
        // would be the one thing a reload lost if the plan travelled only on
        // the live turn's SSE channel.
        let statuses = HashMap::from([("task_1".to_string(), "complete".to_string())]);
        let plans = HashMap::from([("task_1".to_string(), sample_plan())]);

        let json = task_states_json(
            &statuses,
            &HashMap::new(),
            &plans,
            &HashMap::new(),
            &HashMap::new(),
            &BTreeMap::new(),
            &Config::default(),
        );

        assert!(json.contains("\"plan\":{"), "{json}");
        assert!(
            json.contains("\"title\":\"Read the retry helper\""),
            "{json}"
        );
        assert!(json.contains("\"outcome\":\"verified\""), "{json}");
    }

    #[test]
    fn a_turn_that_never_planned_carries_no_plan_field() {
        // Absent, not an empty plan: a trivial turn and a plan that proposed
        // nothing are different facts, and the panel must not appear for the
        // first (§5.1).
        let statuses = HashMap::from([("task_1".to_string(), "complete".to_string())]);

        let json = task_states_json(
            &statuses,
            &HashMap::new(),
            &HashMap::new(),
            &HashMap::new(),
            &HashMap::new(),
            &BTreeMap::new(),
            &Config::default(),
        );

        assert!(!json.contains("\"plan\""), "{json}");
    }

    fn sample_web_diagnostic_record(task_id: &str) -> WebDiagnosticRecord {
        WebDiagnosticRecord {
            id: "webdiagrec_1".to_string(),
            task_id: task_id.to_string(),
            tool: "inspect_web_page".to_string(),
            url: "http://localhost:5001/".to_string(),
            recorded_at_ms: 1_758_800_000_000,
            report: WebDiagnosticReport::from_text(
                r#"{"final_url": "http://localhost:5001/", "status": 200,
                    "page_errors": ["ReferenceError: game is not defined"]}"#,
                false,
            ),
        }
    }

    /// Spec 12 `context.md` §3.3: the card must survive a reload, so the
    /// recorded report rides on `/api/session`'s task, whole.
    #[test]
    fn a_reopened_session_carries_each_turns_web_diagnostics() {
        isolated_data_dir();
        let engine = default_engine().expect("default engine");
        let session = engine
            .session_store
            .create_session("repo_web_diagnostic_test", "diagnosed")
            .expect("create session");
        let task = engine
            .session_store
            .create_task(&session.id, "why is it blank?", "mock", "m")
            .expect("create task");
        engine
            .session_store
            .append_web_diagnostic(&session.id, &sample_web_diagnostic_record(&task.id))
            .expect("append record");
        let (port, token) = serve_for_test();

        let response = get_session_for_test(port, &token, &session.id);

        assert!(response.starts_with("HTTP/1.1 200"), "{response}");
        let body = response.split("\r\n\r\n").nth(1).expect("a body");
        let json: serde_json::Value = serde_json::from_str(body).expect("JSON body");
        let task_json = json["tasks"]
            .as_array()
            .expect("tasks")
            .iter()
            .find(|entry| entry["id"] == task.id.as_str())
            .expect("the task");
        assert_eq!(
            task_json["webDiagnostics"][0]["report"]["details"]["page_errors"][0],
            "ReferenceError: game is not defined",
            "{body}"
        );
        assert_eq!(task_json["webDiagnostics"][0]["taskId"], task.id.as_str());
    }

    #[test]
    fn a_turn_that_never_diagnosed_carries_no_web_diagnostics_field() {
        let statuses = HashMap::from([("task_1".to_string(), "complete".to_string())]);
        let web_diagnostics = HashMap::from([(
            "task_2".to_string(),
            vec![sample_web_diagnostic_record("task_2")],
        )]);

        let json = task_states_json(
            &statuses,
            &HashMap::new(),
            &HashMap::new(),
            &HashMap::new(),
            &web_diagnostics,
            &BTreeMap::new(),
            &Config::default(),
        );

        assert!(json.contains("\"id\":\"task_1\""), "{json}");
        assert!(!json.contains("webDiagnostics"), "{json}");
    }

    /// Recurses until 4 MiB of stack is in use, measured from the addresses
    /// of the first and deepest frames' locals rather than assumed from a
    /// frame size, which debug builds inflate. 4 MiB is twice a default
    /// spawned thread's stack and half the shell's; on a 2 MiB thread this
    /// aborts the test process with a stack overflow.
    fn use_stack_down_to(top: usize) -> usize {
        let marker = std::hint::black_box([0u8; 1024]);
        let here = marker.as_ptr() as usize;
        // The stack grows down on every target this app builds for.
        if top - here >= 4 * 1024 * 1024 {
            return top - here;
        }
        use_stack_down_to(top).max(marker[0] as usize)
    }

    #[test]
    fn the_shell_stack_holds_what_a_default_thread_cannot() {
        let handle = spawn_with_shell_stack("stack-test", || {
            let top = std::hint::black_box([0u8; 1]);
            use_stack_down_to(top.as_ptr() as usize)
        })
        .unwrap();
        let used = handle.join().expect("8 MiB holds 4 MiB of frames");
        assert!(used >= 4 * 1024 * 1024);
    }

    /// The shell passes no steering, so these never happen in a turn it hosts;
    /// were one to arrive, it must not become an SSE event the UI does not know.
    #[test]
    fn steering_progress_streams_nothing() {
        for progress in [
            TurnProgress::Paused,
            TurnProgress::Resumed,
            TurnProgress::NoteAdded {
                message_id: "msg_1".to_string(),
            },
        ] {
            assert!(turn_progress_event(progress).is_none());
        }
    }

    #[test]
    fn a_web_diagnostic_progress_event_streams_as_web_diagnostic() {
        let record = sample_web_diagnostic_record("task_1");

        let event = turn_progress_event(TurnProgress::WebDiagnostic(Box::new(record.clone())));
        let Some(TurnEvent::WebDiagnostic(streamed)) = event else {
            panic!("expected TurnEvent::WebDiagnostic");
        };
        let mut out = Vec::new();
        write_sse_event(&mut out, "web_diagnostic", &web_diagnostic_json(&streamed)).unwrap();

        let text = String::from_utf8(out).unwrap();
        let data = text
            .strip_prefix("event: web_diagnostic\ndata: ")
            .and_then(|rest| rest.strip_suffix("\n\n"))
            .unwrap_or_else(|| panic!("not a web_diagnostic event: {text}"));
        let parsed: WebDiagnosticRecord = serde_json::from_str(data).expect("record JSON");
        assert_eq!(parsed, record);
        assert!(data.contains("\"recordedAtMs\":"), "{data}");
    }

    /// Spec 12 `context.md` §3.4: reveal reuses the artifact path check, so
    /// nothing outside `<data-dir>/web-diagnostics/` can be shown in Finder.
    #[test]
    fn web_diagnostic_reveal_target_accepts_only_artifacts_under_the_data_dir() {
        let data_dir = std::env::temp_dir().join(format!(
            "damaian-reveal-target-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let run_dir = data_dir.join("web-diagnostics/session_1/task_1/run-1");
        fs::create_dir_all(&run_dir).unwrap();
        fs::write(run_dir.join("page.png"), b"png").unwrap();
        fs::write(data_dir.join("config-secret.txt"), b"no").unwrap();
        let config = Config {
            data_dir: data_dir.clone(),
            enable_index_watcher: false,
            ..Config::default()
        };

        let accepted = web_diagnostic_reveal_target(
            &config,
            "web-diagnostics/session_1/task_1/run-1/page.png",
        )
        .expect("a real artifact is accepted");
        assert!(accepted.ends_with("run-1/page.png"), "{accepted:?}");

        for refused in [
            "config-secret.txt",
            "sessions/session_1.jsonl",
            "/etc/hosts",
            "web-diagnostics/../config-secret.txt",
        ] {
            assert!(
                web_diagnostic_reveal_target(&config, refused).is_err(),
                "{refused} must be refused"
            );
        }

        fs::remove_dir_all(data_dir).unwrap();
    }

    // The list view needs the counts and the coverage flag, and must not grow
    // a content field: the manifest deliberately has none.
    #[test]
    fn checkpoint_list_json_reports_what_the_list_view_shows() {
        let manifest = test_manifest();

        let json = checkpoint_list_json(&[manifest]);

        assert!(json.contains("\"checkpointId\":\"checkpoint_1\""));
        assert!(json.contains("\"summary\":\"Before: bump the version\""));
        assert!(json.contains("\"fileCount\":1"));
        assert!(json.contains("\"excludedCount\":1"));
        assert!(json.contains("\"commandEffectsCovered\":true"));
        assert!(json.contains("\"restoredAtMs\":null"));
        assert!(json.contains("\"userMessageId\":\"msg_1\""));
        // The per-file list is what lets the UI offer "restore only this file".
        // Paths and origins only: the manifest has no content and this must not
        // become the place that acquires it.
        assert!(json.contains("\"files\":[{\"path\":\"src/a.rs\",\"origin\":\"patch\"}]"));
        assert!(json.contains("\"pendingApprovals\":[]"));
        assert!(!json.contains("hash"));
    }

    // A rewind that skipped a file and one that refused to overwrite it are
    // different outcomes, and the UI has to be able to say which happened.
    #[test]
    fn checkpoint_restore_json_keeps_skipped_and_conflicted_apart() {
        let result = CheckpointRestoreResult {
            checkpoint_id: "checkpoint_1".to_string(),
            restored_files: vec!["src/a.rs".to_string()],
            deleted_files: Vec::new(),
            skipped_files: vec!["src/b.rs".to_string()],
            conflicted_files: vec!["src/c.rs".to_string()],
            conversation_restored: true,
            warnings: vec!["src/c.rs: changed since the checkpoint was taken".to_string()],
        };

        let json = checkpoint_restore_json(&result);

        assert!(json.contains("\"restoredFiles\":[\"src/a.rs\"]"));
        assert!(json.contains("\"skippedFiles\":[\"src/b.rs\"]"));
        assert!(json.contains("\"conflictedFiles\":[\"src/c.rs\"]"));
        assert!(json.contains("\"conversationRestored\":true"));
    }

    fn test_manifest() -> workspace_engine::CheckpointManifest {
        workspace_engine::CheckpointManifest {
            checkpoint_id: "checkpoint_1".to_string(),
            repository_id: "repo_1".to_string(),
            session_id: "session_1".to_string(),
            task_id: Some("task_1".to_string()),
            created_at_ms: 1_788_000_000_000,
            user_message_id: Some("msg_1".to_string()),
            summary: "Before: bump the version".to_string(),
            conversation: workspace_engine::CheckpointConversation {
                last_event_seq: 12,
                task_status: "running".to_string(),
            },
            pending_approvals: Vec::new(),
            tree_oid: "abc".to_string(),
            files: vec![workspace_engine::CheckpointFile {
                path: "src/a.rs".to_string(),
                hash: Some("hash".to_string()),
                existed: true,
                oid: Some("oid".to_string()),
                origin: workspace_engine::CheckpointOrigin::Patch,
                mode: 0o100644,
                expected_hash: None,
                expected_existed: None,
            }],
            excluded: vec![workspace_engine::CheckpointExclusion {
                path: ".env".to_string(),
                reason: "restricted_pattern".to_string(),
            }],
            restored_at_ms: None,
            command_effects_covered: true,
        }
    }

    // Startup must fail loudly on a data directory this build cannot read.
    // The alternative — carrying on with an empty projects list — reads as data
    // loss and invites the destructive recovery attempt the refusal exists to
    // prevent.
    #[test]
    fn startup_refuses_a_data_directory_written_by_a_newer_schema() {
        let data_dir = temp_path("schema-newer");
        fs::create_dir_all(&data_dir).unwrap();
        fs::write(data_dir.join("schema.conf"), "schema_version=999\n").unwrap();

        let error = verify_data_dir_schema_at(&data_dir).expect_err("startup should refuse");

        assert!(
            error.contains(&data_dir.display().to_string()),
            "refusal should name the data directory: {error}"
        );
        assert!(
            error.contains("999"),
            "refusal should name the version found: {error}"
        );
    }

    #[test]
    fn startup_marks_a_fresh_data_directory_with_the_current_schema() {
        let data_dir = temp_path("schema-fresh");

        verify_data_dir_schema_at(&data_dir).expect("a fresh data directory should be accepted");

        assert_eq!(
            fs::read_to_string(data_dir.join("schema.conf")).expect("marker should be written"),
            "schema_version=1\n"
        );
    }

    // The launch sweep has to *decide* and record, not merely tidy the
    // directory. An entry naming a pid that cannot exist, owned by an instance
    // that cannot exist, is the shape a crashed instance leaves behind.
    #[test]
    fn startup_sweeps_an_entry_left_by_a_crashed_instance() {
        let data_dir = temp_path("startup-sweep");
        fs::create_dir_all(data_dir.join("processes")).unwrap();
        fs::write(
            data_dir.join("processes").join("4000002-111.json"),
            "{\"pid\":4000002,\"startTimeUs\":111,\"pgid\":4000002,\"kind\":\"command\",\
             \"sessionId\":\"ses_1\",\"registeredAtMs\":0,\"ownerPid\":4000001,\
             \"ownerStartTimeUs\":222}",
        )
        .unwrap();
        // Its own config rather than the machine's: whether this passes must
        // not depend on whether auditing happens to be enabled here.
        let config = workspace_engine::Config {
            data_dir: data_dir.clone(),
            ..workspace_engine::Config::default()
        };

        sweep_orphaned_processes(&config).expect("the sweep should run");

        assert!(
            fs::read_dir(data_dir.join("processes"))
                .unwrap()
                .next()
                .is_none(),
            "a spent entry is removed so it is not re-decided at every launch"
        );
        let log = fs::read_to_string(data_dir.join("audit").join("events.jsonl"))
            .expect("the sweep records what it decided");
        assert!(
            log.contains("orphan_process_already_exited"),
            "the sweep must record its decision, not just empty the directory: {log}"
        );
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
        let data_dir = std::env::temp_dir().join(format!(
            "damaian-pty-registry-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&data_dir);
        let registry = workspace_engine::ProcessRegistry::open(&data_dir).expect("registry");
        let id = super::terminal::open(&cwd, 80, 24, &data_dir).expect("open pty session");
        assert_eq!(
            registry.entries().unwrap().len(),
            1,
            "a running shell must be recorded, or a crash leaks it"
        );
        assert_eq!(
            registry.entries().unwrap()[0].1.as_ref().unwrap().kind,
            workspace_engine::ProcessKind::Terminal.as_str()
        );
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
        assert!(
            registry.entries().unwrap().is_empty(),
            "requirement 4: closing a terminal is a clean exit and leaves no entry"
        );
    }

    /// Runs git in `repo` with a fixed identity, so a commit works on a
    /// machine with no global `user.name`.
    fn git_for_test(repo: &std::path::Path, args: &[&str]) -> String {
        let output = std::process::Command::new("git")
            .arg("-C")
            .arg(repo)
            .args(["-c", "user.name=Test", "-c", "user.email=test@example.com"])
            .args([
                "-c",
                "commit.gpgsign=false",
                "-c",
                "core.hooksPath=/dev/null",
            ])
            .args(args)
            .output()
            .expect("run git");
        assert!(
            output.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).trim().to_string()
    }

    /// A repository on `main` with one commit, a modified tracked file and
    /// an untracked one, and `origin` set to `remote`.
    fn repo_bar_fixture(name: &str, remote: Option<&str>) -> PathBuf {
        let repo = temp_path(name);
        fs::create_dir_all(&repo).unwrap();
        git_for_test(&repo, &["init", "-q", "-b", "main"]);
        fs::write(repo.join("a.txt"), "one\ntwo\nthree\n").unwrap();
        git_for_test(&repo, &["add", "a.txt"]);
        git_for_test(&repo, &["commit", "-q", "-m", "initial"]);
        // Replaces one line and appends one: +2 −1 against HEAD.
        fs::write(repo.join("a.txt"), "one\nTWO\nthree\nfour\n").unwrap();
        // Untracked text counts as insertions: +3.
        fs::write(repo.join("new.txt"), "x\ny\nz\n").unwrap();
        if let Some(remote) = remote {
            git_for_test(&repo, &["remote", "add", "origin", remote]);
        }
        repo
    }

    fn git_status_for_test(repo: &std::path::Path) -> String {
        isolated_data_dir();
        let (port, token) = serve_for_test();
        get_for_test(
            port,
            &token,
            &format!(
                "/api/git-status?repo={}",
                percent_encode_for_test(&repo.to_string_lossy())
            ),
        )
    }

    /// Spec 58 requirement 11. The prefix assertion is what keeps the old
    /// fields byte-identical: it is the exact string the route served before.
    #[test]
    fn git_status_adds_branch_head_counts_and_github_url() {
        let repo = repo_bar_fixture("repobar-github", Some("git@github.com:acme/widget.git"));
        let head = git_for_test(&repo, &["rev-parse", "--short", "HEAD"]);

        let response = git_status_for_test(&repo);

        assert!(response.starts_with("HTTP/1.1 200"), "{response}");
        let body = response.split("\r\n\r\n").nth(1).unwrap();
        assert!(
            body.starts_with(
                "{\"clean\":false,\"exitCode\":0,\"files\":[\
                 {\"path\":\"a.txt\",\"raw\":\" M a.txt\",\"untracked\":false,\"conflicted\":false},\
                 {\"path\":\"new.txt\",\"raw\":\"?? new.txt\",\"untracked\":true,\"conflicted\":false}],"
            ),
            "{body}"
        );
        let payload = json_of(&response);
        assert_eq!(payload["branch"], "main", "{payload}");
        assert_eq!(payload["head"], head.as_str(), "{payload}");
        assert_eq!(payload["insertions"], 5, "{payload}");
        assert_eq!(payload["deletions"], 1, "{payload}");
        assert_eq!(
            payload["githubUrl"], "https://github.com/acme/widget",
            "{payload}"
        );
    }

    /// Requirement 12: an origin that is not github.com yields `null`, never
    /// the raw remote.
    #[test]
    fn git_status_reports_a_non_github_origin_as_null() {
        let repo = repo_bar_fixture("repobar-gitlab", Some("https://gitlab.com/acme/widget.git"));

        let response = git_status_for_test(&repo);

        assert!(response.starts_with("HTTP/1.1 200"), "{response}");
        assert!(!response.contains("gitlab"), "{response}");
        let payload = json_of(&response);
        assert_eq!(payload["branch"], "main", "{payload}");
        assert!(payload["githubUrl"].is_null(), "{payload}");
    }

    /// Requirement 14: a plain folder is the old payload plus `null`/0, not
    /// an error.
    #[test]
    fn git_status_of_a_non_git_folder_has_null_repo_fields() {
        let folder = temp_path("repobar-plain");
        fs::create_dir_all(&folder).unwrap();

        let response = git_status_for_test(&folder);

        assert!(response.starts_with("HTTP/1.1 200"), "{response}");
        let payload = json_of(&response);
        assert_eq!(payload["clean"], false, "{payload}");
        assert_ne!(payload["exitCode"], 0, "{payload}");
        assert_eq!(payload["files"], serde_json::json!([]), "{payload}");
        assert!(payload["branch"].is_null(), "{payload}");
        assert!(payload["head"].is_null(), "{payload}");
        assert_eq!(payload["insertions"], 0, "{payload}");
        assert_eq!(payload["deletions"], 0, "{payload}");
        assert!(payload["githubUrl"].is_null(), "{payload}");
    }

    /// Requirement 13: the client never supplies the URL. A `url` field in
    /// the form is ignored and the opener sees only the derived one.
    #[test]
    fn open_github_opens_the_derived_url_and_ignores_a_supplied_one() {
        isolated_data_dir();
        let repo = repo_bar_fixture("repobar-open", Some("https://github.com/acme/widget"));
        let form = HashMap::from([
            ("repo".to_string(), repo.to_string_lossy().to_string()),
            ("url".to_string(), "https://evil.example/".to_string()),
        ]);
        let mut opened = Vec::new();

        let url = super::open_github_for_form(&form, |url| {
            opened.push(url.to_string());
            Ok(())
        })
        .expect("open github");

        assert_eq!(url, "https://github.com/acme/widget");
        assert_eq!(opened, vec!["https://github.com/acme/widget".to_string()]);
    }

    /// No GitHub origin, no repository, or no folder: an error, and nothing
    /// is opened.
    #[test]
    fn open_github_refuses_without_a_github_origin() {
        isolated_data_dir();
        let gitlab = repo_bar_fixture("repobar-open-gitlab", Some("https://gitlab.com/a/b"));
        let no_origin = repo_bar_fixture("repobar-open-none", None);
        let plain = temp_path("repobar-open-plain");
        fs::create_dir_all(&plain).unwrap();
        let missing = temp_path("repobar-open-missing");

        for folder in [&gitlab, &no_origin, &plain, &missing] {
            let form = HashMap::from([("repo".to_string(), folder.to_string_lossy().to_string())]);
            let mut opened = false;
            let result = super::open_github_for_form(&form, |_| {
                opened = true;
                Ok(())
            });
            assert!(result.is_err(), "{folder:?}: {result:?}");
            assert!(!opened, "{folder:?} opened something");
        }
    }

    /// The route exists and requires `repo`. Without one it fails before
    /// anything could be opened, so this never launches a browser.
    #[test]
    fn open_github_route_requires_a_repo() {
        let (port, token) = serve_for_test();

        let response = post_fields_for_test(
            port,
            &token,
            "/api/open-github",
            &[("url", "https://evil.example/")],
        );

        assert!(response.starts_with("HTTP/1.1 500"), "{response}");
        assert!(response.contains("missing form field: repo"), "{response}");
    }

    /// Spec 57 Task 6: a worker's turns, hosted on this process by the same
    /// turn thread `--worker` runs. Its data directory is the shell's
    /// (`isolated_data_dir`), as it is in the app.
    mod worker_turns {
        use super::*;
        use crate::run_chat_request_with;
        use crate::worker::{ControlCommand, WorkerHost, host_in_process};
        use std::path::Path;
        use std::process::Command;
        use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
        use std::sync::{Arc, Condvar, Mutex};
        use workspace_engine::worker_progress::read_progress_after;
        use workspace_engine::{
            ModelAdapter, ModelRequest, ModelRun, SessionStore, TaskStatus, WorkerRecord,
        };

        static COUNTER: AtomicU64 = AtomicU64::new(1);

        /// A committed one-file repository. Canonical, because the session's
        /// repository id is taken from the canonical root.
        fn fixture(tag: &str) -> PathBuf {
            let dir = std::env::temp_dir().join(format!(
                "dwt-{}-{tag}-{}",
                std::process::id(),
                COUNTER.fetch_add(1, Ordering::Relaxed)
            ));
            let _ = fs::remove_dir_all(&dir);
            fs::create_dir_all(&dir).unwrap();
            fs::write(dir.join("README.md"), "# Worker fixture\n").unwrap();
            for args in [
                &["init", "-q"][..],
                &["add", "."][..],
                &[
                    "-c",
                    "user.name=Fixture",
                    "-c",
                    "user.email=fixture@example.test",
                    "commit",
                    "-qm",
                    "init",
                ][..],
            ] {
                let status = Command::new("git")
                    .args(args)
                    .current_dir(&dir)
                    .status()
                    .unwrap();
                assert!(status.success(), "git {args:?}");
            }
            fs::canonicalize(&dir).unwrap()
        }

        fn store() -> SessionStore {
            SessionStore::new(isolated_data_dir())
        }

        /// A session for `repo`; with `worker`, one whose worker record names
        /// `repo` as its worktree, as Task 7's spawn will write it.
        fn session_for(repo: &Path, worker: bool) -> String {
            let store = store();
            let session = store
                .create_session(
                    &workspace_engine::hash::repository_id_for_root(repo),
                    "fixture",
                )
                .unwrap();
            if worker {
                store
                    .record_worker_spawned(
                        &session.id,
                        &WorkerRecord {
                            parent_session_id: None,
                            source_repository: repo.to_path_buf(),
                            worktree_path: repo.to_path_buf(),
                            branch: "damaian/fixture".to_string(),
                            base_commit: "HEAD".to_string(),
                            included_uncommitted: false,
                            git_commands: Vec::new(),
                        },
                    )
                    .unwrap();
            }
            session.id
        }

        fn call(name: &str, arguments_json: &str) -> ToolCall {
            ToolCall {
                id: format!("call_{name}"),
                name: name.to_string(),
                arguments_json: arguments_json.to_string(),
            }
        }

        /// One tool call, then a final answer.
        fn calls_then(name: &str, arguments_json: &str, answer: &str) -> Scripted {
            Scripted::new(MockModelAdapter::new_sequence_with_tool_calls(
                vec![String::new(), answer.to_string()],
                vec![vec![call(name, arguments_json)], Vec::new()],
            ))
        }

        const PWD: &str = r#"{"command":"pwd","reason":"See where the turn runs"}"#;
        const TOUCH: &str = r#"{"command":"touch approved.txt","reason":"Mark the tree"}"#;

        /// A read-only command envelope the policy runs automatically, so a
        /// turn of it walks the round budget without pausing for a command
        /// approval (spec 61).
        fn read_only_round() -> String {
            "DAMAIAN_COMMAND_V1\nCOMMAND: pwd\nREASON: Confirm the working directory.\nEND_COMMAND\n"
                .to_string()
        }

        /// The worker builds an adapter per job; sharing one script across
        /// them lets a turn and its resume read one sequence, as one turn.
        #[derive(Clone)]
        struct Scripted(Arc<Mutex<MockModelAdapter>>);

        impl Scripted {
            fn new(adapter: MockModelAdapter) -> Self {
                Self(Arc::new(Mutex::new(adapter)))
            }
        }

        impl ModelAdapter for Scripted {
            fn stream_response(
                &mut self,
                request: &ModelRequest,
                cancel: &CancelToken,
                on_token: &mut dyn FnMut(&str),
                on_wait: &mut dyn FnMut(u64),
            ) -> workspace_engine::Result<ModelRun> {
                self.0
                    .lock()
                    .unwrap()
                    .stream_response(request, cancel, on_token, on_wait)
            }
        }

        /// Holds the turn's first model call until opened, so a test can act
        /// while a turn is certainly running.
        #[derive(Clone)]
        struct Gated {
            inner: Scripted,
            first: Arc<AtomicBool>,
            gate: Arc<(Mutex<(bool, bool)>, Condvar)>,
        }

        impl Gated {
            fn new(inner: Scripted) -> Self {
                Self {
                    inner,
                    first: Arc::new(AtomicBool::new(true)),
                    gate: Arc::new((Mutex::new((false, false)), Condvar::new())),
                }
            }

            fn wait_entered(&self) {
                let (lock, changed) = &*self.gate;
                let (state, timeout) = changed
                    .wait_timeout_while(lock.lock().unwrap(), Duration::from_secs(30), |state| {
                        !state.0
                    })
                    .unwrap();
                assert!(
                    state.0 && !timeout.timed_out(),
                    "the turn never called the model"
                );
            }

            fn open(&self) {
                let (lock, changed) = &*self.gate;
                lock.lock().unwrap().1 = true;
                changed.notify_all();
            }
        }

        impl ModelAdapter for Gated {
            fn stream_response(
                &mut self,
                request: &ModelRequest,
                cancel: &CancelToken,
                on_token: &mut dyn FnMut(&str),
                on_wait: &mut dyn FnMut(u64),
            ) -> workspace_engine::Result<ModelRun> {
                if self.first.swap(false, Ordering::SeqCst) {
                    let (lock, changed) = &*self.gate;
                    let mut state = lock.lock().unwrap();
                    state.0 = true;
                    changed.notify_all();
                    while !state.1 {
                        state = changed.wait(state).unwrap();
                    }
                }
                self.inner
                    .stream_response(request, cancel, on_token, on_wait)
            }
        }

        fn hosted<A>(repo: &Path, adapter: A) -> (Arc<WorkerHost>, String)
        where
            A: ModelAdapter + Clone + Send + 'static,
        {
            let session = session_for(repo, true);
            let host = host_in_process(
                &session,
                isolated_data_dir(),
                Box::new(move |_config: &Config| {
                    Ok(Box::new(adapter.clone()) as Box<dyn ModelAdapter>)
                }),
            )
            .unwrap();
            (host, session)
        }

        fn send(host: &WorkerHost, command: ControlCommand) -> serde_json::Value {
            host.dispatch(command).0
        }

        fn message(text: &str) -> ControlCommand {
            ControlCommand::Message {
                text: text.to_string(),
            }
        }

        fn wait_for(host: &WorkerHost, done: impl Fn(&str) -> bool) -> serde_json::Value {
            let deadline = std::time::Instant::now() + Duration::from_secs(60);
            loop {
                let snapshot = host.snapshot();
                if done(snapshot["state"].as_str().unwrap()) {
                    return snapshot;
                }
                assert!(
                    std::time::Instant::now() < deadline,
                    "the worker never settled: {snapshot}"
                );
                std::thread::sleep(Duration::from_millis(10));
            }
        }

        /// Idle or waiting for approval: the turn thread has finished its job.
        fn settle(host: &WorkerHost) -> serde_json::Value {
            wait_for(host, |state| !matches!(state, "running" | "paused"))
        }

        fn session_event_types(session: &str, after: u64) -> Vec<String> {
            let log = isolated_data_dir()
                .join("sessions")
                .join(format!("{session}.jsonl"));
            fs::read_to_string(log)
                .unwrap()
                .lines()
                .map(|line| serde_json::from_str::<serde_json::Value>(line).unwrap())
                .filter(|event| event["seq"].as_u64().unwrap() > after)
                .map(|event| event["eventType"].as_str().unwrap().to_string())
                .collect()
        }

        /// This session's audit events, in order. The audit log is shared by
        /// every test in the binary, so it is filtered by the session and its
        /// tasks.
        fn audit_event_types(session: &str) -> Vec<String> {
            let tasks: Vec<String> = store()
                .read_tasks(session)
                .unwrap()
                .into_iter()
                .map(|task| task.id)
                .collect();
            fs::read_to_string(isolated_data_dir().join("audit").join("events.jsonl"))
                .unwrap()
                .lines()
                .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
                .filter(|event| {
                    event["sessionId"] == session
                        || event["taskId"]
                            .as_str()
                            .is_some_and(|task| tasks.iter().any(|id| id == task))
                })
                .map(|event| event["eventType"].as_str().unwrap().to_string())
                .collect()
        }

        fn progress(session: &str) -> Vec<serde_json::Value> {
            read_progress_after(isolated_data_dir(), session, 0).unwrap()
        }

        fn payloads(lines: &[serde_json::Value], kind: &str) -> Vec<serde_json::Value> {
            lines
                .iter()
                .filter(|line| line["kind"] == kind)
                .map(|line| line["payload"].clone())
                .collect()
        }

        /// Requirement 4, the load-bearing test of spec 57: one scripted turn
        /// through the shell's `run_chat_request` and through the worker host
        /// writes the same session-log events and the same audit events, in the
        /// same order. Ids and timestamps differ and are not compared.
        #[test]
        fn a_turn_through_the_worker_host_matches_the_shell_host() {
            isolated_data_dir();
            let prompt = "Where does this run?";
            let script = || calls_then("run_command", PWD, "It runs in the fixture.");

            let shell_repo = fixture("sh");
            let shell_session = session_for(&shell_repo, false);
            let shell_after = store().latest_event_seq(&shell_session).unwrap();
            let form = HashMap::from([
                ("repo".to_string(), shell_repo.to_string_lossy().to_string()),
                ("prompt".to_string(), prompt.to_string()),
                ("session_id".to_string(), shell_session.clone()),
            ]);
            let (events, _discard) = std::sync::mpsc::channel();
            let shell_adapter = script();
            let shell_result =
                run_chat_request_with(&form, &CancelToken::new(), &events, &mut |_| {
                    Ok(Box::new(shell_adapter.clone()))
                })
                .unwrap();
            assert_eq!(shell_result.task.status, TaskStatus::Complete);

            let worker_repo = fixture("wk");
            let (host, worker_session) = hosted(&worker_repo, script());
            let worker_after = store().latest_event_seq(&worker_session).unwrap();
            assert_eq!(send(&host, message(prompt))["ok"], true);
            assert_eq!(settle(&host)["state"], "idle");

            let shell_audit = audit_event_types(&shell_session);
            assert!(
                shell_audit.iter().any(|event| event == "command_proposed"),
                "the scripted turn must run its command, or this compares too little: {shell_audit:?}"
            );
            assert_eq!(
                session_event_types(&worker_session, worker_after),
                session_event_types(&shell_session, shell_after),
                "session-log events differ between the hosts"
            );
            assert_eq!(
                audit_event_types(&worker_session),
                shell_audit,
                "audit events differ between the hosts"
            );
        }

        /// The engine half of "same turn": the worker's engine gets the
        /// shell's chat integrations (`chat_engine`). They change nothing a
        /// turn records unless an integration is configured, so the
        /// comparison above cannot see them; with a browser MCP server
        /// configured, the runner they install is what offers the model
        /// `inspect_web_page`. Its own data directory, because the server is
        /// set in user config.
        #[test]
        fn a_worker_turn_gets_the_shells_chat_integrations() {
            let data_dir = std::env::temp_dir().join(format!(
                "dwt-{}-integ-{}",
                std::process::id(),
                COUNTER.fetch_add(1, Ordering::Relaxed)
            ));
            let _ = fs::remove_dir_all(&data_dir);
            fs::create_dir_all(data_dir.join("config")).unwrap();
            // Never started: the turn only lists its tools, which it offers
            // natively only to a provider configured for it.
            fs::write(
                data_dir.join("config").join("user.conf"),
                "model_provider.openai.supports_native_tools=true\n\
                 mcp_server.browser.transport=stdio\n\
                 mcp_server.browser.command=/usr/bin/false\n\
                 mcp_server.browser.enabled=true\n",
            )
            .unwrap();
            let repo = fixture("integ");
            let store = SessionStore::new(&data_dir);
            let session = store
                .create_session(
                    &workspace_engine::hash::repository_id_for_root(&repo),
                    "fixture",
                )
                .unwrap();
            store
                .record_worker_spawned(
                    &session.id,
                    &WorkerRecord {
                        parent_session_id: None,
                        source_repository: repo.clone(),
                        worktree_path: repo.clone(),
                        branch: "damaian/fixture".to_string(),
                        base_commit: "HEAD".to_string(),
                        included_uncommitted: false,
                        git_commands: Vec::new(),
                    },
                )
                .unwrap();
            let script = Scripted::new(MockModelAdapter::new("Nothing to inspect."));
            let recorded = script.clone();
            let host = host_in_process(
                &session.id,
                &data_dir,
                Box::new(move |_config: &Config| {
                    Ok(Box::new(script.clone()) as Box<dyn ModelAdapter>)
                }),
            )
            .unwrap();
            assert_eq!(send(&host, message("Look at the page."))["ok"], true);
            assert_eq!(settle(&host)["state"], "idle");

            let requests = &recorded.0.lock().unwrap().requests;
            let offered: Vec<String> = requests[0]
                .tools
                .iter()
                .flatten()
                .map(|tool| tool.name.clone())
                .collect();
            assert!(
                offered.iter().any(|name| name == "inspect_web_page"),
                "the worker's engine was built without the shell's integrations: {offered:?}"
            );
        }

        /// §5.5: `phase` carries round and maxRounds, `tool` the tool label,
        /// `text` is batched, and `status` marks each transition.
        #[test]
        fn progress_lines_mirror_the_turn() {
            let answer =
                "A long answer, streamed in many small pieces, so batching has something to batch.";
            let (host, session) = hosted(&fixture("prog"), calls_then("run_command", PWD, answer));
            assert_eq!(send(&host, message("Where?"))["ok"], true);
            assert_eq!(settle(&host)["state"], "idle");

            let lines = progress(&session);
            let statuses = payloads(&lines, "status");
            assert_eq!(
                statuses.first().unwrap()["state"],
                "running",
                "{statuses:?}"
            );
            let last = statuses.last().unwrap();
            assert_eq!(last["state"], "idle", "{statuses:?}");
            assert_eq!(last["taskStatus"], "complete", "{statuses:?}");

            let phases = payloads(&lines, "phase");
            assert!(!phases.is_empty());
            for phase in &phases {
                assert!(
                    phase["round"].is_u64()
                        && phase["maxRounds"].is_u64()
                        && phase["explorationRound"].is_u64()
                        && phase["maxExplorationRounds"].is_u64(),
                    "phase must carry both the action and exploration counts: {phase}"
                );
            }
            let tool_phases: Vec<_> = phases
                .iter()
                .filter(|phase| phase["phase"] == "tool")
                .collect();
            let tools = payloads(&lines, "tool");
            assert!(!tools.is_empty(), "{lines:?}");
            assert_eq!(
                tools
                    .iter()
                    .map(|tool| tool["label"].clone())
                    .collect::<Vec<_>>(),
                tool_phases
                    .iter()
                    .map(|phase| phase["label"].clone())
                    .collect::<Vec<_>>()
            );

            let texts = payloads(&lines, "text");
            let streamed: String = texts
                .iter()
                .map(|text| text["text"].as_str().unwrap())
                .collect();
            assert_eq!(streamed, answer);
            // The mock streams 24-byte tokens; one line per token is no batch.
            assert!(
                texts.len() < answer.len().div_ceil(24),
                "text was not batched: {texts:?}"
            );
        }

        /// §5.4: a turn waiting for approval has left its loop, so notes,
        /// pause and stop do not apply, and no second turn starts over it.
        #[test]
        fn a_note_while_waiting_for_approval_is_refused() {
            let (host, _session) =
                hosted(&fixture("note"), calls_then("run_command", TOUCH, "Done."));
            assert_eq!(send(&host, message("Mark it."))["ok"], true);
            assert_eq!(settle(&host)["state"], "waiting_for_approval");

            for command in [
                ControlCommand::Note {
                    text: "also this".to_string(),
                },
                ControlCommand::Pause,
                ControlCommand::Stop,
                message("another"),
            ] {
                let answer = send(&host, command.clone());
                assert_eq!(answer["ok"], false, "{command:?}");
                assert!(
                    answer["error"].as_str().unwrap().contains("approval"),
                    "{command:?}: {answer}"
                );
            }
            assert_eq!(host.snapshot()["state"], "waiting_for_approval");
        }

        #[test]
        fn message_is_refused_while_a_turn_runs() {
            let gated = Gated::new(Scripted::new(MockModelAdapter::new("First.")));
            let (host, _session) = hosted(&fixture("busy"), gated.clone());
            assert_eq!(send(&host, message("one"))["ok"], true);
            gated.wait_entered();

            let refused = send(&host, message("two"));
            assert_eq!(refused["ok"], false);
            assert!(
                refused["error"]
                    .as_str()
                    .unwrap()
                    .contains("already running"),
                "{refused}"
            );
            assert_eq!(host.snapshot()["state"], "running");

            gated.open();
            assert_eq!(settle(&host)["state"], "idle");
            assert_eq!(
                send(&host, message("two"))["ok"],
                true,
                "idle takes the next message"
            );
            assert_eq!(settle(&host)["state"], "idle");
        }

        /// §5.6: the turn stops on the snapshot as in the shell, and `approve`
        /// resumes it in this process, through the same resume, once.
        #[test]
        fn a_command_approval_continues_in_process() {
            let repo = fixture("appr");
            let (host, session) = hosted(&repo, calls_then("run_command", TOUCH, "Done."));
            assert_eq!(send(&host, message("Mark it."))["ok"], true);
            let waiting = settle(&host);
            assert_eq!(waiting["state"], "waiting_for_approval", "{waiting}");
            let proposal_id = waiting["proposalId"].as_str().unwrap().to_string();
            let snapshot = isolated_data_dir()
                .join("chat")
                .join("pending")
                .join(format!("{proposal_id}.json"));
            assert!(
                snapshot.is_file(),
                "the pending snapshot is written as in the shell"
            );

            let approvals = payloads(&progress(&session), "approval");
            assert_eq!(approvals.len(), 1, "{approvals:?}");
            assert_eq!(approvals[0]["kind"], "command");
            assert_eq!(approvals[0]["proposal"]["proposalId"], proposal_id.as_str());
            assert_eq!(approvals[0]["proposal"]["command"], "touch approved.txt");

            let wrong = send(
                &host,
                ControlCommand::Approve {
                    proposal_id: "chatcmd_not_this_one".to_string(),
                },
            );
            assert_eq!(wrong["ok"], false, "{wrong}");
            assert!(!repo.join("approved.txt").exists());

            assert_eq!(
                send(
                    &host,
                    ControlCommand::Approve {
                        proposal_id: proposal_id.clone()
                    }
                )["ok"],
                true
            );
            let done = settle(&host);
            assert_eq!(done["state"], "idle", "{done}");
            assert_eq!(
                done["pid"],
                std::process::id(),
                "the same process resumed it"
            );
            assert!(!snapshot.exists(), "the resume took the snapshot");
            assert!(
                repo.join("approved.txt").is_file(),
                "the approved command ran"
            );
            let tasks = store().read_tasks(&session).unwrap();
            assert_eq!(tasks.len(), 1, "one turn, resumed, not a second one");
            assert_eq!(tasks[0].status, TaskStatus::Complete);
            assert_eq!(
                send(&host, ControlCommand::Approve { proposal_id })["ok"],
                false,
                "a decision resumes once"
            );
        }

        /// Spec 61 §5.5: a turn paused at its round budget is answered in the
        /// worker's own process through `resume_round_budget`, never by the
        /// shell reading the pending file. `approve` means *Continue*, so the
        /// turn re-enters with more rounds and completes as the same task.
        #[test]
        fn a_worker_answers_a_round_budget_pause_in_process() {
            let repo = fixture("rb");
            // Eight read-only rounds hit the default eight-round budget; the
            // ninth response answers after *Continue* re-enters the turn.
            let responses: Vec<String> = (0..8)
                .map(|_| read_only_round())
                .chain(std::iter::once("Done looking.".to_string()))
                .collect();
            let script = Scripted::new(MockModelAdapter::new_sequence(responses));
            let (host, session) = hosted(&repo, script);

            assert_eq!(send(&host, message("Look at the files."))["ok"], true);
            let waiting = settle(&host);
            assert_eq!(waiting["state"], "waiting_for_approval", "{waiting}");
            let proposal_id = waiting["proposalId"].as_str().unwrap().to_string();

            let approvals = payloads(&progress(&session), "approval");
            assert_eq!(approvals.len(), 1, "{approvals:?}");
            assert_eq!(approvals[0]["kind"], "round_budget");
            assert_eq!(approvals[0]["proposal"]["proposalId"], proposal_id.as_str());

            assert_eq!(
                send(&host, ControlCommand::Approve { proposal_id })["ok"],
                true
            );
            let done = settle(&host);
            assert_eq!(done["state"], "idle", "{done}");
            assert_eq!(
                done["pid"],
                std::process::id(),
                "the same process resumed it"
            );

            let tasks = store().read_tasks(&session).unwrap();
            assert_eq!(tasks.len(), 1, "one turn, resumed, not a second one");
            assert_eq!(tasks[0].status, TaskStatus::Complete);
        }

        /// §5.6: a patch ends the turn; the worker stays alive and idle, and
        /// the shell's own `/api/apply-patch` applies it with the worktree as
        /// `repo`.
        #[test]
        fn a_patch_proposal_leaves_the_worker_idle_and_alive() {
            let repo = fixture("ptch");
            let (host, session) = hosted(
                &repo,
                calls_then(
                    "propose_patch",
                    r#"{"summary":"Add a file","files":[{"path":"new.txt","content":"hello\n"}]}"#,
                    "Proposed.",
                ),
            );
            assert_eq!(send(&host, message("Add a file."))["ok"], true);
            assert_eq!(settle(&host)["state"], "idle");

            let approvals = payloads(&progress(&session), "approval");
            assert_eq!(approvals.len(), 1, "{approvals:?}");
            assert_eq!(approvals[0]["kind"], "patch");
            let patch_id = approvals[0]["proposal"]["patchId"]
                .as_str()
                .unwrap()
                .to_string();
            assert!(!repo.join("new.txt").exists(), "a proposal applies nothing");

            let (port, token) = serve_for_test();
            let response = post_form_for_test(
                port,
                &token,
                "/api/apply-patch",
                &format!(
                    "repo={}&patch_id={patch_id}",
                    percent_encode_for_test(&repo.to_string_lossy())
                ),
            );
            assert!(response.starts_with("HTTP/1.1 200"), "{response}");
            assert_eq!(fs::read_to_string(repo.join("new.txt")).unwrap(), "hello\n");

            assert_eq!(
                send(&host, message("Thanks."))["ok"],
                true,
                "the worker is still alive"
            );
            assert_eq!(settle(&host)["state"], "idle");
        }

        /// `pause` holds the turn at the next round boundary, a note queued
        /// there reaches the conversation, and `resume` continues the turn.
        #[test]
        fn pause_holds_the_turn_and_a_note_reaches_it() {
            let gated = Gated::new(calls_then("run_command", PWD, "Done."));
            let (host, session) = hosted(&fixture("paus"), gated.clone());
            assert_eq!(send(&host, message("Where?"))["ok"], true);
            gated.wait_entered();
            assert_eq!(send(&host, ControlCommand::Pause)["ok"], true);
            assert_eq!(
                host.snapshot()["state"],
                "running",
                "a pause takes effect at the boundary, not mid-call"
            );
            gated.open();
            wait_for(&host, |state| state == "paused");
            assert_eq!(
                send(
                    &host,
                    ControlCommand::Note {
                        text: "Mind the README.".to_string()
                    }
                )["ok"],
                true
            );
            assert_eq!(
                send(&host, ControlCommand::Resume { note: None })["ok"],
                true
            );
            assert_eq!(settle(&host)["state"], "idle");

            let messages = store().read_messages(&session).unwrap();
            assert!(
                messages
                    .iter()
                    .any(|message| message.role == "user" && message.content == "Mind the README."),
                "the note is a user message in the session"
            );
            let lines = progress(&session);
            let states: Vec<_> = payloads(&lines, "status")
                .iter()
                .map(|status| status["state"].as_str().unwrap().to_string())
                .collect();
            assert_eq!(
                states,
                ["running", "paused", "running", "idle"],
                "{lines:?}"
            );
            assert_eq!(payloads(&lines, "note").len(), 1, "{lines:?}");
        }

        /// A note accepted while the round runs, in a round that then stops
        /// for approval, reaches the resumed turn: the approval continues the
        /// same turn (§5.6), so the note joins it at its next boundary like
        /// any other (§5.4). Task 10 discovery 3.
        #[test]
        fn a_note_queued_before_an_approval_reaches_the_resumed_turn() {
            let script = calls_then("run_command", TOUCH, "Done.");
            let gated = Gated::new(script.clone());
            let (host, session) = hosted(&fixture("nappr"), gated.clone());
            assert_eq!(send(&host, message("Mark it."))["ok"], true);
            gated.wait_entered();
            assert_eq!(
                send(
                    &host,
                    ControlCommand::Note {
                        text: "Mind the README.".to_string()
                    }
                )["ok"],
                true,
                "a note is accepted while the round runs"
            );
            gated.open();
            let waiting = settle(&host);
            assert_eq!(waiting["state"], "waiting_for_approval", "{waiting}");
            let proposal_id = waiting["proposalId"].as_str().unwrap().to_string();
            assert_eq!(
                send(&host, ControlCommand::Approve { proposal_id })["ok"],
                true
            );
            assert_eq!(settle(&host)["state"], "idle");

            let messages = store().read_messages(&session).unwrap();
            assert!(
                messages
                    .iter()
                    .any(|message| message.role == "user" && message.content == "Mind the README."),
                "the note was dropped at the approval: {messages:?}"
            );
            let requests = &script.0.lock().unwrap().requests;
            assert!(
                requests
                    .last()
                    .unwrap()
                    .messages
                    .iter()
                    .any(|message| message.content.contains("Mind the README.")),
                "the resumed turn's next model request carries the note"
            );
            assert_eq!(payloads(&progress(&session), "note").len(), 1);
        }

        #[test]
        fn stop_ends_the_running_turn_cancelled() {
            let gated = Gated::new(calls_then("run_command", PWD, "Done."));
            let (host, session) = hosted(&fixture("stop"), gated.clone());
            assert_eq!(send(&host, message("Where?"))["ok"], true);
            gated.wait_entered();
            assert_eq!(send(&host, ControlCommand::Stop)["ok"], true);
            gated.open();
            assert_eq!(settle(&host)["state"], "idle");
            let statuses = payloads(&progress(&session), "status");
            assert_eq!(
                statuses.last().unwrap()["taskStatus"],
                "cancelled",
                "{statuses:?}"
            );
            // The next turn has its own cancel token.
            assert_eq!(send(&host, message("Again."))["ok"], true);
            let statuses = {
                settle(&host);
                payloads(&progress(&session), "status")
            };
            assert_eq!(
                statuses.last().unwrap()["taskStatus"],
                "complete",
                "{statuses:?}"
            );
        }
    }

    /// Spec 57 Task 7: the shell-side supervisor and its routes.
    ///
    /// The supervisor starts a stub instead of the real worker: a shell script
    /// that records its argv, its environment and the shape of its handover
    /// line, then holds its stdin like a worker and exits when the shell lets
    /// go of it. Where a test needs the socket protocol, it serves the worker's
    /// socket itself (`fake_socket`). Neither is a real worker, and nothing
    /// here kills a process: every stub ends by its lifeline. The tests that
    /// start the real worker binary or signal a process are `#[ignore]`d in
    /// `tests/workers.rs`.
    mod worker_supervisor {
        use super::*;
        use crate::remember_model_api_key;
        use crate::workers::{
            Refusal, SpawnRequest, SpawnedWorker, StopNext, StopOutcome, StopTarget, Supervisor,
            WorkerRequest, WorkerTimeouts, begin_stop, finish_stop, route,
            workers_to_stop_for_deleted_session,
        };
        use serde_json::{Value, json};
        use std::io::{BufRead, BufReader};
        use std::os::unix::fs::PermissionsExt;
        use std::os::unix::net::UnixListener;
        use std::path::Path;
        use std::process::Command;
        use std::sync::atomic::{AtomicU64, Ordering};
        use std::sync::{Arc, Mutex};
        use std::time::Instant;
        use workspace_engine::worker_progress::{ProgressKind, ProgressWriter};
        use workspace_engine::{ProcessRegistry, SessionStore};

        /// Plain on purpose: the secret scanner leaves it alone, so a leak
        /// would show (Task 6 found that an `sk-…` key is redacted on its way
        /// to disk, which would hide one). Checked in `Fixture::new`.
        const KEY: &str = "plain-worker-handover-key";
        static COUNTER: AtomicU64 = AtomicU64::new(1);

        /// Records argv, the environment, and the handover line's length and
        /// shape with the key masked, so the key itself never reaches disk.
        /// Then it holds stdin open as a worker does, and ends at EOF.
        const STUB: &str = r#"#!/bin/sh
d=$(dirname "$0")
printf '%s\n' "$@" > "$d/argv.out"
/usr/bin/env -0 > "$d/env.out"
IFS= read -r line
printf '%s' "${#line}" > "$d/len.out"
printf '%s' "$line" | /usr/bin/sed -E 's/"apiKey":"[^"]*"/"apiKey":"<masked>"/' > "$d/handover.out"
: > "$d/ready.out"
exec /bin/cat > /dev/null
"#;

        /// Short and not canonicalized: a worker's socket path must fit in
        /// 104 bytes, and `/private` would add 8.
        fn scratch() -> PathBuf {
            let dir = std::env::temp_dir().join(format!(
                "dws-{}-{}",
                std::process::id(),
                COUNTER.fetch_add(1, Ordering::Relaxed)
            ));
            let _ = fs::remove_dir_all(&dir);
            fs::create_dir_all(&dir).unwrap();
            dir
        }

        fn git(dir: &Path, args: &[&str]) -> String {
            let output = Command::new("git")
                .args(["-c", "user.name=Fixture", "-c", "user.email=f@example.test"])
                .args(args)
                .current_dir(dir)
                .output()
                .unwrap();
            assert!(output.status.success(), "git {args:?}: {output:?}");
            String::from_utf8_lossy(&output.stdout).to_string()
        }

        /// A committed one-file repository, canonical as the spawn records it.
        fn repository(root: &Path, name: &str) -> PathBuf {
            let repo = root.join(name);
            fs::create_dir_all(&repo).unwrap();
            fs::write(repo.join("README.md"), "# Fixture\n").unwrap();
            git(&repo, &["init", "-q"]);
            git(&repo, &["add", "."]);
            git(&repo, &["commit", "-qm", "init"]);
            fs::canonicalize(&repo).unwrap()
        }

        fn timeouts() -> WorkerTimeouts {
            WorkerTimeouts {
                ready: Duration::from_secs(10),
                exchange: Duration::from_secs(2),
                stop_to_shutdown: Duration::from_millis(300),
                // Long enough that a stub always ends by its lifeline first:
                // nothing in this module may signal a process.
                shutdown_to_sigterm: Duration::from_secs(10),
                poll: Duration::from_millis(20),
            }
        }

        struct Fixture {
            root: PathBuf,
            repo: PathBuf,
            config: Config,
            supervisor: Supervisor,
        }

        impl Fixture {
            fn new() -> Self {
                let root = scratch();
                let repo = repository(&root, "r");
                let stub_dir = root.join("stub");
                fs::create_dir_all(&stub_dir).unwrap();
                let stub = stub_dir.join("worker");
                fs::write(&stub, STUB).unwrap();
                fs::set_permissions(&stub, fs::Permissions::from_mode(0o755)).unwrap();
                // The key comes from `resolve_model_api_key`'s in-memory cache,
                // as in the app: an account of this test's own, so no Keychain
                // is ever asked.
                let account = format!(
                    "damaian-worker-test-{}-{}",
                    std::process::id(),
                    COUNTER.fetch_add(1, Ordering::Relaxed)
                );
                remember_model_api_key(&account, KEY);
                let config = Config {
                    data_dir: root.join("d"),
                    enable_worker_sessions: true,
                    enable_index_watcher: false,
                    model_api_key_env: format!("keychain:{account}"),
                    ..Config::default()
                };
                let scanner = SecretScanner::new(config.secret_patterns.clone());
                assert_eq!(
                    scanner.redact(KEY).text,
                    KEY,
                    "the test key must survive redaction, or a leak would be hidden"
                );
                Self {
                    supervisor: Supervisor::new(stub, timeouts()),
                    root,
                    repo,
                    config,
                }
            }

            fn store(&self) -> SessionStore {
                SessionStore::new(&self.config.data_dir)
            }

            fn spawn_from(&self, repo: &Path, prompt: &str, parent: Option<&str>) -> SpawnedWorker {
                self.supervisor
                    .spawn(
                        &self.config,
                        SpawnRequest {
                            repo: repo.to_path_buf(),
                            prompt: prompt.to_string(),
                            include_uncommitted: false,
                            parent_session_id: parent.map(str::to_string),
                        },
                        KEY,
                    )
                    .expect("the worker should spawn")
            }

            fn spawn(&self, prompt: &str, parent: Option<&str>) -> SpawnedWorker {
                self.spawn_from(&self.repo, prompt, parent)
            }

            fn parent(&self, title: &str) -> String {
                self.store()
                    .create_session(
                        &workspace_engine::hash::repository_id_for_root(&self.repo),
                        title,
                    )
                    .unwrap()
                    .id
            }

            /// One worker route, with `config` (on or off) and form fields
            /// encoded the way the browser sends them.
            fn route_with(
                &self,
                config: &Config,
                method: &str,
                path: &str,
                query: &[(&str, &str)],
                form: &[(&str, &str)],
            ) -> Result<Value, Refusal> {
                let query: HashMap<String, String> = query
                    .iter()
                    .map(|(key, value)| (key.to_string(), value.to_string()))
                    .collect();
                let form = parse_form(&form_body_for_test(form));
                route(
                    &self.supervisor,
                    config,
                    &|config: &Config| resolve_shell_model_api_key(config),
                    &WorkerRequest {
                        method,
                        path,
                        query: &query,
                        form: &form,
                    },
                )
                .map(|body| serde_json::from_str(&body).expect("a JSON answer"))
            }

            fn route(
                &self,
                method: &str,
                path: &str,
                query: &[(&str, &str)],
                form: &[(&str, &str)],
            ) -> Result<Value, Refusal> {
                self.route_with(&self.config, method, path, query, form)
            }

            fn wait_for_exit(&self, session: &str) {
                wait_until("the worker to exit", || !self.supervisor.is_alive(session));
            }
        }

        fn wait_until(what: &str, done: impl Fn() -> bool) {
            let deadline = Instant::now() + Duration::from_secs(20);
            while !done() {
                assert!(Instant::now() < deadline, "timed out waiting for {what}");
                std::thread::sleep(Duration::from_millis(20));
            }
        }

        /// The stub's record of its start, once it has written all of it.
        fn stub_output(fixture: &Fixture, name: &str) -> String {
            let dir = fixture.root.join("stub");
            wait_until("the stub to record its start", || {
                dir.join("ready.out").exists()
            });
            fs::read_to_string(dir.join(name)).unwrap()
        }

        /// Serves the worker's socket in the test, recording every request and
        /// answering with `answer`. Stands in for the worker's protocol.
        fn fake_socket(
            data_dir: &Path,
            session: &str,
            answer: fn(&Value) -> Value,
        ) -> Arc<Mutex<Vec<Value>>> {
            let path = crate::worker_socket_path(data_dir, session).unwrap();
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            let listener = UnixListener::bind(&path).unwrap();
            let requests = Arc::new(Mutex::new(Vec::new()));
            let seen = Arc::clone(&requests);
            std::thread::spawn(move || {
                for stream in listener.incoming() {
                    let Ok(stream) = stream else { return };
                    let Ok(mut writer) = stream.try_clone() else {
                        return;
                    };
                    for line in BufReader::new(stream).lines() {
                        let Ok(line) = line else { break };
                        let request: Value = serde_json::from_str(&line).unwrap();
                        let reply = answer(&request);
                        seen.lock().unwrap().push(request);
                        if writeln!(writer, "{reply}").is_err() {
                            break;
                        }
                    }
                }
            });
            requests
        }

        fn echo(request: &Value) -> Value {
            match request["command"].as_str() {
                Some("status") => json!({"ok": true, "state": "idle"}),
                command => json!({"ok": true, "echo": command}),
            }
        }

        fn audit_events(data_dir: &Path, event_type: &str) -> Vec<Value> {
            fs::read_to_string(data_dir.join("audit").join("events.jsonl"))
                .unwrap_or_default()
                .lines()
                .filter_map(|line| serde_json::from_str::<Value>(line).ok())
                .filter(|event| event["eventType"] == event_type)
                .collect()
        }

        fn files_under(dir: &Path) -> Vec<PathBuf> {
            let mut files = Vec::new();
            let Ok(entries) = fs::read_dir(dir) else {
                return files;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                let Ok(meta) = fs::symlink_metadata(&path) else {
                    continue;
                };
                if meta.is_dir() {
                    files.extend(files_under(&path));
                } else if meta.is_file() {
                    files.push(path);
                }
            }
            files
        }

        /// §5.1 steps 7–8: one `worker_spawned` on the worker's log, one on the
        /// parent's, one in the audit log, and the worker's and the audit's
        /// carry the exact git commands that created the worktree.
        #[test]
        fn spawn_records_worker_spawned_on_worker_parent_and_audit() {
            let fixture = Fixture::new();
            let parent = fixture.parent("Planner");
            let spawned =
                fixture.spawn("\n  Fix the parser\n\nRead src/a.rs first.", Some(&parent));

            assert_eq!(spawned.title, "Fix the parser");
            assert_eq!(spawned.branch, "damaian/fix-the-parser");
            assert_eq!(
                spawned.worktree,
                fixture.repo.join(".damaian/worktrees/fix-the-parser")
            );
            let store = fixture.store();
            let record = store.worker_record(&spawned.session_id).unwrap().unwrap();
            assert_eq!(record.parent_session_id.as_deref(), Some(parent.as_str()));
            assert_eq!(record.source_repository, fixture.repo);
            assert_eq!(record.worktree_path, spawned.worktree);
            assert_eq!(record.branch, spawned.branch);
            assert!(!record.included_uncommitted);
            assert!(
                record
                    .git_commands
                    .iter()
                    .any(|command| command.contains("worktree add -b damaian/fix-the-parser")),
                "{:?}",
                record.git_commands
            );
            assert_eq!(
                store.worker_children(&parent).unwrap(),
                vec![spawned.session_id.clone()]
            );
            let session = store.read_session(&spawned.session_id).unwrap().unwrap();
            assert_eq!(session.title, "Fix the parser");
            assert_eq!(
                session.repository_id,
                workspace_engine::hash::repository_id_for_root(&spawned.worktree),
                "the worker session belongs to its worktree"
            );
            assert_eq!(store.session_mode(&spawned.session_id), SessionMode::Code);

            let audited = audit_events(&fixture.config.data_dir, "worker_spawned");
            assert_eq!(audited.len(), 1, "{audited:?}");
            let audited = &audited[0];
            assert_eq!(audited["sessionId"], spawned.session_id);
            assert_eq!(audited["parentSessionId"], parent);
            assert_eq!(audited["branch"], spawned.branch);
            assert_eq!(audited["baseCommit"], record.base_commit);
            assert_eq!(audited["includedUncommitted"], "false");
            let commands: Vec<String> =
                serde_json::from_str(audited["gitCommands"].as_str().unwrap()).unwrap();
            assert_eq!(
                commands, record.git_commands,
                "the exact commands, in order"
            );

            fixture.supervisor.shutdown_all();
        }

        /// A data directory whose worker socket path cannot fit `sun_path` is
        /// refused before anything is created: no worktree, no branch, no
        /// session. Before this check it was refused only when the process
        /// started, after all three existed (Task 9 discovery 3).
        #[test]
        fn a_data_dir_too_long_for_the_socket_is_refused_before_the_worktree() {
            let mut fixture = Fixture::new();
            fixture.config.data_dir = fixture.root.join("d".repeat(120));
            let refused = fixture.supervisor.spawn(
                &fixture.config,
                SpawnRequest {
                    repo: fixture.repo.clone(),
                    prompt: "Fix the parser".to_string(),
                    include_uncommitted: false,
                    parent_session_id: None,
                },
                KEY,
            );
            let error = refused.expect_err("the spawn must be refused");
            assert!(error.contains("socket path"), "{error}");
            assert!(
                !fixture.repo.join(".damaian").exists(),
                "a worktree was created before the refusal"
            );
            let branches = Command::new("git")
                .args(["branch", "--list", "damaian/*"])
                .current_dir(&fixture.repo)
                .output()
                .unwrap();
            assert!(
                branches.stdout.is_empty(),
                "a branch was created before the refusal: {}",
                String::from_utf8_lossy(&branches.stdout)
            );
            assert!(
                !fixture.config.data_dir.join("sessions").exists(),
                "a session was created before the refusal"
            );
        }

        /// Requirement 9: the key reaches the worker on stdin only, from
        /// `resolve_model_api_key`'s cache. Its argv carries neither key nor
        /// token, its environment is the shell's own, and the key is in no
        /// file the spawn wrote and not in the route's answer.
        #[test]
        fn spawn_hands_over_the_cached_key_on_stdin_only() {
            let fixture = Fixture::new();
            let answer = fixture
                .route(
                    "POST",
                    "/api/workers",
                    &[],
                    &[
                        ("repo", fixture.repo.to_str().unwrap()),
                        ("prompt", "Write the docs"),
                    ],
                )
                .expect("the spawn route answers");
            let session = answer["sessionId"].as_str().unwrap().to_string();

            let argv: Vec<String> = stub_output(&fixture, "argv.out")
                .lines()
                .map(str::to_string)
                .collect();
            assert_eq!(
                argv,
                vec![
                    "--worker".to_string(),
                    "--session".to_string(),
                    session.clone(),
                    "--data-dir".to_string(),
                    fixture.config.data_dir.to_string_lossy().to_string(),
                ]
            );

            let handover: Value =
                serde_json::from_str(&stub_output(&fixture, "handover.out")).unwrap();
            let token = handover["token"].as_str().unwrap().to_string();
            assert_eq!(handover["apiKey"], "<masked>");
            assert_eq!(handover.as_object().unwrap().len(), 2, "{handover}");
            assert!(
                token.len() == 64 && token.bytes().all(|byte| byte.is_ascii_hexdigit()),
                "a 256-bit token in hex"
            );
            let expected = json!({"apiKey": KEY, "token": token}).to_string();
            assert_eq!(
                stub_output(&fixture, "len.out"),
                expected.len().to_string(),
                "the line on stdin is the handover with this key"
            );
            assert!(!argv.join(" ").contains(KEY) && !argv.join(" ").contains(&token));

            let child_env: HashMap<String, String> = stub_output(&fixture, "env.out")
                .split('\0')
                .filter_map(|pair| pair.split_once('='))
                .map(|(name, value)| (name.to_string(), value.to_string()))
                .collect();
            // What the stub's `/bin/sh` changes on its own: it manages these
            // four, and macOS's System Integrity Protection strips `DYLD_*`
            // when it runs a protected binary. The real worker is neither.
            let set_by_sh = |name: &str| {
                matches!(name, "PWD" | "OLDPWD" | "SHLVL" | "_") || name.starts_with("DYLD_")
            };
            for (name, value) in std::env::vars().filter(|(name, _)| !set_by_sh(name)) {
                assert_eq!(
                    child_env.get(&name),
                    Some(&value),
                    "the worker gets the shell's environment unchanged: {name}"
                );
            }
            let added: Vec<&String> = child_env
                .keys()
                .filter(|name| std::env::var(name.as_str()).is_err() && !set_by_sh(name))
                .collect();
            assert!(added.is_empty(), "added to the environment: {added:?}");
            assert!(child_env.values().all(|value| !value.contains(KEY)));

            assert!(!answer.to_string().contains(KEY));
            // Everything the spawn wrote: the data directory (session logs,
            // audit, registry) and the repository with its new worktree. The
            // stub's own files are left out; they hold the token by design.
            let written = files_under(&fixture.config.data_dir)
                .into_iter()
                .chain(files_under(&fixture.repo));
            for file in written {
                let bytes = fs::read(&file).unwrap();
                let text = String::from_utf8_lossy(&bytes);
                assert!(!text.contains(KEY), "the key reached {}", file.display());
                assert!(
                    !text.contains(&token),
                    "the token reached {}",
                    file.display()
                );
            }

            fixture.supervisor.shutdown_all();
        }

        /// C6: the shell registers the worker as `worker`, under the worker's
        /// session id, with the shell as owner, so the launch sweep and the
        /// shell's own `sweep_own` both reach it.
        #[test]
        fn the_shell_registers_the_worker_as_process_kind_worker() {
            let fixture = Fixture::new();
            let spawned = fixture.spawn("Register me", None);
            let pid = fixture.supervisor.pid(&spawned.session_id).unwrap();

            let registry = ProcessRegistry::open(&fixture.config.data_dir).unwrap();
            let entries: Vec<_> = registry
                .entries()
                .unwrap()
                .into_iter()
                .filter_map(|(_, entry)| entry)
                .collect();
            assert_eq!(entries.len(), 1, "{entries:?}");
            let entry = &entries[0];
            assert_eq!(entry.kind, "worker");
            assert_eq!(entry.session_id, spawned.session_id);
            assert_eq!(entry.pid, pid);
            assert_eq!(entry.pgid, pid, "in its own process group");
            assert_eq!(entry.owner_pid, std::process::id(), "owned by the shell");

            fixture.supervisor.shutdown_all();
            assert!(
                registry.entries().unwrap().is_empty(),
                "the entry goes with the worker"
            );
        }

        /// §5.3 step 5: deleting a parent sends `shutdown` to its workers, and
        /// the workers' worktrees and session logs stay.
        #[test]
        fn deleting_a_parent_sends_shutdown_to_its_workers_and_keeps_their_worktrees() {
            let fixture = Fixture::new();
            let parent = fixture.parent("Planner");
            let other = fixture.parent("Someone else");
            let spawned = fixture.spawn("Child task", Some(&parent));
            let unrelated = fixture.spawn("Unrelated task", Some(&other));
            let requests = fake_socket(&fixture.config.data_dir, &spawned.session_id, echo);
            let store = fixture.store();

            let stopping = workers_to_stop_for_deleted_session(&store, &parent);
            assert_eq!(stopping, vec![spawned.session_id.clone()]);
            for worker in &stopping {
                assert!(fixture.supervisor.shutdown(worker));
            }
            store.delete_session(&parent).unwrap();

            fixture.wait_for_exit(&spawned.session_id);
            assert!(
                requests
                    .lock()
                    .unwrap()
                    .iter()
                    .any(|request| request["command"] == "shutdown"),
                "{:?}",
                requests.lock().unwrap()
            );
            assert!(fixture.supervisor.is_alive(&unrelated.session_id));
            assert!(spawned.worktree.is_dir(), "the worktree is kept");
            assert!(
                store.read_session(&spawned.session_id).unwrap().is_some(),
                "the worker's session is kept"
            );
            // Deleting a worker's own session stops that worker too.
            assert_eq!(
                workers_to_stop_for_deleted_session(&store, &unrelated.session_id),
                vec![unrelated.session_id.clone()]
            );

            fixture.supervisor.shutdown_all();
        }

        /// §5.1 "One worker per worktree": *Restart worker* is refused while a
        /// live worker holds the session's worktree, and works once it is gone.
        #[test]
        fn restart_is_refused_while_a_live_worker_holds_the_worktree() {
            let fixture = Fixture::new();
            let spawned = fixture.spawn("Restartable", None);
            let first = fixture.supervisor.pid(&spawned.session_id).unwrap();
            let restart = |fixture: &Fixture| {
                fixture.route(
                    "POST",
                    "/api/worker-restart",
                    &[],
                    &[("session_id", spawned.session_id.as_str())],
                )
            };

            let refused = restart(&fixture).unwrap_err();
            assert_eq!(refused.status, 409, "{refused:?}");
            assert_eq!(fixture.supervisor.pid(&spawned.session_id), Some(first));

            assert!(fixture.supervisor.shutdown(&spawned.session_id));
            fixture.wait_for_exit(&spawned.session_id);
            assert_eq!(
                restart(&fixture).unwrap(),
                json!({"status": "started"}),
                "restarted once the worktree is free"
            );
            let second = fixture.supervisor.pid(&spawned.session_id).unwrap();
            assert_ne!(first, second, "a new process for the same session");

            // An ordinary session has no worktree to restart in.
            let ordinary = fixture.parent("Not a worker");
            assert_eq!(
                fixture
                    .route(
                        "POST",
                        "/api/worker-restart",
                        &[],
                        &[("session_id", ordinary.as_str())],
                    )
                    .unwrap_err()
                    .status,
                400
            );

            fixture.supervisor.shutdown_all();
        }

        /// #36 via §5.1: a dirty worktree is removed only after the second
        /// confirmation, the branch is never deleted, and a live worker's
        /// worktree is not removed at all.
        #[test]
        fn remove_worktree_requires_the_second_confirmation_when_dirty() {
            let fixture = Fixture::new();
            let spawned = fixture.spawn("Leave a mess", None);
            let remove = |confirmed: Option<&str>| {
                let mut form = vec![("session_id", spawned.session_id.as_str())];
                if let Some(confirmed) = confirmed {
                    form.push(("confirmed_dirty", confirmed));
                }
                fixture.route("POST", "/api/worker-remove-worktree", &[], &form)
            };

            assert_eq!(
                remove(Some("true")).unwrap_err().status,
                409,
                "not while the worker runs"
            );
            assert!(fixture.supervisor.shutdown(&spawned.session_id));
            fixture.wait_for_exit(&spawned.session_id);
            fs::write(spawned.worktree.join("scratch.txt"), "unsaved\n").unwrap();

            assert_eq!(
                remove(None).unwrap(),
                json!({"status": "dirty", "paths": ["scratch.txt"]})
            );
            assert!(spawned.worktree.join("scratch.txt").exists());
            assert_eq!(
                remove(Some("false")).unwrap()["status"],
                "dirty",
                "only `true` confirms"
            );

            assert_eq!(remove(Some("true")).unwrap(), json!({"status": "removed"}));
            assert!(!spawned.worktree.exists());
            assert!(
                git(&fixture.repo, &["branch", "--list", &spawned.branch])
                    .contains(&spawned.branch),
                "the branch is never deleted"
            );
            assert_eq!(
                audit_events(&fixture.config.data_dir, "worker_worktree_removed").len(),
                1
            );
        }

        /// C7: `/api/worker-control` takes form fields and relays exactly the
        /// socket's names to the worker; the first message a spawn sends is the
        /// prompt.
        #[test]
        fn worker_control_takes_form_fields() {
            let fixture = Fixture::new();
            let spawned = fixture.spawn("First prompt\nwith two lines", None);
            let requests = fake_socket(&fixture.config.data_dir, &spawned.session_id, echo);
            let control = |form: &[(&str, &str)]| {
                let mut fields = vec![("session_id", spawned.session_id.as_str())];
                fields.extend_from_slice(form);
                fixture.route("POST", "/api/worker-control", &[], &fields)
            };
            wait_until("the first message", || !requests.lock().unwrap().is_empty());

            assert_eq!(
                control(&[("command", "note"), ("text", "check the tests & docs")]).unwrap(),
                json!({"ok": true, "echo": "note"}),
                "the socket's own answer"
            );
            control(&[("command", "approve"), ("proposal_id", "prop_1")]).unwrap();
            control(&[("command", "resume"), ("note", "carry on")]).unwrap();
            control(&[("command", "resume")]).unwrap();
            control(&[("command", "pause")]).unwrap();

            let seen = requests.lock().unwrap().clone();
            let token = seen[0]["token"].as_str().unwrap().to_string();
            assert_eq!(token.len(), 64);
            let without_token: Vec<Value> = seen
                .iter()
                .map(|request| {
                    assert_eq!(request["token"], token.as_str(), "one token throughout");
                    let mut request = request.clone();
                    request.as_object_mut().unwrap().remove("token");
                    request
                })
                .collect();
            assert_eq!(
                without_token,
                vec![
                    json!({"command": "message", "text": "First prompt\nwith two lines"}),
                    json!({"command": "note", "text": "check the tests & docs"}),
                    json!({"command": "approve", "proposalId": "prop_1"}),
                    json!({"command": "resume", "note": "carry on"}),
                    json!({"command": "resume"}),
                    json!({"command": "pause"}),
                ]
            );

            let unknown = control(&[("command", "self_destruct")]).unwrap_err();
            assert_eq!(unknown.status, 400, "{unknown:?}");
            // A JSON body, as §5.7 first described it, is not a form.
            let json_body = route(
                &fixture.supervisor,
                &fixture.config,
                &|_: &Config| Ok(KEY.to_string()),
                &WorkerRequest {
                    method: "POST",
                    path: "/api/worker-control",
                    query: &HashMap::new(),
                    form: &parse_form(&format!(
                        r#"{{"sessionId":"{}","command":"status"}}"#,
                        spawned.session_id
                    )),
                },
            )
            .unwrap_err();
            assert_eq!(json_body.status, 400, "{json_body:?}");
            assert_eq!(
                requests.lock().unwrap().len(),
                seen.len(),
                "neither reached it"
            );

            fixture.supervisor.shutdown_all();
        }

        /// §5.7: a project's board lists the workers whose source is that
        /// repository, in Needs you / Running / Finished, newest first, and
        /// only finished rows carry files changed and commits ahead.
        #[test]
        fn workers_list_groups_by_source_repository() {
            let fixture = Fixture::new();
            let other_repo = repository(&fixture.root, "o");
            let parent = fixture.parent("Planner");
            let waiting = fixture.spawn("Waiting one", Some(&parent));
            let running = fixture.spawn("Running one", None);
            let done = fixture.spawn("Done one", None);
            let gone = fixture.spawn("Gone one", None);
            let elsewhere = fixture.spawn_from(&other_repo, "Elsewhere", None);
            let progress = |session: &str, lines: &[(ProgressKind, Value)]| {
                let mut writer = ProgressWriter::create(
                    &fixture.config.data_dir,
                    session,
                    SecretScanner::new(Vec::new()),
                )
                .unwrap();
                for (kind, payload) in lines {
                    writer.append(*kind, payload.clone()).unwrap();
                }
            };
            progress(
                &waiting.session_id,
                &[(
                    ProgressKind::Status,
                    json!({"state": "waiting_for_approval"}),
                )],
            );
            progress(
                &running.session_id,
                &[
                    (ProgressKind::Status, json!({"state": "running"})),
                    (
                        ProgressKind::Phase,
                        json!({
                            "phase": "tool",
                            "label": "Running cargo test",
                            "round": 2,
                            "maxRounds": 8,
                            "explorationRound": 3,
                            "maxExplorationRounds": 24,
                        }),
                    ),
                ],
            );
            progress(
                &done.session_id,
                &[(
                    ProgressKind::Status,
                    json!({"state": "idle", "taskStatus": "complete"}),
                )],
            );
            progress(
                &gone.session_id,
                &[(ProgressKind::Status, json!({"state": "running"}))],
            );
            assert!(fixture.supervisor.shutdown(&gone.session_id));
            fixture.wait_for_exit(&gone.session_id);

            let list = || {
                fixture
                    .route(
                        "GET",
                        "/api/workers",
                        &[("repo", fixture.repo.to_str().unwrap())],
                        &[],
                    )
                    .unwrap()["workers"]
                    .as_array()
                    .unwrap()
                    .clone()
            };
            let rows = list();
            let summary: Vec<(String, String, String)> = rows
                .iter()
                .map(|row| {
                    (
                        row["title"].as_str().unwrap().to_string(),
                        row["group"].as_str().unwrap().to_string(),
                        row["state"].as_str().unwrap().to_string(),
                    )
                })
                .collect();
            let row = |title: &str, group: &str, state: &str| {
                (title.to_string(), group.to_string(), state.to_string())
            };
            assert_eq!(
                summary,
                vec![
                    row("Waiting one", "needs_you", "waiting_for_approval"),
                    row("Running one", "running", "running"),
                    row("Gone one", "finished", "exited"),
                    row("Done one", "finished", "idle"),
                ],
                "grouped, newest first in each group, and only this repository's"
            );
            assert!(
                rows.iter()
                    .all(|row| row["sessionId"] != elsewhere.session_id)
            );

            let by_title = |rows: &[Value], title: &str| {
                rows.iter()
                    .find(|row| row["title"] == title)
                    .unwrap()
                    .clone()
            };
            let waiting_row = by_title(&rows, "Waiting one");
            assert_eq!(waiting_row["parentSessionId"], parent);
            assert_eq!(waiting_row["parentTitle"], "Planner");
            assert_eq!(waiting_row["alive"], true);
            assert_eq!(waiting_row["filesChanged"], Value::Null);
            let running_row = by_title(&rows, "Running one");
            assert_eq!(running_row["parentTitle"], Value::Null);
            assert_eq!(running_row["round"], 2);
            assert_eq!(running_row["maxRounds"], 8);
            assert_eq!(
                running_row["explorationRound"], 3,
                "the board row mirrors the phase's read count additively"
            );
            assert_eq!(running_row["maxExplorationRounds"], 24);
            assert_eq!(running_row["doing"], "Running cargo test");
            assert_eq!(running_row["branch"], running.branch);
            let gone_row = by_title(&rows, "Gone one");
            assert_eq!(gone_row["alive"], false);
            assert_eq!(gone_row["filesChanged"], 0);
            assert_eq!(gone_row["commitsAhead"], 0);
            assert_eq!(by_title(&rows, "Done one")["taskStatus"], "complete");

            fs::write(gone.worktree.join("README.md"), "# Changed\n").unwrap();
            git(&gone.worktree, &["commit", "-qam", "one ahead"]);
            fs::write(gone.worktree.join("new.txt"), "uncommitted\n").unwrap();
            git(&gone.worktree, &["add", "new.txt"]);
            let gone_row = by_title(&list(), "Gone one");
            assert_eq!(gone_row["commitsAhead"], 1);
            assert_eq!(
                gone_row["filesChanged"], 2,
                "committed and staged, against the base"
            );

            fixture.supervisor.shutdown_all();
        }

        /// `after=` pages exactly, `alive` says whether this shell still holds
        /// the worker, and a session id that is not one is refused before any
        /// path is built from it.
        #[test]
        fn worker_progress_pages_with_after() {
            let fixture = Fixture::new();
            let spawned = fixture.spawn("Make progress", None);
            let mut writer = ProgressWriter::create(
                &fixture.config.data_dir,
                &spawned.session_id,
                SecretScanner::new(Vec::new()),
            )
            .unwrap();
            for text in ["one", "two", "three"] {
                writer
                    .append(ProgressKind::Text, json!({"text": text}))
                    .unwrap();
            }
            let page = |after: &str| {
                fixture.route(
                    "GET",
                    "/api/worker-progress",
                    &[("session", spawned.session_id.as_str()), ("after", after)],
                    &[],
                )
            };

            let answer = page("1").unwrap();
            let seqs: Vec<u64> = answer["events"]
                .as_array()
                .unwrap()
                .iter()
                .map(|event| event["seq"].as_u64().unwrap())
                .collect();
            assert_eq!(seqs, vec![2, 3]);
            assert_eq!(answer["events"][1]["payload"]["text"], "three");
            assert_eq!(answer["alive"], true);
            assert_eq!(page("3").unwrap()["events"], json!([]));
            assert_eq!(page("0").unwrap()["events"].as_array().unwrap().len(), 3);

            assert!(fixture.supervisor.shutdown(&spawned.session_id));
            fixture.wait_for_exit(&spawned.session_id);
            assert_eq!(page("0").unwrap()["alive"], false);

            let traversal = fixture
                .route(
                    "GET",
                    "/api/worker-progress",
                    &[("session", "../../etc/x"), ("after", "0")],
                    &[],
                )
                .unwrap_err();
            assert_eq!(traversal.status, 400);
            assert_eq!(page("not-a-number").unwrap_err().status, 400);
        }

        /// Requirement 1 and §5.0: with the setting off, every worker route
        /// refuses, except the list (a running worker stays reachable) and
        /// the setting's own read.
        #[test]
        fn every_worker_route_refuses_when_the_setting_is_off() {
            let fixture = Fixture::new();
            let spawned = fixture.spawn("Started while on", None);
            let off = Config {
                enable_worker_sessions: false,
                ..fixture.config.clone()
            };
            let repo = fixture.repo.to_str().unwrap();
            let session = spawned.session_id.as_str();
            type Case<'a> = (
                &'a str,
                &'a str,
                Vec<(&'a str, &'a str)>,
                Vec<(&'a str, &'a str)>,
            );
            let refused: Vec<Case> = vec![
                (
                    "GET",
                    "/api/worker-source-status",
                    vec![("repo", repo), ("prompt", "x")],
                    vec![],
                ),
                (
                    "POST",
                    "/api/workers",
                    vec![],
                    vec![("repo", repo), ("prompt", "x")],
                ),
                (
                    "GET",
                    "/api/worker-progress",
                    vec![("session", session), ("after", "0")],
                    vec![],
                ),
                (
                    "POST",
                    "/api/worker-control",
                    vec![],
                    vec![("session_id", session), ("command", "status")],
                ),
                (
                    "POST",
                    "/api/worker-restart",
                    vec![],
                    vec![("session_id", session)],
                ),
                (
                    "POST",
                    "/api/worker-remove-worktree",
                    vec![],
                    vec![("session_id", session)],
                ),
            ];
            for (method, path, query, form) in refused {
                let refusal = fixture
                    .route_with(&off, method, path, &query, &form)
                    .expect_err(path);
                assert_eq!(refusal.status, 403, "{path}: {refusal:?}");
            }
            assert_eq!(
                fixture
                    .store()
                    .worker_sessions_for_source(&fixture.repo)
                    .unwrap()
                    .len(),
                1,
                "the refused spawn created nothing"
            );

            let listed = fixture
                .route_with(&off, "GET", "/api/workers", &[("repo", repo)], &[])
                .unwrap();
            assert_eq!(listed["workers"][0]["sessionId"], session);
            assert_eq!(listed["workers"][0]["alive"], true);
            assert_eq!(
                fixture
                    .route_with(&off, "GET", "/api/workers-enabled", &[], &[])
                    .unwrap(),
                json!({"enabled": false})
            );
            assert_eq!(
                fixture
                    .route("GET", "/api/workers-enabled", &[], &[])
                    .unwrap(),
                json!({"enabled": true})
            );

            fixture.supervisor.shutdown_all();
        }

        /// The routes are served by `handle_connection`, through the global
        /// supervisor and the shell's own config, which is off by default.
        #[test]
        fn the_shell_serves_the_worker_routes_and_they_refuse_while_off() {
            isolated_data_dir();
            let repo = scratch();
            let (port, token) = serve_for_test();

            let control = post_fields_for_test(
                port,
                &token,
                "/api/worker-control",
                &[("session_id", "session_x"), ("command", "status")],
            );
            assert!(control.starts_with("HTTP/1.1 403"), "{control}");
            let enabled = get_for_test(port, &token, "/api/workers-enabled");
            assert!(enabled.starts_with("HTTP/1.1 200"), "{enabled}");
            assert_eq!(json_of(&enabled), json!({"enabled": false}));
            let listed = get_for_test(
                port,
                &token,
                &format!("/api/workers?repo={}", repo.display()),
            );
            assert_eq!(json_of(&listed), json!({"workers": []}), "{listed}");
        }

        /// Every stub here ends by its lifeline: `shutdown_all` lets go of
        /// each worker's stdin and waits for it, and no signal is needed.
        #[test]
        fn shutdown_all_ends_every_worker_and_waits_for_it() {
            let fixture = Fixture::new();
            let first = fixture.spawn("First", None);
            let second = fixture.spawn("Second", None);
            let pids = [
                fixture.supervisor.pid(&first.session_id).unwrap(),
                fixture.supervisor.pid(&second.session_id).unwrap(),
            ];

            fixture.supervisor.shutdown_all();

            for pid in pids {
                assert!(
                    workspace_engine::ProcessIdentity::of(pid).is_none(),
                    "{pid} is still running after shutdown_all returned"
                );
            }
            assert!(!fixture.supervisor.is_alive(&first.session_id));
            assert!(!fixture.supervisor.is_alive(&second.session_id));
            assert!(
                ProcessRegistry::open(&fixture.config.data_dir)
                    .unwrap()
                    .entries()
                    .unwrap()
                    .is_empty(),
                "every registration went with its worker"
            );
            assert!(
                audit_events(&fixture.config.data_dir, "worker_terminated").is_empty(),
                "the lifeline ended them; no signal was needed"
            );
        }

        /// A scripted worker for the stop escalation: what it answers, whether
        /// a stop ends its turn, and whether `shutdown` makes it exit.
        struct FakeTarget {
            state: &'static str,
            answers: bool,
            stop_ends_turn: bool,
            exits_on_shutdown: bool,
            exited: bool,
            terminated: bool,
            sent: Vec<String>,
        }

        impl FakeTarget {
            fn new(state: &'static str) -> Self {
                Self {
                    state,
                    answers: true,
                    stop_ends_turn: false,
                    exits_on_shutdown: false,
                    exited: false,
                    terminated: false,
                    sent: Vec::new(),
                }
            }
        }

        impl StopTarget for FakeTarget {
            fn send(&mut self, command: &str) -> Option<Value> {
                self.sent.push(command.to_string());
                if !self.answers || self.exited {
                    return None;
                }
                Some(match command {
                    "status" => json!({"ok": true, "state": self.state}),
                    "stop" => match self.state {
                        "running" | "paused" => {
                            if self.stop_ends_turn {
                                self.state = "idle";
                            }
                            json!({"ok": true})
                        }
                        "waiting_for_approval" => {
                            json!({"ok": false, "error": "approve or reject it"})
                        }
                        _ => json!({"ok": false, "error": "no turn is running"}),
                    },
                    "shutdown" => {
                        if self.exits_on_shutdown {
                            self.exited = true;
                        }
                        json!({"ok": true})
                    }
                    _ => json!({"ok": false, "error": "unexpected"}),
                })
            }

            fn has_exited(&mut self) -> bool {
                self.exited
            }

            fn terminate(&mut self) {
                self.terminated = true;
                self.exited = true;
            }
        }

        fn short() -> WorkerTimeouts {
            WorkerTimeouts {
                stop_to_shutdown: Duration::from_millis(150),
                shutdown_to_sigterm: Duration::from_millis(150),
                poll: Duration::from_millis(10),
                ..WorkerTimeouts::default()
            }
        }

        /// §5.3: `stop`; the turn has not ended in time, so `shutdown`; the
        /// process has not exited in time, so `SIGTERM` by registry identity.
        /// The real-process half is `tests/workers.rs`'s
        /// `stop_escalates_to_shutdown_then_sigterm_by_registry_pid`.
        #[test]
        fn the_stop_escalation_sends_shutdown_then_sigterm_after_its_timeouts() {
            let mut target = FakeTarget::new("running");
            let (answer, next) = begin_stop(&mut target);
            assert_eq!(answer, json!({"ok": true}), "the stop's own answer");
            assert_eq!(next, StopNext::AwaitTurnEnd);

            let started = Instant::now();
            assert_eq!(
                finish_stop(&mut target, next, &short()),
                StopOutcome::Terminated
            );
            let elapsed = started.elapsed();
            assert!(
                elapsed >= Duration::from_millis(300) && elapsed < Duration::from_secs(5),
                "waited both timeouts and no longer: {elapsed:?}"
            );
            assert!(target.terminated);
            assert_eq!(&target.sent[..2], ["status", "stop"]);
            assert_eq!(
                target
                    .sent
                    .iter()
                    .filter(|sent| *sent == "shutdown")
                    .count(),
                1,
                "{:?}",
                target.sent
            );
        }

        #[test]
        fn a_stop_that_ends_the_turn_goes_no_further() {
            let mut target = FakeTarget::new("paused");
            target.stop_ends_turn = true;
            let (_, next) = begin_stop(&mut target);
            assert_eq!(
                finish_stop(&mut target, next, &short()),
                StopOutcome::TurnEnded
            );
            assert!(!target.sent.contains(&"shutdown".to_string()));
            assert!(!target.terminated);
        }

        #[test]
        fn a_worker_that_exits_on_shutdown_is_not_signalled() {
            let mut target = FakeTarget::new("running");
            target.exits_on_shutdown = true;
            let (_, next) = begin_stop(&mut target);
            assert_eq!(
                finish_stop(&mut target, next, &short()),
                StopOutcome::ExitedAfterShutdown
            );
            assert!(!target.terminated);
        }

        /// Task 6 (d): `stop` is refused while waiting for approval, so the
        /// escalation does not send it and goes straight to `shutdown`.
        #[test]
        fn a_stop_while_waiting_for_approval_goes_straight_to_shutdown() {
            let mut target = FakeTarget::new("waiting_for_approval");
            target.exits_on_shutdown = true;
            let (answer, next) = begin_stop(&mut target);
            assert_eq!(next, StopNext::Shutdown);
            assert_eq!(answer["ok"], true, "{answer}");
            assert_eq!(target.sent, ["status"]);
            assert_eq!(
                finish_stop(&mut target, next, &short()),
                StopOutcome::ExitedAfterShutdown
            );
            assert_eq!(target.sent, ["status", "shutdown"]);
        }

        /// An idle worker has no turn to stop and is left running.
        #[test]
        fn an_idle_worker_is_not_escalated() {
            let mut target = FakeTarget::new("idle");
            let (answer, next) = begin_stop(&mut target);
            assert_eq!(next, StopNext::Done);
            assert_eq!(answer["ok"], false, "the worker's own refusal: {answer}");
            assert_eq!(finish_stop(&mut target, next, &short()), StopOutcome::Done);
            assert!(!target.sent.contains(&"shutdown".to_string()));
        }

        /// A worker that does not answer at all is shut down, then signalled.
        #[test]
        fn a_silent_worker_is_shut_down_then_signalled() {
            let mut target = FakeTarget::new("running");
            target.answers = false;
            let (answer, next) = begin_stop(&mut target);
            assert_eq!(next, StopNext::Shutdown);
            assert_eq!(answer["ok"], false);
            assert_eq!(
                finish_stop(&mut target, next, &short()),
                StopOutcome::Terminated
            );
        }
    }
    /// Spec 32 Task 9 over real HTTP: list, review, toggle and test a hook.
    ///
    /// The review writes user config only — the endpoint has no path parameter,
    /// and a `path` field naming the repository's own `.damaian/config.conf` is
    /// ignored. A pending repository hook cannot be tested before it is
    /// reviewed, and a stale approval hash is refused rather than adopting a
    /// definition the user never saw. The test run reports redacted stderr.
    #[test]
    fn hook_routes_list_review_toggle_and_test_over_http() {
        let repo = std::env::temp_dir().join(format!(
            "damaian-hook-routes-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(repo.join(".damaian")).unwrap();
        fs::create_dir_all(repo.join("scripts")).unwrap();
        let lint_script = repo.join("scripts").join("lint-hook.sh");
        let lint_marker = repo.join("lint-ran");
        fs::write(
            &lint_script,
            format!(
                "#!/bin/sh\ntouch {}\necho '{{\"hookApiVersion\":1,\"verdict\":\"allow\"}}'\n",
                lint_marker.to_string_lossy()
            ),
        )
        .unwrap();
        let repository_config = Config::repository_config_path(&repo);
        fs::write(
            &repository_config,
            "hook.lint.command=/bin/sh\n\
             hook.lint.args=./scripts/lint-hook.sh\n\
             hook.lint.events=before_tool\n\
             hook.lint.mode=optional\n",
        )
        .unwrap();
        let repository_before = fs::read_to_string(&repository_config).unwrap();

        let data_dir = isolated_data_dir();
        let user_config = data_dir.join("config").join("user.conf");
        fs::create_dir_all(user_config.parent().unwrap()).unwrap();
        let noter_script = data_dir.join("noter.sh");
        fs::write(
            &noter_script,
            "cat > /dev/null\n\
             echo '{\"hookApiVersion\":1,\"verdict\":\"warn\",\"reason\":\"heads up\"}'\n\
             echo 'aws key AKIAIOSFODNN7EXAMPLE' >&2\n",
        )
        .unwrap();
        fs::write(
            &user_config,
            format!(
                "hook.noter.command=/bin/sh\nhook.noter.args={}\nhook.noter.events=before_tool\n",
                noter_script.to_string_lossy()
            ),
        )
        .unwrap();

        let repo_arg = repo.to_string_lossy().to_string();
        let (port, token) = serve_for_test();
        let hooks_path = format!("/api/hooks?repo={}", percent_encode_for_test(&repo_arg));

        // Both are listed; the repository hook is pending until reviewed.
        let listed = get_for_test(port, &token, &hooks_path);
        assert!(listed.starts_with("HTTP/1.1 200"), "{listed}");
        let listed = json_of(&listed);
        let rows = listed["hooks"].as_array().expect("a hooks list");
        assert_eq!(rows.len(), 2, "{listed}");
        let lint = rows.iter().find(|row| row["id"] == "lint").unwrap();
        assert_eq!(lint["activation"], "pending_review");
        assert_eq!(lint["source"], "repository");
        assert_eq!(lint["mode"], "optional");
        assert_eq!(lint["command"], "/bin/sh");
        assert_eq!(lint["events"][0], "before_tool");
        let hash = lint["hash"]
            .as_str()
            .expect("a definition hash")
            .to_string();
        let noter = rows.iter().find(|row| row["id"] == "noter").unwrap();
        assert_eq!(noter["activation"], "active");
        assert_eq!(noter["source"], "user");

        // A pending repository hook is not executed by a test run.
        let refused = post_fields_for_test(
            port,
            &token,
            "/api/hook-test",
            &[
                ("repo", repo_arg.as_str()),
                ("hook", "lint"),
                ("event", "before_tool"),
            ],
        );
        assert!(refused.starts_with("HTTP/1.1 500"), "{refused}");
        assert!(refused.contains("review"), "{refused}");
        assert!(!lint_marker.exists(), "an unreviewed hook ran");

        // A stale hash is refused, and nothing is written.
        let stale = post_fields_for_test(
            port,
            &token,
            "/api/hook-review",
            &[
                ("repo", repo_arg.as_str()),
                ("hook", "lint"),
                ("decision", "approve"),
                ("hash", "sha256:deadbeef"),
            ],
        );
        assert!(stale.starts_with("HTTP/1.1 500"), "{stale}");
        assert!(
            !fs::read_to_string(&user_config)
                .unwrap()
                .contains("hook_review."),
            "a refused review wrote user config"
        );

        // The real review: a `path` field naming the repository's own config is
        // ignored, because the endpoint has no such parameter.
        let reviewed = post_fields_for_test(
            port,
            &token,
            "/api/hook-review",
            &[
                ("repo", repo_arg.as_str()),
                ("hook", "lint"),
                ("decision", "approve"),
                ("hash", hash.as_str()),
                ("path", repository_config.to_str().unwrap()),
            ],
        );
        assert!(reviewed.starts_with("HTTP/1.1 200"), "{reviewed}");
        let reviewed = json_of(&reviewed);
        let lint = reviewed["hooks"]
            .as_array()
            .unwrap()
            .iter()
            .find(|row| row["id"] == "lint")
            .unwrap();
        assert_eq!(lint["activation"], "active", "{reviewed}");
        assert_eq!(
            fs::read_to_string(&repository_config).unwrap(),
            repository_before,
            "the endpoint wrote the repository's own config"
        );
        let user = fs::read_to_string(&user_config).unwrap();
        assert!(user.contains("hook_review."), "{user}");
        assert!(user.contains(&hash), "{user}");

        // A user hook can be switched off, and the definition survives.
        let disabled = post_fields_for_test(
            port,
            &token,
            "/api/hook-enabled",
            &[
                ("repo", repo_arg.as_str()),
                ("hook", "noter"),
                ("enabled", "false"),
            ],
        );
        assert!(disabled.starts_with("HTTP/1.1 200"), "{disabled}");
        let disabled = json_of(&disabled);
        let noter = disabled["hooks"]
            .as_array()
            .unwrap()
            .iter()
            .find(|row| row["id"] == "noter")
            .unwrap();
        assert_eq!(noter["activation"], "disabled", "{disabled}");
        assert_eq!(noter["enabled"], false);

        // A repository's hook is not the user's to switch off.
        let refused_toggle = post_fields_for_test(
            port,
            &token,
            "/api/hook-enabled",
            &[
                ("repo", repo_arg.as_str()),
                ("hook", "lint"),
                ("enabled", "false"),
            ],
        );
        assert!(
            refused_toggle.starts_with("HTTP/1.1 500"),
            "{refused_toggle}"
        );
        assert_eq!(
            fs::read_to_string(&repository_config).unwrap(),
            repository_before
        );

        // A test run reports the verdict and redacted stderr.
        let tested = post_fields_for_test(
            port,
            &token,
            "/api/hook-test",
            &[
                ("repo", repo_arg.as_str()),
                ("hook", "noter"),
                ("event", "before_tool"),
            ],
        );
        assert!(tested.starts_with("HTTP/1.1 200"), "{tested}");
        let tested = json_of(&tested);
        assert_eq!(tested["verdict"], "warn", "{tested}");
        assert!(tested["failure"].is_null(), "{tested}");
        let stderr = tested["stderr"].as_str().expect("stderr");
        assert!(stderr.contains("[REDACTED_AWS_ACCESS_KEY"), "{stderr}");
        assert!(!stderr.contains("AKIAIOSFODNN7EXAMPLE"), "{stderr}");

        // The reviewed repository hook now runs, and the settings listing shows
        // it active.
        let ran = post_fields_for_test(
            port,
            &token,
            "/api/hook-test",
            &[
                ("repo", repo_arg.as_str()),
                ("hook", "lint"),
                ("event", "before_tool"),
            ],
        );
        assert!(ran.starts_with("HTTP/1.1 200"), "{ran}");
        assert_eq!(json_of(&ran)["verdict"], "allow");
        assert!(lint_marker.exists(), "the reviewed hook did not run");

        let _ = fs::remove_dir_all(&repo);
    }

    struct CommitRouteFixture {
        repo: PathBuf,
        session_id: String,
        engine: WorkspaceEngine,
    }

    fn git_in(repo: &Path, args: &[&str]) -> String {
        let output = std::process::Command::new("git")
            .arg("-C")
            .arg(repo)
            .args(args)
            .output()
            .expect("git should run");
        assert!(
            output.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).trim().to_string()
    }

    /// A real repository with one accepted change and a Code-mode session. The
    /// data directory is the test binary's isolated one, which the shell's own
    /// `engine_for_repo` also resolves, so the route sees the same session and
    /// patch the test created.
    fn commit_route_fixture(name: &str) -> CommitRouteFixture {
        let data_dir = isolated_data_dir().to_path_buf();
        let repo = temp_path(name);
        std::fs::create_dir_all(&repo).unwrap();
        git_in(&repo, &["init", "--quiet", "--initial-branch=main"]);
        git_in(&repo, &["config", "user.email", "test@damaian.local"]);
        git_in(&repo, &["config", "user.name", "Damaian Test"]);
        git_in(&repo, &["config", "commit.gpgsign", "false"]);
        std::fs::write(repo.join("a.txt"), "original\n").unwrap();
        git_in(&repo, &["add", "-A"]);
        git_in(&repo, &["commit", "--quiet", "-m", "initial"]);

        let engine = WorkspaceEngine::new(Config {
            data_dir,
            enable_index_watcher: false,
            ..Config::default()
        });
        let repository_id = workspace_engine::repository_id_for_root(&repo);
        let session = engine
            .session_store
            .create_session(&repository_id, "Commit routes")
            .unwrap();
        engine
            .session_store
            .append_message(&session.id, None, "user", "Add a file")
            .unwrap();
        let task = engine
            .session_store
            .create_task(&session.id, "Add a file", "test", "test")
            .unwrap();
        std::fs::write(repo.join("a.txt"), "damaian\n").unwrap();
        let patch = engine
            .patch_engine
            .create_patch(
                &repo,
                &[workspace_engine::ProposedChange {
                    path: "a.txt".to_string(),
                    new_content: "damaian\n".to_string(),
                    status: None,
                    allow_restricted: false,
                }],
                Some(&task.id),
                "test",
            )
            .unwrap();
        let mut patch = patch;
        patch.session_id = session.id.clone();
        patch.origin_session_id = session.id.clone();
        engine.patch_store.save(&patch).unwrap();
        engine
            .edit_orchestrator
            .apply_stored_patch(&repo, &patch.id, None, None, "tester", false)
            .unwrap();
        CommitRouteFixture {
            repo,
            session_id: session.id,
            engine,
        }
    }

    #[test]
    fn commit_status_and_preparation_are_served() {
        let fixture = commit_route_fixture("commit-status");
        let (port, token) = serve_for_test();
        let repo = percent_encode_for_test(&fixture.repo.to_string_lossy());
        let status = get_for_test(
            port,
            &token,
            &format!(
                "/api/commit-status?repo={repo}&session={}",
                fixture.session_id
            ),
        );
        assert!(status.starts_with("HTTP/1.1 200"), "{status}");
        let status = response_json(&status);
        assert_eq!(status["candidateCount"], 1);
        assert_eq!(status["selectedCount"], 1);
        assert_eq!(status["canCommit"], true);

        let preparation = get_for_test(
            port,
            &token,
            &format!(
                "/api/commit-preparation?repo={repo}&session={}",
                fixture.session_id
            ),
        );
        assert!(preparation.starts_with("HTTP/1.1 200"), "{preparation}");
        let preparation = response_json(&preparation);
        assert_eq!(preparation["diffRedacted"], true);
        assert!(preparation["hooks"]["hooks"].is_array());
        assert_eq!(preparation["candidates"][0]["path"], "a.txt");
        assert_eq!(preparation["candidates"][0]["selectedByDefault"], true);
        let _ = std::fs::remove_dir_all(&fixture.repo);
    }

    #[test]
    fn a_commit_needs_its_own_approval() {
        let fixture = commit_route_fixture("commit-approval");
        let (port, token) = serve_for_test();
        let repo_text = fixture.repo.to_string_lossy().to_string();
        let repo = percent_encode_for_test(&repo_text);
        let preparation = response_json(&get_for_test(
            port,
            &token,
            &format!(
                "/api/commit-preparation?repo={repo}&session={}",
                fixture.session_id
            ),
        ));
        let prepare_id = preparation["prepareId"].as_str().unwrap();
        let before = git_in(&fixture.repo, &["rev-parse", "HEAD"]);

        let response = post_fields_for_test(
            port,
            &token,
            "/api/commit",
            &[
                ("repo", repo_text.as_str()),
                ("session_id", fixture.session_id.as_str()),
                ("prepare_id", prepare_id),
                ("paths", "a.txt"),
                ("message", "Commit a"),
            ],
        );
        assert!(response.starts_with("HTTP/1.1 400"), "{response}");
        let body = response_json(&response);
        assert_eq!(body["code"], "approval_required");
        assert_eq!(git_in(&fixture.repo, &["rev-parse", "HEAD"]), before);
        let _ = std::fs::remove_dir_all(&fixture.repo);
    }

    #[test]
    fn a_commit_is_refused_in_ask_mode() {
        let fixture = commit_route_fixture("commit-ask");
        fixture
            .engine
            .session_store
            .set_session_mode(&fixture.session_id, SessionMode::Ask, "user")
            .unwrap();
        let (port, token) = serve_for_test();
        let repo_text = fixture.repo.to_string_lossy().to_string();
        let repo = percent_encode_for_test(&repo_text);
        let preparation = response_json(&get_for_test(
            port,
            &token,
            &format!(
                "/api/commit-preparation?repo={repo}&session={}",
                fixture.session_id
            ),
        ));
        let prepare_id = preparation["prepareId"].as_str().unwrap();
        assert_eq!(preparation["canCommit"], false);
        assert!(
            preparation["refusals"][0]
                .as_str()
                .unwrap()
                .contains("Code mode")
        );
        let response = post_fields_for_test(
            port,
            &token,
            "/api/commit",
            &[
                ("repo", repo_text.as_str()),
                ("session_id", fixture.session_id.as_str()),
                ("prepare_id", prepare_id),
                ("approval", "commit"),
                ("paths", "a.txt"),
                ("message", "Commit a"),
            ],
        );
        assert!(response.starts_with("HTTP/1.1 409"), "{response}");
        let body = response_json(&response);
        assert_eq!(body["code"], "commit_not_allowed");
        let _ = std::fs::remove_dir_all(&fixture.repo);
    }

    #[test]
    fn a_commit_route_creates_the_commit_and_reports_the_sha() {
        let fixture = commit_route_fixture("commit-ok");
        let (port, token) = serve_for_test();
        let repo_text = fixture.repo.to_string_lossy().to_string();
        let repo = percent_encode_for_test(&repo_text);
        let preparation = response_json(&get_for_test(
            port,
            &token,
            &format!(
                "/api/commit-preparation?repo={repo}&session={}",
                fixture.session_id
            ),
        ));
        let prepare_id = preparation["prepareId"].as_str().unwrap();
        let response = post_fields_for_test(
            port,
            &token,
            "/api/commit",
            &[
                ("repo", repo_text.as_str()),
                ("session_id", fixture.session_id.as_str()),
                ("prepare_id", prepare_id),
                ("approval", "commit"),
                ("paths", "a.txt"),
                ("message", "Commit a"),
                ("allow_generated_secrets", "false"),
            ],
        );
        assert!(response.starts_with("HTTP/1.1 200"), "{response}");
        let body = response_json(&response);
        let sha = body["commitSha"].as_str().expect("a SHA");
        assert_eq!(git_in(&fixture.repo, &["rev-parse", "HEAD"]), sha);
        assert_eq!(git_in(&fixture.repo, &["show", "HEAD:a.txt"]), "damaian");
        let _ = std::fs::remove_dir_all(&fixture.repo);
    }

    #[test]
    fn a_rewritten_message_is_returned_with_a_code() {
        let fixture = commit_route_fixture("commit-rewrite");
        let hook = fixture.repo.join(".git/hooks/commit-msg");
        std::fs::write(
            &hook,
            "#!/bin/sh\ngrep -q '^Reviewed-by: Test$' \"$1\" || printf '\\nReviewed-by: Test\\n' >> \"$1\"\n",
        )
        .unwrap();
        use std::os::unix::fs::PermissionsExt;
        let mut permissions = std::fs::metadata(&hook).unwrap().permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&hook, permissions).unwrap();

        let (port, token) = serve_for_test();
        let repo_text = fixture.repo.to_string_lossy().to_string();
        let repo = percent_encode_for_test(&repo_text);
        let preparation = response_json(&get_for_test(
            port,
            &token,
            &format!(
                "/api/commit-preparation?repo={repo}&session={}",
                fixture.session_id
            ),
        ));
        let prepare_id = preparation["prepareId"].as_str().unwrap();
        let before = git_in(&fixture.repo, &["rev-parse", "HEAD"]);
        let response = post_fields_for_test(
            port,
            &token,
            "/api/commit",
            &[
                ("repo", repo_text.as_str()),
                ("session_id", fixture.session_id.as_str()),
                ("prepare_id", prepare_id),
                ("approval", "commit"),
                ("paths", "a.txt"),
                ("message", "Commit a"),
            ],
        );
        assert!(response.starts_with("HTTP/1.1 409"), "{response}");
        let body = response_json(&response);
        assert_eq!(body["code"], "commit_message_rewritten");
        assert!(
            body["message"]
                .as_str()
                .unwrap()
                .contains("Reviewed-by: Test")
        );
        assert_eq!(git_in(&fixture.repo, &["rev-parse", "HEAD"]), before);
        let _ = std::fs::remove_dir_all(&fixture.repo);
    }
}

/// The shell half of spec 56: the offer's JSON and the decision route.
#[cfg(test)]
mod provider_fallback_tests {
    use super::{TurnEvent, chat_result_json, resume_fallback_decision, switch_model_provider};
    use std::cell::RefCell;
    use std::collections::HashMap;
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};
    use workspace_engine::{
        CancelToken, ChatTurnResult, ClientError, Config, MockModelAdapter, ModelAdapter,
        ModelProviderConfig, ModelRequest, ModelRun, ProviderRefusal, TaskStatus, TurnProgress,
        TurnSink, WorkspaceEngine,
    };

    static COUNTER: AtomicU64 = AtomicU64::new(1);

    fn temp_repo() -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "damaian-shell-fallback-{}-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("README.md"), "# Fallback\n").unwrap();
        dir
    }

    fn provider(id: &str, model: &str) -> ModelProviderConfig {
        ModelProviderConfig {
            id: id.to_string(),
            label: id.to_string(),
            base_url: format!("https://{id}.example.test"),
            api_key_env: format!("{}_KEY", id.to_uppercase()),
            models: vec![model.to_string()],
            supports_native_tools: false,
            max_output_tokens: None,
            context_token_budget: None,
            provider_reports_usage: true,
            price_per_million_input_tokens: None,
            price_per_million_output_tokens: None,
            price_per_million_cached_input_tokens: None,
            supports_explicit_cache_breakpoints: false,
        }
    }

    /// `primary` active and `fallback` beside it, each with its own key, in the
    /// repository's own data directory. Built directly rather than loaded, so
    /// the test touches neither the user's config nor the binary's shared one.
    fn two_provider_config(repo: &Path) -> Config {
        Config {
            data_dir: repo.join(".damaian"),
            enable_index_watcher: false,
            model_provider: "primary".to_string(),
            model_name: "model-a".to_string(),
            model_api_key_env: "PRIMARY_KEY".to_string(),
            model_providers: vec![
                provider("primary", "model-a"),
                provider("fallback", "model-b"),
            ],
            ..Config::default()
        }
    }

    struct RateLimited;

    impl ModelAdapter for RateLimited {
        fn stream_response(
            &mut self,
            _request: &ModelRequest,
            _cancel: &CancelToken,
            _on_token: &mut dyn FnMut(&str),
            _on_wait: &mut dyn FnMut(u64),
        ) -> workspace_engine::Result<ModelRun> {
            Err(ClientError::Provider(
                ProviderRefusal::RateLimited {
                    retry_after_secs: None,
                },
                "rate limited".to_string(),
            ))
        }
    }

    /// A refused turn, paused on an offer to answer with `fallback`.
    fn paused_offer(repo: &Path) -> ChatTurnResult {
        let engine = WorkspaceEngine::new(two_provider_config(repo));
        let cancel = CancelToken::new();
        let mut on_token = |_token: &str| {};
        let mut on_progress = |_event: TurnProgress| {};
        let mut sink = TurnSink {
            on_token: &mut on_token,
            on_progress: &mut on_progress,
            cancel: &cancel,
            steering: None,
        };
        let result = engine
            .chat_orchestrator
            .ask_with_session(
                repo,
                "explain the readme",
                &[],
                None,
                &mut RateLimited,
                &mut sink,
            )
            .expect("a refusal with a candidate pauses rather than failing");
        assert_eq!(result.task.status, TaskStatus::WaitingForApproval);
        result
    }

    fn form(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(key, value)| (key.to_string(), value.to_string()))
            .collect()
    }

    /// Runs the route's decision with engines built the way the shell builds
    /// them, and returns the result plus every provider an engine was built for
    /// and every provider an adapter (and so a key) was asked for.
    fn decide(
        repo: &Path,
        form: &HashMap<String, String>,
        answer: &str,
    ) -> (
        Result<ChatTurnResult, String>,
        Vec<Option<String>>,
        Vec<String>,
    ) {
        let engines = RefCell::new(Vec::new());
        let mut adapters = Vec::new();
        let (events, _receiver) = std::sync::mpsc::channel::<TurnEvent>();
        let result = resume_fallback_decision(
            form,
            &CancelToken::new(),
            &events,
            &|provider| {
                engines.borrow_mut().push(provider.map(str::to_string));
                let mut config = two_provider_config(repo);
                if let Some(provider) = provider {
                    switch_model_provider(&mut config, provider)?;
                }
                Ok(WorkspaceEngine::new(config))
            },
            &mut |config| {
                adapters.push(format!("{}/{}", config.model_provider, config.model_name));
                Ok(Box::new(MockModelAdapter::new(answer)))
            },
        );
        (result, engines.into_inner(), adapters)
    }

    fn task_status(repo: &Path, result: &ChatTurnResult) -> String {
        let engine = WorkspaceEngine::new(two_provider_config(repo));
        engine
            .session_store
            .read_task_statuses(&result.session.id)
            .unwrap()
            .remove(&result.task.id)
            .expect("the task has a status")
    }

    #[test]
    fn the_turn_json_names_both_providers_and_offers_no_always() {
        let repo = temp_repo();
        let result = paused_offer(&repo);

        let json: serde_json::Value = serde_json::from_str(&chat_result_json(&result)).unwrap();
        let offer = &json["fallbackProposal"];
        assert_eq!(offer["refusedProvider"], "primary");
        assert_eq!(offer["refusedModel"], "model-a");
        assert_eq!(offer["fallbackProvider"], "fallback");
        assert_eq!(offer["fallbackModel"], "model-b");
        assert_eq!(offer["refusalCode"], "provider_rate_limited");
        assert_eq!(offer["message"], "rate limited");
        assert!(
            offer["proposalId"]
                .as_str()
                .is_some_and(|id| !id.is_empty())
        );
        let fields: Vec<&String> = offer.as_object().unwrap().keys().collect();
        assert!(
            fields
                .iter()
                .all(|field| !field.to_lowercase().contains("always")),
            "requirement 3: there is no always, {fields:?}"
        );
        assert_eq!(json["taskStatus"], "waiting_for_approval");
    }

    #[test]
    fn an_ordinary_turn_carries_a_null_fallback_proposal() {
        let repo = temp_repo();
        let engine = WorkspaceEngine::new(two_provider_config(&repo));
        let cancel = CancelToken::new();
        let mut on_token = |_token: &str| {};
        let mut on_progress = |_event: TurnProgress| {};
        let mut sink = TurnSink {
            on_token: &mut on_token,
            on_progress: &mut on_progress,
            cancel: &cancel,
            steering: None,
        };
        let result = engine
            .chat_orchestrator
            .ask_with_session(
                &repo,
                "hello",
                &[],
                None,
                &mut MockModelAdapter::new("hi"),
                &mut sink,
            )
            .unwrap();
        let json: serde_json::Value = serde_json::from_str(&chat_result_json(&result)).unwrap();
        assert!(json["fallbackProposal"].is_null());
    }

    #[test]
    fn a_declined_offer_fails_the_task_and_resolves_no_key() {
        let repo = temp_repo();
        let offer = paused_offer(&repo);
        let proposal_id = offer.fallback_proposal.as_ref().unwrap().id.clone();

        // `always` is ignored, not honoured: it cannot turn a decline into
        // anything, and on an approval it must not make the switch stick.
        let (result, engines, adapters) = decide(
            &repo,
            &form(&[
                ("repo", "unused"),
                ("proposal_id", &proposal_id),
                ("approved", "false"),
                ("always", "true"),
            ]),
            "must not be used",
        );

        let error = result.expect_err("a decline fails the turn as spec 48 does");
        assert!(error.contains("rate limited"), "{error}");
        assert!(
            adapters.is_empty(),
            "a decline builds no transport: {adapters:?}"
        );
        assert_eq!(
            engines,
            vec![None],
            "a decline stays on the configured provider"
        );
        assert_eq!(task_status(&repo, &offer), "failed");
    }

    #[test]
    fn an_unknown_offer_errors_before_any_key_is_resolved() {
        let repo = temp_repo();
        let (result, _engines, adapters) = decide(
            &repo,
            &form(&[
                ("repo", "unused"),
                ("proposal_id", "chatprop_missing"),
                ("approved", "true"),
            ]),
            "must not be used",
        );

        let error = result.expect_err("there is no such offer");
        assert!(
            error.contains("No pending provider fallback offer"),
            "{error}"
        );
        assert!(adapters.is_empty(), "no key for an offer that is not there");
    }

    #[test]
    fn an_approval_builds_the_fallbacks_engine_and_adapter() {
        let repo = temp_repo();
        let offer = paused_offer(&repo);
        let proposal_id = offer.fallback_proposal.as_ref().unwrap().id.clone();

        let (result, engines, adapters) = decide(
            &repo,
            &form(&[
                ("repo", "unused"),
                ("proposal_id", &proposal_id),
                ("approved", "true"),
            ]),
            "answered by the fallback",
        );

        let result = result.expect("the fallback answers");
        assert_eq!(result.response, "answered by the fallback");
        assert_eq!(engines, vec![None, Some("fallback".to_string())]);
        assert_eq!(
            adapters,
            vec!["fallback/model-b".to_string()],
            "C1: the key and the request belong to the fallback"
        );
        assert_eq!(task_status(&repo, &offer), "complete");
    }
}

/// The shell half of spec 61's round-budget pause: the route that resumes it.
#[cfg(test)]
mod round_budget_tests {
    use super::{TurnEvent, resume_round_budget_decision};
    use std::collections::HashMap;
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};
    use workspace_engine::{
        CancelToken, ChatTurnResult, Config, MockModelAdapter, ModelAdapter, TaskStatus,
        TurnProgress, TurnSink, WorkspaceEngine,
    };

    static COUNTER: AtomicU64 = AtomicU64::new(1);

    fn temp_repo() -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "damaian-shell-roundbudget-{}-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(dir.join("src")).unwrap();
        fs::write(dir.join("src/a.rs"), "fn main() {}\n").unwrap();
        dir
    }

    fn config(repo: &Path) -> Config {
        Config {
            data_dir: repo.join(".damaian"),
            agent_max_tool_rounds: 8,
            enable_index_watcher: false,
            ..Config::default()
        }
    }

    fn read_only_round() -> String {
        "DAMAIAN_COMMAND_V1\nCOMMAND: ls src\nREASON: Look again.\nEND_COMMAND\n".to_string()
    }

    fn form(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(key, value)| (key.to_string(), value.to_string()))
            .collect()
    }

    /// A turn that hit its round budget and paused for the user.
    fn paused_turn(repo: &Path) -> ChatTurnResult {
        let engine = WorkspaceEngine::new(config(repo));
        let cancel = CancelToken::new();
        let mut on_token = |_token: &str| {};
        let mut on_progress = |_event: TurnProgress| {};
        let mut sink = TurnSink {
            on_token: &mut on_token,
            on_progress: &mut on_progress,
            cancel: &cancel,
            steering: None,
        };
        let result = engine
            .chat_orchestrator
            .ask_with_session(
                repo,
                "Look at the source",
                &[],
                None,
                &mut MockModelAdapter::new_sequence((0..8).map(|_| read_only_round()).collect()),
                &mut sink,
            )
            .expect("the turn pauses");
        assert_eq!(result.task.status, TaskStatus::WaitingForApproval);
        result
    }

    #[test]
    fn the_round_budget_route_resumes_the_pause() {
        let repo = temp_repo();
        let paused = paused_turn(&repo);
        let proposal_id = paused
            .round_budget_proposal
            .as_ref()
            .expect("the pause surfaces a proposal")
            .id
            .clone();

        let (events, _receiver) = std::sync::mpsc::channel::<TurnEvent>();
        let mut adapters = 0;
        let result = resume_round_budget_decision(
            &form(&[("proposal_id", &proposal_id), ("decision", "continue")]),
            &CancelToken::new(),
            &events,
            &|| Ok(WorkspaceEngine::new(config(&repo))),
            &mut |_config| {
                adapters += 1;
                Ok(Box::new(MockModelAdapter::new("Done.")) as Box<dyn ModelAdapter>)
            },
        );

        let resumed = result.expect("the decision resumes the pause");
        assert_eq!(
            resumed.task.id, paused.task.id,
            "resuming continues the same task, never a new one"
        );
        assert_eq!(resumed.task.status, TaskStatus::Complete);
        assert_eq!(adapters, 1, "one adapter for the resumed turn");
    }
}

#[cfg(test)]
mod commit_message_tests {
    use super::*;
    use workspace_engine::{
        CommitCandidate, CommitCandidateSource, CommitRecord, CommitVerification, MockModelAdapter,
        PlanStep, StepStatus, TaskPlan,
    };

    struct Fixture {
        engine: WorkspaceEngine,
        session_id: String,
        _base: std::path::PathBuf,
    }

    fn fixture(name: &str) -> Fixture {
        let base = std::env::temp_dir().join(format!(
            "damaian-commit-message-{name}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();
        let engine = WorkspaceEngine::new(Config {
            data_dir: base.join("data"),
            enable_index_watcher: false,
            ..Config::default()
        });
        let session = engine
            .session_store
            .create_session("repo_test", "Commit message")
            .unwrap();
        engine
            .session_store
            .append_message(&session.id, None, "user", "Add a commit flow")
            .unwrap();
        let task = engine
            .session_store
            .create_task(&session.id, "Add a commit flow", "test", "test")
            .unwrap();
        let mut plan = TaskPlan::new(task.id.clone(), 1);
        plan.steps.push(PlanStep {
            id: "step_1".to_string(),
            title: "Add the dialog".to_string(),
            detail: None,
            status: StepStatus::InProgress,
            depends_on: Vec::new(),
            started_at_ms: None,
            completed_at_ms: None,
            evidence: Vec::new(),
        });
        engine.session_store.create_plan(&task, &plan).unwrap();
        Fixture {
            engine,
            session_id: session.id,
            _base: base,
        }
    }

    fn record(fixture: &Fixture) -> CommitRecord {
        CommitRecord {
            prepare_id: "commit_test".to_string(),
            repository_root: "/tmp/repo".to_string(),
            session_id: fixture.session_id.clone(),
            task_id: None,
            selected_paths: vec!["src/app.js".to_string()],
            branch: Some("main".to_string()),
            expected_old: None,
            candidates: vec![CommitCandidate {
                path: "src/app.js".to_string(),
                status: "modified".to_string(),
                source: CommitCandidateSource::AcceptedPatch,
                label: "Accepted change".to_string(),
                selected_by_default: true,
                blob_oid: Some("oid".to_string()),
                content_sha256: None,
                mode: Some("100644".to_string()),
                size: 10,
                unscanned: false,
                secret_warnings: Vec::new(),
                diff: "-old\n+new [REDACTED: aws_access_key]\n".to_string(),
                accepted_hunks: Vec::new(),
                renamed_from: None,
                renamed_to: None,
                copied_from: None,
            }],
            hooks: workspace_engine::GitHookReport::default(),
            message: "chore: Add a commit flow".to_string(),
            verification: CommitVerification::default(),
            prepared_at_ms: 0,
            state: "prepared".to_string(),
            prepared_commit: None,
            head_after: None,
            hook_outcomes: Vec::new(),
            generated_secret_override: false,
            hunk_count: 0,
        }
    }

    #[test]
    fn a_model_draft_is_shown_as_a_subject_and_body() {
        let fixture = fixture("model");
        let record = record(&fixture);
        let mut adapter = MockModelAdapter::new("feat: add the commit flow\n\nBody line.");
        let draft = draft_commit_message_with(
            &fixture.engine,
            &record,
            &["src/app.js".to_string()],
            Some(&mut adapter),
        );
        assert_eq!(draft.source, "model");
        assert_eq!(draft.subject, "feat: add the commit flow");
        assert_eq!(draft.body, "Body line.");
        assert_eq!(draft.message, "feat: add the commit flow\n\nBody line.");
        assert!(draft.notice.is_none());
    }

    #[test]
    fn a_failed_or_empty_model_draft_falls_back_to_the_template() {
        let fixture = fixture("fallback");
        let record = record(&fixture);
        let mut adapter = MockModelAdapter::new("");
        let draft = draft_commit_message_with(
            &fixture.engine,
            &record,
            &["src/app.js".to_string()],
            Some(&mut adapter),
        );
        assert_eq!(draft.source, "fallback");
        assert_eq!(draft.message, "chore: Add a commit flow");
        assert!(draft.notice.is_some());
    }

    #[test]
    fn no_configured_model_falls_back_without_a_call() {
        let fixture = fixture("no-model");
        let record = record(&fixture);
        let draft =
            draft_commit_message_with(&fixture.engine, &record, &["src/app.js".to_string()], None);
        assert_eq!(draft.source, "fallback");
        assert_eq!(draft.message, "chore: Add a commit flow");
    }

    #[test]
    fn the_model_sees_the_redacted_diff_and_the_plan_titles() {
        let fixture = fixture("prompt");
        let record = record(&fixture);
        let mut adapter = MockModelAdapter::new("feat: x");
        let _ = draft_commit_message_with(
            &fixture.engine,
            &record,
            &["src/app.js".to_string()],
            Some(&mut adapter),
        );
        let prompt = &adapter.requests[0].messages[1].content;
        assert!(prompt.contains("Add a commit flow"), "{prompt}");
        assert!(prompt.contains("Add the dialog"), "{prompt}");
        assert!(prompt.contains("[REDACTED: aws_access_key]"), "{prompt}");
        assert!(!prompt.contains("AKIAIOSFODNN7EXAMPLE"), "{prompt}");
        assert!(prompt.contains("src/app.js"), "{prompt}");
    }

    #[test]
    fn a_fenced_or_prefixed_answer_is_still_a_draft() {
        assert_eq!(
            parse_commit_draft("```\nSubject: feat: x\n\nBody\n```"),
            Some(("feat: x".to_string(), "Body".to_string()))
        );
        assert_eq!(parse_commit_draft("   \n\n"), None);
    }
}
