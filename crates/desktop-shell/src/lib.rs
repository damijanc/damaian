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
    AgentCheckBatch, AgentContextReview, AgentFallbackProposal, AgentFetchProposal,
    AgentPlanProposal, AgentQuestionProposal, AgentRoundBudgetProposal, BranchError,
    CURRENT_DATA_SCHEMA_VERSION, CancelToken, ChatMessage, ChatTurnOptions, ChatTurnResult,
    ClientError, CommitError, Config, ContextDecision, ContextManifest, CostEstimate,
    CurlModelTransport, DataSchemaOutcome, EffectivePolicy, ExportFormat, FetchDecision,
    GeneratedSecretWarning, GitMutation, HookActivation, HookEvent, LanguageServerStatus,
    LineRange, McpClient, McpServerConfig, McpTokenResolver, McpTransport, MemoryRecallRef,
    MemoryScope, ModelAdapter, ModelMessage, ModelRequest, ModelRun, OpenAICompatibleAdapter,
    PausedTurns, PlanRevisionCheck, PlanRevisionStep, PlannedCheck, ProcessRegistry, ProfileId,
    ProjectIdentity, ProposedFilePatch, QuestionState, RepositoryConfigReport,
    ResumeDecisionOptions, RoundBudgetDecision, SearchOptions, Session, SessionMode, SessionStore,
    StepStatus, SymbolRef, Task, TaskExecutionBudget, TaskPlan, TaskUsage, TokenUsage, TurnNotes,
    TurnPhase, TurnProgress, TurnSink, UserConfirmation, VerificationEvent, VerificationOutcome,
    VerificationReport, WebDiagnosticCall, WebDiagnosticKind, WebDiagnosticRecord,
    WebDiagnosticReport, WebDiagnosticsRunner, WebDiagnosticsRunnerHandle, WorkerReturn,
    WorkspaceEngine, allow_always_eligible, builtin_context_window, builtin_token_defaults,
    command_approval_prompt, custom_profile_ids, ensure_data_dir_schema, export_profile,
    fold_verification, import_profile, normalize_mcp_server_id, normalize_model_provider,
    normalize_model_reasoning_level, normalize_planned_check_directory, parse_hunk_selection,
    parse_mcp_transport, patch_diff_text, profile_import_base, repository_id_for_root,
    review_profile_import, review_profile_rejections, select_profile,
};

mod keychain;
mod lanes;
mod recovery;
mod routes;
pub mod terminal;
mod worker;
pub mod workers;

pub use worker::{WORKER_FLAG, run_worker_from_args, worker_socket_path};
pub use workers::shutdown_all_workers;

mod browser_diagnostics;
use browser_diagnostics::*;
mod config_api;
use config_api::*;
mod desktop_api;
pub use desktop_api::*;
mod edit_api;
use edit_api::*;
mod git_api;
use git_api::*;
mod http;
use http::*;
mod json;
use json::*;
mod memory_api;
use memory_api::*;
mod repository_api;
use repository_api::*;
mod session_api;
use session_api::*;
mod turn_api;
pub use turn_api::*;
mod turn_events;
use turn_events::*;

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

    serve_listener(listener, &options);
    // A normal exit: servers get `shutdown` and `exit` before they are
    // killed. A signal goes through `sweep_own` instead (spec 25 context.md §3).
    workspace_engine::LanguageServerHub::shutdown_global();
    Ok(())
}

/// Spec 110 D1/D5: every connection on its own thread, so a turn's stream
/// never holds the accept loop and the UI answers while sessions work.
fn serve_listener(listener: TcpListener, options: &ShellOptions) {
    for stream in listener.incoming() {
        match stream {
            Ok(stream) => {
                let options = options.clone();
                if let Err(error) = spawn_with_shell_stack("damaian-connection", move || {
                    serve_connection(stream, &options)
                }) {
                    eprintln!("start a connection thread: {error}");
                }
            }
            Err(error) => eprintln!("connection failed: {error}"),
        }
    }
}

/// Serves one connection; an error becomes a 500, as it always has.
fn serve_connection(mut stream: TcpStream, options: &ShellOptions) {
    if let Err(error) = handle_connection(&mut stream, options) {
        let _ = write_basic_response(&mut stream, 500, "application/json", &json_error(&error));
    }
    // Whatever path the request took, its lane is free once it is answered.
    lanes::release_current();
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
    let Some(route) = routes::route_for(&request.method, &request.path) else {
        return write_response(
            stream,
            &request,
            404,
            "application/json",
            &json_error("not found"),
        );
    };
    // Spec 110 D1–D2: a session's work holds that session's lane while it
    // runs; a second piece of work for the same session is refused at once
    // with what the first is doing, never queued. UI routes take no lane.
    if let routes::RouteClass::SessionWork(Some(session_id)) = route_class(route, &request) {
        match lanes::claim(&session_id, route.work_label()) {
            Ok(guard) => lanes::hold_current(guard),
            Err(running) => {
                return write_response(
                    stream,
                    &request,
                    409,
                    "application/json",
                    &session_working_json(&session_id, &running),
                );
            }
        }
    }
    let outcome = dispatch(stream, &request, route, options);
    lanes::release_current();
    outcome
}

/// The class of one request, with the session it acts for when it names one
/// (spec 110 D2). A resume route names only its paused turn, so the turn's
/// snapshot says whose it is.
fn route_class(route: routes::Route, request: &Request) -> routes::RouteClass {
    if !route.is_session_work() {
        return routes::RouteClass::Ui;
    }
    let form = parse_form(&request.body);
    let field = routes::session_field(route);
    let named = form
        .get(field)
        .or_else(|| request.query.get(field))
        .filter(|value| !value.is_empty())
        .cloned();
    if named.is_some() {
        return routes::RouteClass::SessionWork(named);
    }
    let paused = form
        .get("proposal_id")
        .filter(|id| is_plain_id(id))
        .and_then(|id| {
            let data_dir = effective_data_dir().ok()?;
            PausedTurns::new(data_dir).session_id(id)
        });
    routes::RouteClass::SessionWork(paused)
}

/// An id that can name a file under the data directory without leaving it.
fn is_plain_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 256
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
}

/// A refused request for a session that is already working (spec 110 req. 2).
fn session_working_json(session_id: &str, running: &str) -> String {
    serde_json::json!({
        "error": format!("This session is already working: {running}. Wait for it to finish or stop it first."),
        "code": "session_working",
        "sessionId": session_id,
        "label": running,
    })
    .to_string()
}

/// The handler for each route. Exhaustive, like [`routes::Route::is_session_work`].
fn dispatch(
    stream: &mut TcpStream,
    request: &Request,
    route: routes::Route,
    options: &ShellOptions,
) -> Result<(), String> {
    use routes::Route;
    match route {
        Route::Index => write_response(
            stream,
            request,
            200,
            "text/html; charset=utf-8",
            &index_html(),
        ),
        Route::StyleCss => {
            write_response(stream, request, 200, "text/css; charset=utf-8", STYLE_CSS)
        }
        Route::AppJs => write_response(
            stream,
            request,
            200,
            "application/javascript; charset=utf-8",
            APP_JS,
        ),
        Route::XtermJs => write_response(
            stream,
            request,
            200,
            "application/javascript; charset=utf-8",
            XTERM_JS,
        ),
        Route::XtermAddonFitJs => write_response(
            stream,
            request,
            200,
            "application/javascript; charset=utf-8",
            XTERM_ADDON_FIT_JS,
        ),
        Route::XtermCss => {
            write_response(stream, request, 200, "text/css; charset=utf-8", XTERM_CSS)
        }
        Route::Bootstrap => {
            let repo = options.default_repo.clone().unwrap_or_default();
            write_response(
                stream,
                request,
                200,
                "application/json",
                &format!("{{\"defaultRepo\":\"{}\"}}", escape_json(&repo)),
            )
        }
        Route::WebDiagnosticArtifact => handle_web_diagnostic_artifact(stream, request),
        Route::Config => handle_config(stream, request),
        Route::GetConfigFile => handle_get_config_file(stream, request),
        Route::EffectivePolicy => handle_effective_policy(stream, request),
        Route::ProviderTokenDefaults => handle_provider_token_defaults(stream, request),
        Route::PermissionProfile => handle_permission_profile(stream, request),
        Route::PermissionProfiles => handle_permission_profiles(stream, request),
        Route::PermissionProfileExport => handle_permission_profile_export(stream, request),
        Route::PermissionProfileImport => handle_permission_profile_import(stream, request),
        Route::ModelKeyStatus => handle_model_key_status(stream, request),
        Route::GitStatus => handle_git_status(stream, request),
        Route::CommitMessage => handle_commit_message(stream, request),
        Route::CommitStatus => handle_commit_status(stream, request),
        // A preparation writes a durable record and an audit entry, so it is a
        // `POST`: a safe-looking `GET` with side effects is a cache and retry
        // hazard (review finding 3h).
        Route::CommitPreparation => handle_commit_preparation(stream, request),
        Route::Commit => handle_commit(stream, request),
        // Spec 36: branch state, the prepared operation, its typed approval,
        // and two read-only reports. There is deliberately no route that runs
        // a merge, rebase, or cherry-pick, and no route takes a raw command.
        Route::BranchInfo => handle_branch_info(stream, request),
        Route::BranchSuggestion => handle_branch_suggestion(stream, request),
        Route::BranchPrepare => handle_branch_prepare(stream, request),
        Route::BranchChange => handle_branch_change(stream, request),
        Route::MergePreparation => handle_merge_preparation(stream, request),
        Route::TerminalCwd => handle_terminal_cwd(stream, request),
        Route::TerminalRun => handle_terminal_run(stream, request),
        Route::Sessions => handle_sessions(stream, request),
        Route::SessionSearch => handle_session_search(stream, request),
        Route::SessionExport => handle_session_export(stream, request),
        // Spec 110 D3: which sessions are working, and a cancel for one.
        Route::WorkingSessions => handle_working_sessions(stream, request),
        Route::CancelSessionWork => handle_cancel_session_work(stream, request),
        Route::SessionNote => handle_session_note(stream, request),
        // The launch sweep behind `docs/specs/45_crash_recovery_prompt.md`. It
        // runs once per process; every call after the first serves the same
        // snapshot, minus tasks the user has since dealt with.
        Route::Recovery => handle_recovery(stream, request),
        Route::PendingCard => handle_pending_card(stream, request),
        // Resume, mark failed, or abandon one recovered task. The decision is
        // re-classified server-side: see `recovery::decide`.
        Route::RecoveryDecision => handle_recovery_decision(stream, request),
        // Closes out a task whose reattached approval has been answered, so the
        // next launch does not offer to run the same command again.
        Route::RecoveryApprovalResolved => handle_recovery_approval_resolved(stream, request),
        Route::Checkpoints => handle_checkpoints(stream, request),
        // Rewind. `files` and `conversation` are independent switches, and
        // `path` narrows the file half to one file, which is the fourth of the
        // restore operations rather than a fourth code path.
        Route::Rewind => handle_rewind(stream, request),
        Route::Session => handle_session(stream, request),
        Route::SessionCreate => handle_session_create(stream, request),
        Route::SessionRename => handle_session_rename(stream, request),
        Route::SessionArchive => handle_session_archive(stream, request, true),
        Route::SessionUnarchive => handle_session_archive(stream, request, false),
        Route::CommandSessionApproval => handle_command_session_approval(stream, request),
        Route::SessionApprovalsRevoke => handle_session_approvals_revoke(stream, request),
        Route::SessionMode => handle_session_mode(stream, request),
        // Spec 98 Task 4: the user's explicit protected-path control. Every
        // route is repository-bound, and writes go through the engine's
        // canonicalising API rather than appending session events here.
        Route::ContextPin => handle_context_pin(stream, request),
        Route::ContextRestriction => handle_context_restriction(stream, request),
        Route::ContextRemove => handle_context_remove(stream, request),
        Route::ContextRemoveSummary => handle_context_remove_summary(stream, request),
        // The manifest names memory entries but never their statements (spec 30
        // C3); the panel resolves the rows it was shown through this read.
        Route::ContextMemory => handle_context_memory(stream, request),
        Route::ResumeContextStream => handle_resume_context_stream(stream, request),
        // Spec 29: the pending proposals and the two user actions. There is no
        // accept-all route and no route that takes a statement.
        Route::MemoryProposals => handle_memory_proposals(stream, request),
        Route::MemoryConfirm => handle_memory_confirm(stream, request),
        Route::MemoryDismiss => handle_memory_dismiss(stream, request),
        // Spec 22 Task 10. Each findings route is its own function, so the
        // engine it builds lives in that frame rather than this one: every
        // arm's locals share this function's frame, and inlining three more
        // `WorkspaceEngine`s overflowed a 2 MiB thread stack in a debug build.
        Route::Findings => handle_findings(stream, request),
        Route::FindingStatus => handle_finding_status(stream, request),
        Route::FindingsRepair => handle_findings_repair(stream, request),
        Route::SessionDelete => handle_session_delete(stream, request),
        Route::Compact => handle_compact(stream, request),
        Route::OpenVscode => handle_open_vscode(stream, request),
        Route::RevealInFinder => handle_reveal_in_finder(stream, request),
        // Spec 58: takes `repo` only and opens the URL derived from `origin`.
        Route::OpenGithub => handle_open_github(stream, request),
        Route::RevealWebDiagnosticArtifact => {
            handle_reveal_web_diagnostic_artifact(stream, request)
        }
        Route::ContextFile => handle_context_file(stream, request),
        Route::OpenVscodeFile => handle_open_vscode_file(stream, request),
        Route::RenderMarkdown => handle_render_markdown(stream, request),
        Route::AskStream => handle_ask_stream(stream, request),
        Route::ResumeCommandStream => handle_resume_command_stream(stream, request),
        Route::ResumeFetchStream => handle_resume_fetch_stream(stream, request),
        Route::FetchDecision => handle_fetch_decision(stream, request),
        Route::VerifyStream => handle_verify_stream(stream, request),
        Route::ContinueRepairStream => handle_continue_repair_stream(stream, request),
        Route::ContinueTaskStream => handle_continue_task_stream(stream, request),
        Route::ContinueWorkerResultStream => handle_continue_worker_result_stream(stream, request),
        Route::ResumeChecksStream => handle_resume_checks_stream(stream, request),
        Route::ResumePlanStream => handle_resume_plan_stream(stream, request),
        Route::PlanWorkerPrompt => handle_plan_worker_prompt(stream, request),
        Route::PlanToWorker => handle_plan_to_worker(stream, request),
        Route::AnswerQuestionStream => handle_answer_question_stream(stream, request),
        Route::ResumeFallbackStream => handle_resume_fallback_stream(stream, request),
        Route::ResumeRoundBudgetStream => handle_resume_round_budget_stream(stream, request),
        Route::Ask => handle_ask(stream, request),
        Route::ProposeEdit => handle_propose_edit(stream, request),
        Route::ApplyPatch => handle_apply_patch(stream, request),
        Route::RollbackPatch => handle_rollback_patch(stream, request),
        Route::RejectPatchFiles => handle_reject_patch_files(stream, request),
        Route::RejectPatch => handle_reject_patch(stream, request),
        Route::ProposeCommand => handle_propose_command(stream, request),
        Route::RunCommand => handle_run_command(stream, request),
        Route::RejectCommand => handle_reject_command(stream, request),
        Route::RepositoryMap => handle_repository_map(stream, request),
        Route::RepositoryRoots => handle_repository_roots(stream, request),
        Route::ProjectChecks => handle_project_checks(stream, request),
        Route::ProjectChecksSet => handle_project_checks_set(stream, request),
        Route::CommandRules => handle_command_rules(stream, request),
        Route::CommandRulesSet => handle_command_rules_set(stream, request),
        // Spec 25 Task 10: symbol search, on-demand references, and the state
        // of each configured language server. Each is its own function for the
        // reason the findings routes are (spec 22 Task 10).
        Route::Symbols => handle_symbols(stream, request),
        Route::SymbolReferences => handle_symbol_references(stream, request),
        Route::LanguageServers => handle_language_servers(stream, request),
        Route::CommandProposal => handle_command_proposal(stream, request),
        Route::RepositoryConfigReview => handle_repository_config_review(stream, request),
        Route::RepositoryConfigAllowlist => handle_repository_config_allowlist(stream, request),
        // Spec 32 Task 9: the hooks the user can see, review, switch and test.
        // None of these takes a config path: user config is the only file they
        // can write, and its location comes from the data directory.
        Route::Hooks => handle_hooks(stream, request),
        Route::HookReview => handle_hook_review(stream, request),
        Route::HookEnabled => handle_hook_enabled(stream, request),
        Route::HookTest => handle_hook_test(stream, request),
        Route::ConfigSet => handle_config_set(stream, request),
        Route::PostConfigFile => handle_post_config_file(stream, request),
        Route::ModelKey => handle_model_key(stream, request),
        Route::ProviderKey => handle_provider_key(stream, request),
        Route::ModelKeyDelete => handle_model_key_delete(stream, request),
        Route::McpTest => handle_mcp_test(stream, request),
        Route::WorkersEnabled
        | Route::WorkerSourceStatus
        | Route::StartWorker
        | Route::ListWorkers
        | Route::WorkerProgress
        | Route::WorkerControl
        | Route::WorkerRestart
        | Route::WorkerRemoveWorktree => handle_worker_route(stream, request),
    }
}

/// The effective data directory, for callers outside this crate that need one
/// without building a whole engine — the Tauri app opening a terminal, say.
pub fn effective_data_dir() -> Result<PathBuf, String> {
    Config::load_for_repository(None)
        .map(|config| config.data_dir)
        .map_err(|error| error.to_string())
}

#[cfg(test)]
mod acceptance_tests;
#[cfg(test)]
mod responsive_tests;

#[cfg(test)]
mod tests;

/// The shell half of spec 56: the offer's JSON and the decision route.
#[cfg(test)]
mod provider_fallback_tests;

/// The shell half of spec 61's round-budget pause: the route that resumes it.
#[cfg(test)]
mod round_budget_tests;

#[cfg(test)]
mod commit_message_tests;

/// The display side of spec 55: the marker's data and the manual route.
#[cfg(test)]
mod conversation_compaction_tests;

/// Spec 29 Task 6: the proposal list and the two decisions, through the same
/// token-gated routes the UI calls. There is deliberately no accept-all route
/// and no route that takes a statement.
#[cfg(test)]
mod memory_route_tests;

/// Spec 36 Task 7: the branch routes through the same token-gated HTTP surface
/// the dialog uses. Each test drives the real routes, so a create proves the
/// ref appears and `HEAD` does not move, and a merge report proves the index,
/// `HEAD`, and status are unchanged byte for byte.
#[cfg(test)]
mod branch_route_tests;
