use crate::audit::AuditLog;
use crate::cancel::CancelToken;
use crate::checkpoint::{
    CheckpointConversation, CheckpointRequest, CheckpointStore, CommandCensus, PendingApproval,
};
use crate::command_policy::allow_always_eligible;
use crate::command_runner::CommandExecution;
use crate::config::{Config, McpTransport};
use crate::context_manager::ContextManager;
use crate::edit::{GeneratedEdit, PatchStore};
use crate::error::{ClientError, Result};
use crate::file_access::FileAccessController;
use crate::git_service::{GitService, GitStatus};
use crate::hash::create_id;
use crate::indexer::{ProjectIndexer, SearchResult};
use crate::mcp::{McpRuntime, McpServerRuntime, parse_namespaced_tool_name};
use crate::model::{ModelAdapter, ModelMessage, ModelRequest, ModelRun, ToolCall, ToolDefinition};
use crate::patch_engine::{PatchEngine, ProposedChange, ProposedFilePatch, ProposedPatch};
use crate::secret_scanner::SecretScanner;
use crate::session::{ChatMessage, Session, SessionStore, Task, TaskStatus};
use crate::validation::{
    CommandProposal, CommandStore, ValidationOrchestrator, command_approval_prompt,
};
use crate::vector_index::VectorIndexCache;
use crate::web_diagnostics::{
    WEB_SCENARIO_ACTIONS, WebDiagnosticCall, WebDiagnosticKind, WebDiagnosticReport,
    WebDiagnosticsRunnerHandle,
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

type McpTokenResolverFn = Arc<dyn Fn(&str) -> Option<String> + Send + Sync>;

/// Which stage of a turn is running. Drives the progress indicator, so the user
/// can tell a slow provider apart from a running tool apart from a hang.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PhaseKind {
    Context,
    Model,
    Tool,
    Finalizing,
}

impl PhaseKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Context => "context",
            Self::Model => "model",
            Self::Tool => "tool",
            Self::Finalizing => "finalizing",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TurnPhase {
    pub kind: PhaseKind,
    /// Human-readable detail, supplied here rather than in the UI so the
    /// frontend never needs to know tool names. Empty when the kind says it all.
    pub label: String,
    /// **1-based**, unlike the loop counter it comes from.
    pub round: u32,
    pub max_rounds: u32,
}

impl TurnPhase {
    fn new(kind: PhaseKind, label: impl Into<String>, round: u32, max_rounds: u32) -> Self {
        Self {
            kind,
            label: label.into(),
            round: round + 1,
            max_rounds,
        }
    }
}

/// Out-of-band progress about a turn, distinct from the answer text itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TurnProgress {
    /// The turn's session id, reported as soon as it exists so a client that
    /// stops before the turn finishes can still identify what it stopped.
    Session(String),
    Phase(TurnPhase),
}

/// The per-turn side channel: where answer tokens go, where progress goes, and
/// whether the user has asked to stop.
///
/// Grouped into one value because all three travel together through the whole
/// turn, and threading them as separate parameters would push
/// [`ChatOrchestrator::run_agentic_turn`]'s argument list further past the point
/// where clippy already objects.
pub struct TurnSink<'a> {
    pub on_token: &'a mut dyn FnMut(&str),
    pub on_progress: &'a mut dyn FnMut(TurnProgress),
    pub cancel: &'a CancelToken,
}

impl TurnSink<'_> {
    fn session(&mut self, session_id: &str) {
        (self.on_progress)(TurnProgress::Session(session_id.to_string()));
    }

    fn phase(&mut self, kind: PhaseKind, label: impl Into<String>, round: u32, max_rounds: u32) {
        (self.on_progress)(TurnProgress::Phase(TurnPhase::new(
            kind, label, round, max_rounds,
        )));
    }
}

/// Resolves an MCP server's `auth_token_env` reference (`keychain:<account>`
/// or an environment variable name) to the actual bearer token. The engine
/// never reads the keychain itself; the desktop shell injects a resolver that
/// does. Wrapped so [`ChatOrchestrator`] can stay `Debug`/`Clone`.
#[derive(Clone)]
pub struct McpTokenResolver(McpTokenResolverFn);

impl McpTokenResolver {
    pub fn new(resolver: impl Fn(&str) -> Option<String> + Send + Sync + 'static) -> Self {
        Self(Arc::new(resolver))
    }

    fn resolve(&self, reference: &str) -> Option<String> {
        (self.0)(reference)
    }
}

impl std::fmt::Debug for McpTokenResolver {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("McpTokenResolver(..)")
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentCommandProposal {
    pub id: String,
    pub command: String,
    pub prompt: String,
    pub risk: String,
    pub requires_approval: bool,
    pub blocked: bool,
    /// Whether the approval UI may offer "allow always" for this proposal.
    /// Always false for MCP tool calls, which aren't shell commands and so
    /// have no `command_allowlist` entry to write.
    pub allow_always: bool,
    /// Whether the UI may offer a session-scoped browser diagnostic allowance.
    /// This is intentionally separate from command allowlisting: it writes a
    /// session event, not repository or user config.
    pub allow_browser_diagnostics_for_session: bool,
}

/// A patch the model proposed mid-conversation via the `propose_patch` tool
/// call (as opposed to `EditOrchestrator::propose_edit`'s dedicated one-shot
/// flow). Carries the same `ProposedFilePatch` data the text-envelope path
/// produces, so the UI can render it with the exact same component either
/// way (`patch_id` + `summary` + `files` mirrors `/api/propose-edit`'s
/// response shape).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentPatchProposal {
    pub patch_id: String,
    pub summary: String,
    pub files: Vec<ProposedFilePatch>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChatTurnResult {
    pub session: Session,
    pub task: Task,
    pub model_run: ModelRun,
    pub context_files: Vec<String>,
    pub response: String,
    pub command_proposal: Option<AgentCommandProposal>,
    pub patch_proposal: Option<AgentPatchProposal>,
    /// The user stopped this turn. Distinct from a failure: `response` holds
    /// whatever had been generated, and it is persisted.
    pub cancelled: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct ChatTurnOptions {
    #[serde(default)]
    pub continue_debugging: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ResumeDecisionOptions {
    pub allow_browser_diagnostics_for_session: bool,
}

#[derive(Debug, Clone)]
pub struct ChatOrchestrator {
    config: Config,
    scanner: SecretScanner,
    audit_log: AuditLog,
    indexer: ProjectIndexer,
    context_manager: ContextManager,
    session_store: SessionStore,
    validation_orchestrator: ValidationOrchestrator,
    command_store: CommandStore,
    pending_commands: PendingCommandStore,
    file_access: FileAccessController,
    git: GitService,
    patch_engine: PatchEngine,
    patch_store: PatchStore,
    checkpoint_store: CheckpointStore,
    mcp_token_resolver: Option<McpTokenResolver>,
    web_diagnostics_runner: Option<WebDiagnosticsRunnerHandle>,
}

impl ChatOrchestrator {
    // Dependency-injection constructor: every argument is a collaborator the
    // orchestrator needs. Grouping them into a struct would only move the
    // same list one level out.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        config: Config,
        scanner: SecretScanner,
        audit_log: AuditLog,
        indexer: ProjectIndexer,
        context_manager: ContextManager,
        session_store: SessionStore,
        validation_orchestrator: ValidationOrchestrator,
        command_store: CommandStore,
        file_access: FileAccessController,
        git: GitService,
        patch_engine: PatchEngine,
        patch_store: PatchStore,
        checkpoint_store: CheckpointStore,
    ) -> Self {
        let pending_commands = PendingCommandStore::new(&config.data_dir);
        Self {
            config,
            scanner,
            audit_log,
            indexer,
            context_manager,
            session_store,
            validation_orchestrator,
            command_store,
            pending_commands,
            file_access,
            git,
            patch_engine,
            patch_store,
            checkpoint_store,
            mcp_token_resolver: None,
            web_diagnostics_runner: None,
        }
    }

    /// Injects the resolver used to turn MCP `auth_token_env` references into
    /// bearer tokens. The desktop shell calls this with a keychain-backed
    /// resolver; callers that don't wire MCP (or use only stdio servers) can
    /// leave it unset.
    pub fn set_mcp_token_resolver(&mut self, resolver: McpTokenResolver) {
        self.mcp_token_resolver = Some(resolver);
    }

    pub fn set_web_diagnostics_runner(&mut self, runner: WebDiagnosticsRunnerHandle) {
        self.web_diagnostics_runner = Some(runner);
    }

    fn default_tool_round_limit(&self) -> u32 {
        self.config
            .agent_max_tool_rounds
            .clamp(1, ABSOLUTE_TOOL_ROUND_CAP)
    }

    fn web_debug_tool_round_limit(&self) -> u32 {
        self.config
            .agent_web_debug_max_tool_rounds
            .clamp(self.default_tool_round_limit(), ABSOLUTE_TOOL_ROUND_CAP)
    }

    fn tool_round_limit(&self, web_debug_mode: bool, options: ChatTurnOptions) -> u32 {
        if options.continue_debugging {
            ABSOLUTE_TOOL_ROUND_CAP
        } else if web_debug_mode {
            self.web_debug_tool_round_limit()
        } else {
            self.default_tool_round_limit()
        }
    }

    /// Builds the per-turn MCP runtime from the active server config, resolving
    /// each HTTP server's auth token up front. Returns an inert runtime when no
    /// servers are active (the common case), so there's zero overhead and no
    /// behavior change for users who don't configure MCP.
    fn build_mcp_runtime(&self) -> McpRuntime {
        let servers: Vec<McpServerRuntime> = self
            .config
            .active_mcp_servers()
            .into_iter()
            .map(|server| {
                let auth_token = if server.transport == McpTransport::Http {
                    self.resolve_mcp_token(&server.auth_token_env)
                } else {
                    None
                };
                McpServerRuntime {
                    config: server.clone(),
                    auth_token,
                }
            })
            .collect();
        if servers.is_empty() {
            McpRuntime::disabled()
        } else {
            McpRuntime::new(servers, self.audit_log.clone())
        }
    }

    fn browser_diagnostic_mcp_server_ids(&self) -> Vec<String> {
        if self.web_diagnostics_runner.is_none() {
            return Vec::new();
        }
        self.config
            .active_mcp_servers()
            .into_iter()
            .filter(|server| looks_like_browser_diagnostics_mcp_server(server))
            .map(|server| server.id.clone())
            .collect()
    }

    fn resolve_mcp_token(&self, reference: &str) -> Option<String> {
        let reference = reference.trim();
        if reference.is_empty() {
            return None;
        }
        if let Some(resolver) = &self.mcp_token_resolver {
            return resolver.resolve(reference);
        }
        // Fallback with no injected resolver: only plain env vars can be read;
        // keychain references require the desktop shell's resolver.
        if reference.starts_with("keychain:") {
            return None;
        }
        std::env::var(reference).ok()
    }

    fn run_web_diagnostic_report(&self, call: &WebDiagnosticCall) -> Result<WebDiagnosticReport> {
        let Some(runner) = &self.web_diagnostics_runner else {
            return Err(ClientError::InvalidInput(
                "Browser diagnostics are not configured for this Damaian session.".to_string(),
            ));
        };
        match call.kind {
            WebDiagnosticKind::Inspect => runner.inspect(call),
            WebDiagnosticKind::Scenario => runner.run_scenario(call),
        }
    }

    fn run_web_diagnostic_call(&self, call: &WebDiagnosticCall) -> String {
        self.format_web_diagnostic_result(self.run_web_diagnostic_report(call))
    }

    fn format_web_diagnostic_result(&self, report: Result<WebDiagnosticReport>) -> String {
        match report {
            Ok(report) => {
                let mut text = self.scanner.redact(&report.text).text;
                if report.is_error && !text.starts_with("Browser diagnostic failed") {
                    text = format!("Browser diagnostic failed:\n{text}");
                }
                if !report.artifacts.is_empty() {
                    text.push_str("\n\nArtifacts:");
                    for artifact in report.artifacts {
                        text.push_str("\n- ");
                        text.push_str(&artifact.kind);
                        text.push_str(": ");
                        text.push_str(&self.scanner.redact(&artifact.path).text);
                        if let (Some(width), Some(height)) = (artifact.width, artifact.height) {
                            text.push_str(&format!(" ({width}x{height})"));
                        }
                    }
                }
                text
            }
            Err(error) => format!("Browser diagnostic failed: {error}"),
        }
    }

    /// Signature deliberately unchanged: this is the entry point for
    /// `damaian-cli` and the engine's own tests, neither of which has anything
    /// to cancel or anywhere to show progress. Only the desktop shell needs
    /// [`Self::ask_with_session`]'s full sink.
    pub fn ask(
        &self,
        repository_root: impl AsRef<Path>,
        prompt: &str,
        explicit_paths: &[String],
        model_adapter: &mut dyn ModelAdapter,
        on_token: &mut dyn FnMut(&str),
    ) -> Result<ChatTurnResult> {
        let never_cancelled = CancelToken::new();
        let mut discard_progress = |_event: TurnProgress| {};
        let mut sink = TurnSink {
            on_token,
            on_progress: &mut discard_progress,
            cancel: &never_cancelled,
        };
        self.ask_with_session(
            repository_root,
            prompt,
            explicit_paths,
            None,
            model_adapter,
            &mut sink,
        )
    }

    pub fn ask_with_session(
        &self,
        repository_root: impl AsRef<Path>,
        prompt: &str,
        explicit_paths: &[String],
        session_id: Option<&str>,
        model_adapter: &mut dyn ModelAdapter,
        sink: &mut TurnSink<'_>,
    ) -> Result<ChatTurnResult> {
        self.ask_with_session_with_options(
            repository_root,
            prompt,
            explicit_paths,
            session_id,
            model_adapter,
            sink,
            ChatTurnOptions::default(),
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn ask_with_session_with_options(
        &self,
        repository_root: impl AsRef<Path>,
        prompt: &str,
        explicit_paths: &[String],
        session_id: Option<&str>,
        model_adapter: &mut dyn ModelAdapter,
        sink: &mut TurnSink<'_>,
        options: ChatTurnOptions,
    ) -> Result<ChatTurnResult> {
        let repository_root = repository_root.as_ref();
        let index = crate::index_cache::IndexCache::get_or_build(&self.indexer, repository_root)?;
        let (session, prior_messages) = if let Some(session_id) = session_id {
            let Some(session) = self.session_store.read_session(session_id)? else {
                return Err(ClientError::InvalidInput(format!(
                    "Unknown session: {session_id}"
                )));
            };
            if session.repository_id != index.repository_id {
                return Err(ClientError::AccessDenied(
                    "Session belongs to a different repository".to_string(),
                ));
            }
            let messages = self.session_store.read_messages(&session.id)?;
            (session, messages)
        } else {
            (
                self.session_store
                    .create_session(&index.repository_id, &session_title(prompt))?,
                Vec::new(),
            )
        };
        // Captured before the turn's own events, so rewinding this checkpoint
        // takes the user's prompt with it rather than leaving a question the
        // conversation no longer answers.
        let position = self.session_store.latest_event_seq(&session.id)?;
        let mut task = self.session_store.create_task(
            &session.id,
            prompt,
            &self.config.model_provider,
            &self.config.model_name,
        )?;
        task = self
            .session_store
            .update_task_status(&task, TaskStatus::Running, None)?;
        let user_message =
            self.session_store
                .append_message(&session.id, Some(&task.id), "user", prompt)?;
        self.take_turn_checkpoint(
            repository_root,
            &session,
            &task,
            prompt,
            position,
            &user_message.id,
        );

        // Before context assembly, which can take a while on a large repository:
        // a client that stops during it must still know which session it stopped.
        sink.session(&session.id);
        sink.phase(
            PhaseKind::Context,
            "",
            0,
            self.tool_round_limit(prompt_enters_web_debug_mode(prompt), options),
        );

        let context = self.context_manager.build_context(
            repository_root,
            &index.repository_id,
            &task.id,
            prompt,
            Some(&index),
            explicit_paths,
            self.config.context_token_budget(),
        );
        let model_prompt = build_model_prompt(prompt, &context.items, &prior_messages, None);
        let messages = vec![
            ModelMessage::system(system_prompt()),
            ModelMessage::user(model_prompt),
        ];

        self.run_agentic_turn(
            repository_root,
            session,
            task,
            context.files,
            messages,
            0,
            model_adapter,
            sink,
            options,
        )
    }

    /// Continues a chat turn that stopped to ask the user whether a proposed
    /// command may run. Executes (or rejects) the command, feeds the result
    /// back to the model, and lets the agentic loop keep going from there —
    /// previously approving a risky command just ran it in isolation and the
    /// model never got to use the result to answer the user's question.
    pub fn resume_after_command_decision(
        &self,
        proposal_id: &str,
        approved: bool,
        approved_by: &str,
        model_adapter: &mut dyn ModelAdapter,
        sink: &mut TurnSink<'_>,
    ) -> Result<ChatTurnResult> {
        self.resume_after_command_decision_with_options(
            proposal_id,
            approved,
            approved_by,
            model_adapter,
            sink,
            ResumeDecisionOptions::default(),
        )
    }

    pub fn resume_after_command_decision_with_options(
        &self,
        proposal_id: &str,
        approved: bool,
        approved_by: &str,
        model_adapter: &mut dyn ModelAdapter,
        sink: &mut TurnSink<'_>,
        decision_options: ResumeDecisionOptions,
    ) -> Result<ChatTurnResult> {
        let pending = self.pending_commands.take(proposal_id)?;
        let repository_root = PathBuf::from(&pending.repository_root);
        let mut messages = pending.messages;

        // Three kinds of paused action resume through here: a browser
        // diagnostic, an MCP tool call, or the original shell-command path.
        let (assistant_summary, tool_result_content) = if let Some(web_call) =
            &pending.web_diagnostic_call
        {
            let call = web_call.call.clone();
            let summary = web_diagnostic_summary(&call);
            let decision = if approved && decision_options.allow_browser_diagnostics_for_session {
                self.session_store
                    .allow_browser_diagnostics_for_session(&pending.session.id, approved_by)?;
                "approved_for_session"
            } else if approved {
                "approved_once"
            } else {
                "denied"
            };
            self.audit_log.record(
                "browser_diagnostic_approval_decision",
                &[
                    ("actor", approved_by.to_string()),
                    ("sessionId", pending.session.id.clone()),
                    ("taskId", pending.task.id.clone()),
                    ("proposalId", proposal_id.to_string()),
                    ("decision", decision.to_string()),
                    ("tool", call.name().to_string()),
                    ("url", call.url.clone()),
                ],
            )?;
            let content = if approved {
                self.run_web_diagnostic_call(&call)
            } else {
                format!(
                    "The user declined to run `{}` against `{}`. Do not request the same browser diagnostic again; answer using what you already know, noting the limitation if it matters.",
                    call.name(),
                    call.url
                )
            };
            (summary, content)
        } else if let Some(mcp_call) = &pending.mcp_call {
            let summary = mcp_call_summary(&mcp_call.server_id, &mcp_call.tool_name);
            let content = if approved {
                let mut mcp = self.build_mcp_runtime();
                match mcp.call_tool(
                    &mcp_call.server_id,
                    &mcp_call.tool_name,
                    &mcp_call.arguments_json,
                ) {
                    Ok(result) => {
                        let text = self.scanner.redact(&result.text).text;
                        if result.is_error {
                            format!("MCP tool reported an error:\n{text}")
                        } else {
                            text
                        }
                    }
                    Err(error) => format!("MCP tool call failed: {error}"),
                }
            } else {
                format!(
                    "The user declined to run the MCP tool `{}` on server `{}`. Do not request it again; answer using what you already know, noting the limitation if it matters.",
                    mcp_call.tool_name, mcp_call.server_id
                )
            };
            (summary, content)
        } else {
            let proposal = self.command_store.load_proposal(proposal_id)?;
            let command_request = CommandRequest {
                command: proposal.command.clone(),
                reason: proposal.reason.clone(),
            };
            let content = if approved {
                // The census has to be taken before the command runs: once it
                // has, the pre-command bytes are gone.
                let census = self
                    .checkpoint_store
                    .begin_command_census(&repository_root)
                    .unwrap_or_else(|_| CommandCensus::unavailable());
                let record =
                    self.validation_orchestrator
                        .run_proposal(proposal_id, true, approved_by)?;
                self.record_command_effects(
                    &repository_root,
                    &pending.session,
                    &pending.task,
                    &census,
                );
                sandbox_command_context(&record.execution)
            } else {
                self.validation_orchestrator
                    .reject_proposal(proposal_id, approved_by)?;
                format!(
                    "The user declined to run `{}`. Do not request it again; answer using what you already know, noting the limitation if it matters.",
                    command_request.command
                )
            };
            (tool_call_summary(&command_request), content)
        };

        self.session_store.append_message(
            &pending.session.id,
            Some(&pending.task.id),
            "assistant",
            &assistant_summary,
        )?;
        self.session_store.append_message(
            &pending.session.id,
            Some(&pending.task.id),
            "tool",
            &tool_result_content,
        )?;

        if let Some(call) = &pending.matched_tool_call {
            messages.push(ModelMessage::assistant_with_tool_calls(
                pending.last_content.clone(),
                vec![call.clone()],
                pending.reasoning_content.clone(),
            ));
            messages.push(ModelMessage::tool(call.id.clone(), tool_result_content));
        } else {
            messages.push(ModelMessage::assistant(pending.last_content.clone()));
            messages.push(ModelMessage::user(format!(
                "Command result:\n{tool_result_content}"
            )));
        }

        // The pause is over either way, so the checkpoint stops claiming one.
        self.note_pending_approvals(&pending.session, &pending.task, Vec::new());
        let task =
            self.session_store
                .update_task_status(&pending.task, TaskStatus::Running, None)?;

        self.run_agentic_turn(
            &repository_root,
            pending.session,
            task,
            pending.context_files,
            messages,
            pending.round + 1,
            model_adapter,
            sink,
            pending.turn_options,
        )
    }

    /// Takes the checkpoint the turn can be rewound to. Best-effort: a store
    /// that cannot be written is recorded in the audit log and the turn still
    /// runs, because refusing to answer at all would be a worse failure than
    /// losing the ability to rewind one turn.
    fn take_turn_checkpoint(
        &self,
        repository_root: &Path,
        session: &Session,
        task: &Task,
        prompt: &str,
        position: u64,
        user_message_id: &str,
    ) {
        let summary = checkpoint_summary(prompt);
        let created = self.checkpoint_store.create_checkpoint(
            repository_root,
            CheckpointRequest {
                session_id: &session.id,
                task_id: Some(&task.id),
                user_message_id: Some(user_message_id),
                summary: &summary,
                conversation: CheckpointConversation {
                    last_event_seq: position,
                    task_status: TaskStatus::Running.as_str().to_string(),
                },
                pending_approvals: Vec::new(),
                // Nothing has happened yet: paths arrive as the turn accepts a
                // patch or runs an approved command.
                paths: Vec::new(),
            },
        );
        match created {
            Ok(_) => {
                // Retention runs here rather than on a timer: it is cheap
                // unless something actually expired, and a turn is the moment
                // a new checkpoint was just added.
                let _ = self
                    .checkpoint_store
                    .cleanup(&session.repository_id, Some(&session.id));
            }
            Err(error) => {
                let _ = self.audit_log.record(
                    "checkpoint_creation_failed",
                    &[
                        ("actor", "system".to_string()),
                        ("sessionId", session.id.clone()),
                        ("taskId", task.id.clone()),
                        ("error", error.to_string()),
                    ],
                );
            }
        }
    }

    /// Records on the turn's checkpoint what it is waiting on, or clears it
    /// once the decision is in. Best-effort, like the rest of the checkpoint
    /// path: a missing note is worth less than a failed turn.
    fn note_pending_approvals(
        &self,
        session: &Session,
        task: &Task,
        pending_approvals: Vec<PendingApproval>,
    ) {
        let Ok(Some(manifest)) = self
            .checkpoint_store
            .read_checkpoint_for_task(&session.repository_id, &task.id)
        else {
            return;
        };
        let _ = self
            .checkpoint_store
            .set_pending_approvals(&manifest, pending_approvals);
    }

    /// Adds what an approved command changed to the turn's checkpoint.
    /// Best-effort, like the rest of the checkpoint path.
    fn record_command_effects(
        &self,
        repository_root: &Path,
        session: &Session,
        task: &Task,
        census: &CommandCensus,
    ) {
        let Ok(Some(manifest)) = self
            .checkpoint_store
            .read_checkpoint_for_task(&session.repository_id, &task.id)
        else {
            return;
        };
        let _ = self
            .checkpoint_store
            .record_command_effects(repository_root, &manifest, census);
    }

    /// Records what the turn left on disk, so a later rewind can tell the
    /// agent's own changes from somebody else's. Best-effort for the same
    /// reason as [`Self::take_turn_checkpoint`].
    fn seal_turn_checkpoint(&self, repository_root: &Path, session: &Session, task: &Task) {
        let Ok(Some(manifest)) = self
            .checkpoint_store
            .read_checkpoint_for_task(&session.repository_id, &task.id)
        else {
            return;
        };
        let _ = self
            .checkpoint_store
            .seal_checkpoint(repository_root, &manifest);
    }

    /// Whether `proposal_id` was raised by a chat turn (and so should be
    /// resumed via [`Self::resume_after_command_decision`]) as opposed to a
    /// standalone command proposal from outside the chat flow.
    pub fn has_pending_chat_command(&self, proposal_id: &str) -> bool {
        self.pending_commands.has(proposal_id)
    }

    /// Runs the model in a loop, letting it request tools across multiple
    /// rounds (e.g. `read_git_status` followed by `read_git_diff`) instead of
    /// stopping after one. Configured round budgets keep a provider that never
    /// stops requesting tools from running forever; the final round always
    /// drops `tools`, forcing a plain answer. If a tool needs human approval,
    /// the in-flight conversation state is persisted so the turn can be
    /// resumed later via [`Self::resume_after_command_decision`].
    // Threads the full per-turn state (session, task, messages, round) plus the
    // model adapter and token sink through one recursive-ish loop.
    #[allow(clippy::too_many_arguments)]
    fn run_agentic_turn(
        &self,
        repository_root: &Path,
        session: Session,
        mut task: Task,
        context_files: Vec<String>,
        mut messages: Vec<ModelMessage>,
        mut round: u32,
        model_adapter: &mut dyn ModelAdapter,
        sink: &mut TurnSink<'_>,
        turn_options: ChatTurnOptions,
    ) -> Result<ChatTurnResult> {
        // Per-turn MCP runtime: connects lazily, caches tool lists and
        // connections for this turn, and tears everything down on drop.
        let mut mcp = self.build_mcp_runtime();
        let browser_mcp_server_ids = self.browser_diagnostic_mcp_server_ids();
        let native_tools = self.config.supports_native_tools().then(|| {
            let mut tools = vec![
                run_command_tool_definition(),
                propose_patch_tool_definition(),
                read_file_tool_definition(),
                search_codebase_tool_definition(),
                read_git_status_tool_definition(),
                read_git_diff_tool_definition(),
            ];
            if self.web_diagnostics_runner.is_some() {
                tools.push(inspect_web_page_tool_definition());
                tools.push(run_web_scenario_tool_definition());
            }
            // Best-effort: discovered MCP tools are namespaced (mcp__<server>__<tool>)
            // and appended; a server that fails to connect is simply skipped.
            tools.extend(mcp.tool_definitions().into_iter().filter(|tool| {
                parse_namespaced_tool_name(&tool.name)
                    .map(|(server_id, _)| !browser_mcp_server_ids.contains(&server_id))
                    .unwrap_or(true)
            }));
            tools
        });

        // Whatever the model has produced so far, carried across rounds so a
        // stop between them still has an answer to preserve.
        let mut partial_response = String::new();
        let mut web_debug_mode =
            turn_options.continue_debugging || prompt_enters_web_debug_mode(&task.user_prompt);
        let mut failed_browser_calls = HashMap::new();

        let (final_run, response, command_proposal, patch_proposal, tool_budget_exhausted) = loop {
            // Checked before each round rather than only mid-stream: stopping
            // here is what saves a whole model call, and it is the only point
            // that catches a stop arriving during context assembly or a tool.
            if sink.cancel.is_cancelled() {
                return self.finish_cancelled_turn(
                    repository_root,
                    session,
                    task,
                    context_files,
                    &partial_response,
                    ModelRun::cancelled_before_start(
                        &self.config.model_provider,
                        &self.config.model_name,
                    ),
                );
            }

            let max_rounds = self.tool_round_limit(web_debug_mode, turn_options);
            let force_final = round >= max_rounds;
            let tools = if force_final {
                None
            } else {
                native_tools.clone()
            };
            let request = ModelRequest {
                provider: self.config.model_provider.clone(),
                model: self.config.model_name.clone(),
                messages: messages.clone(),
                temperature: Some("0".to_string()),
                reasoning_level: Some(self.config.model_reasoning_level.clone()),
                stream: true,
                tools,
                max_tokens: self.config.max_output_tokens(),
            };

            let token_estimate: usize = messages
                .iter()
                .map(|message| message.content.len())
                .sum::<usize>()
                .div_ceil(4);
            self.audit_log.record(
                "model_request_prepared",
                &[
                    ("actor", "system".to_string()),
                    ("sessionId", session.id.clone()),
                    ("taskId", task.id.clone()),
                    ("repositoryId", session.repository_id.clone()),
                    ("contextFiles", context_files.join(",")),
                    ("tokenEstimate", token_estimate.to_string()),
                    ("toolRound", round.to_string()),
                    ("toolRoundLimit", max_rounds.to_string()),
                ],
            )?;

            sink.phase(PhaseKind::Model, "", round, max_rounds);
            let model_run =
                match model_adapter.stream_response(&request, sink.cancel, &mut *sink.on_token) {
                    Ok(model_run) => model_run,
                    // A stop is not a failure. The transport raises `Cancelled`
                    // when it killed the request mid-flight, and it must not be
                    // recorded as a provider error.
                    Err(ClientError::Cancelled) => {
                        return self.finish_cancelled_turn(
                            repository_root,
                            session,
                            task,
                            context_files,
                            &partial_response,
                            ModelRun::cancelled_before_start(
                                &self.config.model_provider,
                                &self.config.model_name,
                            ),
                        );
                    }
                    Err(error) => {
                        let _ = self.session_store.update_task_status(
                            &task,
                            TaskStatus::Failed,
                            Some(&error.to_string()),
                        );
                        return Err(error);
                    }
                };
            let redacted = self.scanner.redact(&model_run.content).text;

            // An adapter that streamed part of an answer before noticing the
            // stop returns `Ok` with partial content, so the flag has to be
            // re-checked here as well as at the top of the loop.
            if sink.cancel.is_cancelled() {
                let partial = model_run.content.clone();
                return self.finish_cancelled_turn(
                    repository_root,
                    session,
                    task,
                    context_files,
                    &partial,
                    model_run,
                );
            }
            partial_response = redacted.clone();

            if force_final {
                if model_output_requests_tool(&model_run, &redacted) {
                    let response = tool_budget_exhausted_response(max_rounds);
                    let mut exhausted_run = model_run;
                    exhausted_run.content = response.clone();
                    break (exhausted_run, response, None, None, true);
                }
                break (model_run, redacted, None, None, false);
            }

            let (matched_tool_call, decoded_tool_action, tool_decode_error) =
                first_decodable_tool_action(&model_run.tool_calls);
            let tool_action = decoded_tool_action
                .or_else(|| parse_command_request(&redacted).map(ToolAction::Command));

            let Some(tool_action) = tool_action else {
                // The model asked for a tool but the request couldn't be
                // decoded. The usual cause is the provider stopping at its
                // output-token ceiling and cutting the `arguments` JSON off
                // mid-string. This used to end the turn silently: the user saw
                // a lead-in like "Let me create all the necessary files:", no
                // patch, and a task marked complete. Feed the failure back
                // instead so the model can retry within the remaining rounds —
                // the same recovery the restricted-path patch arm uses.
                let Some(undecodable) = model_run.tool_calls.first().cloned() else {
                    break (model_run, redacted, None, None, false);
                };
                let note = tool_decode_error.unwrap_or_else(|| {
                    undecodable_tool_call_note(&undecodable, model_run.truncated)
                });
                let summary = format!(
                    "Attempted to call `{}`, but the request could not be decoded.",
                    undecodable.name
                );
                self.session_store.append_message(
                    &session.id,
                    Some(&task.id),
                    "assistant",
                    &summary,
                )?;
                self.session_store
                    .append_message(&session.id, Some(&task.id), "tool", &note)?;
                // Deliberately not echoed back as an `assistant` tool_calls /
                // `tool` pair: the malformed arguments would have to be
                // replayed verbatim, and providers reject a tool result whose
                // call didn't parse. Plain text carries the correction safely.
                messages.push(ModelMessage::assistant(if redacted.trim().is_empty() {
                    summary
                } else {
                    redacted.clone()
                }));
                messages.push(ModelMessage::user(note));
                round += 1;
                continue;
            };

            if matches!(tool_action, ToolAction::WebDiagnostic(_)) {
                web_debug_mode = true;
            }
            let max_rounds = self.tool_round_limit(web_debug_mode, turn_options);
            sink.phase(
                PhaseKind::Tool,
                tool_action_label(&tool_action),
                round,
                max_rounds,
            );

            // Each non-terminal arm below produces the (assistant summary,
            // tool result) pair to persist and feed back to the model.
            // Terminal outcomes (a command needing approval, or a patch
            // ready for review) `break` the loop directly instead, since
            // both always require the human before anything continues.
            let (assistant_summary, tool_result_text) = match tool_action {
                ToolAction::Command(command_request) => {
                    let proposal = self.validation_orchestrator.propose_command(
                        repository_root,
                        &command_request.command,
                        &command_request.reason,
                    )?;

                    if proposal.requires_approval || proposal.blocked {
                        let response = command_proposal_response(&proposal);
                        self.pending_commands.save(&PendingChatTurn {
                            proposal_id: proposal.id.clone(),
                            session: session.clone(),
                            task: task.clone(),
                            repository_root: repository_root.to_string_lossy().to_string(),
                            context_files: context_files.clone(),
                            round,
                            messages: messages.clone(),
                            matched_tool_call: matched_tool_call.clone(),
                            last_content: redacted.clone(),
                            turn_options,
                            reasoning_content: model_run.reasoning_content.clone(),
                            mcp_call: None,
                            web_diagnostic_call: None,
                        })?;
                        self.note_pending_approvals(
                            &session,
                            &task,
                            vec![PendingApproval {
                                kind: "command".to_string(),
                                proposal_id: proposal.id.clone(),
                            }],
                        );
                        let mut proposal_run = model_run;
                        proposal_run.content = response.clone();
                        break (
                            proposal_run,
                            response,
                            Some(agent_command_proposal(&self.config, &proposal)),
                            None,
                            false,
                        );
                    }

                    let record = self.validation_orchestrator.run_proposal(
                        &proposal.id,
                        false,
                        "sandbox",
                    )?;
                    let command_context = sandbox_command_context(&record.execution);
                    (tool_call_summary(&command_request), command_context)
                }
                ToolAction::ProposePatch(generated_edit) => {
                    match self.patch_engine.create_patch(
                        repository_root,
                        &generated_edit.changes,
                        Some(&task.id),
                        &generated_edit.summary,
                    ) {
                        Ok(patch) => {
                            self.patch_store.save(&patch)?;
                            let response = patch_proposal_response(&patch);
                            let proposal = agent_patch_proposal(&patch);
                            let mut proposal_run = model_run;
                            proposal_run.content = response.clone();
                            break (proposal_run, response, None, Some(proposal), false);
                        }
                        // Fed back as a tool result rather than aborting the
                        // turn, so the model can see why (e.g. a restricted
                        // or out-of-repo path) and correct itself within the
                        // remaining rounds instead of the turn just failing.
                        Err(error) => (
                            format!("Attempted to propose a patch: {}", generated_edit.summary),
                            format!("Cannot propose that patch: {error}"),
                        ),
                    }
                }
                ToolAction::ReadFile(path) => {
                    let content = match self.file_access.read_file(
                        repository_root,
                        &path,
                        Some(&task.id),
                        Some(&session.repository_id),
                        false,
                        false,
                    ) {
                        Ok(file_read) => {
                            format!("Content of {}:\n{}", file_read.path, file_read.content)
                        }
                        Err(error) => format!("Cannot read {path}: {error}"),
                    };
                    (format!("Read `{path}`"), content)
                }
                ToolAction::SearchCodebase {
                    query,
                    semantic,
                    limit,
                } => {
                    let index = crate::index_cache::IndexCache::get_or_build(
                        &self.indexer,
                        repository_root,
                    )?;
                    let results = if semantic {
                        if self.config.enable_semantic_search {
                            VectorIndexCache::semantic_search(
                                &self.config.data_dir,
                                &index,
                                &query,
                                limit,
                            )
                        } else {
                            index.semantic_search(&query, limit)
                        }
                    } else {
                        index.keyword_search(&query, limit)
                    };
                    (
                        format!("Searched codebase for \"{query}\""),
                        format_search_results(&results),
                    )
                }
                ToolAction::ReadGitStatus => {
                    let content = match self.git.status(repository_root) {
                        Ok(status) => format_git_status(&status),
                        Err(error) => format!("Cannot read git status: {error}"),
                    };
                    ("Checked git status".to_string(), content)
                }
                ToolAction::ReadGitDiff { staged } => {
                    let content = match self.git.diff(repository_root, staged) {
                        Ok(diff) if diff.trim().is_empty() => "No differences.".to_string(),
                        Ok(diff) => diff,
                        Err(error) => format!("Cannot read git diff: {error}"),
                    };
                    (
                        format!("Read git diff{}", if staged { " (staged)" } else { "" }),
                        content,
                    )
                }
                ToolAction::WebDiagnostic(call) => {
                    let call = call.with_context(&session.id, &task.id);
                    let session_approved = if call.is_low_risk() {
                        false
                    } else {
                        self.session_store
                            .browser_diagnostics_allowed_for_session(&session.id)?
                    };
                    if !call.is_low_risk() && !session_approved {
                        let proposal_id = create_id("webdiag");
                        let proposal = web_diagnostic_approval_proposal(&proposal_id, &call);
                        let response = proposal.prompt.clone();
                        self.pending_commands.save(&PendingChatTurn {
                            proposal_id,
                            session: session.clone(),
                            task: task.clone(),
                            repository_root: repository_root.to_string_lossy().to_string(),
                            context_files: context_files.clone(),
                            round,
                            messages: messages.clone(),
                            matched_tool_call: matched_tool_call.clone(),
                            last_content: redacted.clone(),
                            turn_options,
                            reasoning_content: model_run.reasoning_content.clone(),
                            mcp_call: None,
                            web_diagnostic_call: Some(PendingWebDiagnosticCall { call }),
                        })?;
                        self.note_pending_approvals(
                            &session,
                            &task,
                            vec![PendingApproval {
                                kind: "browser_diagnostic".to_string(),
                                proposal_id: proposal.id.clone(),
                            }],
                        );
                        let mut proposal_run = model_run;
                        proposal_run.content = response.clone();
                        break (proposal_run, response, Some(proposal), None, false);
                    }
                    if session_approved {
                        self.audit_log.record(
                            "browser_diagnostic_session_approval_used",
                            &[
                                ("actor", "system".to_string()),
                                ("sessionId", session.id.clone()),
                                ("taskId", task.id.clone()),
                                ("tool", call.name().to_string()),
                                ("url", call.url.clone()),
                            ],
                        )?;
                    }

                    let signature = web_diagnostic_signature(&call);
                    let retry_limit = self.config.agent_tool_retry_limit;
                    let content = if failed_browser_calls
                        .get(&signature)
                        .copied()
                        .unwrap_or_default()
                        >= retry_limit
                    {
                        browser_retry_limit_note(retry_limit)
                    } else {
                        let report = self.run_web_diagnostic_report(&call);
                        let content = self.format_web_diagnostic_result(report);
                        if browser_tool_result_failed(&content) {
                            *failed_browser_calls.entry(signature).or_insert(0) += 1;
                        }
                        content
                    };
                    (web_diagnostic_summary(&call), content)
                }
                ToolAction::McpCall {
                    server_id,
                    tool_name,
                    arguments_json,
                } => {
                    // MCP tools reach an external service and can have side
                    // effects, so unless the server is marked no-approval we
                    // pause the turn exactly like a command needing approval:
                    // persist state keyed by a fresh proposal id and hand the
                    // user a proposal to accept or decline.
                    if mcp.requires_approval(&server_id) {
                        let proposal_id = create_id("mcp");
                        let proposal = mcp_approval_proposal(
                            &proposal_id,
                            &server_id,
                            &tool_name,
                            &arguments_json,
                        );
                        let response = proposal.prompt.clone();
                        self.pending_commands.save(&PendingChatTurn {
                            proposal_id,
                            session: session.clone(),
                            task: task.clone(),
                            repository_root: repository_root.to_string_lossy().to_string(),
                            context_files: context_files.clone(),
                            round,
                            messages: messages.clone(),
                            matched_tool_call: matched_tool_call.clone(),
                            last_content: redacted.clone(),
                            turn_options,
                            reasoning_content: model_run.reasoning_content.clone(),
                            mcp_call: Some(PendingMcpCall {
                                server_id,
                                tool_name,
                                arguments_json,
                            }),
                            web_diagnostic_call: None,
                        })?;
                        self.note_pending_approvals(
                            &session,
                            &task,
                            vec![PendingApproval {
                                kind: "mcp_tool".to_string(),
                                proposal_id: proposal.id.clone(),
                            }],
                        );
                        let mut proposal_run = model_run;
                        proposal_run.content = response.clone();
                        break (proposal_run, response, Some(proposal), None, false);
                    }

                    // No approval required: run it now and feed the result back.
                    let summary = mcp_call_summary(&server_id, &tool_name);
                    let content = match mcp.call_tool(&server_id, &tool_name, &arguments_json) {
                        Ok(result) => {
                            let text = self.scanner.redact(&result.text).text;
                            if result.is_error {
                                format!("MCP tool reported an error:\n{text}")
                            } else {
                                text
                            }
                        }
                        Err(error) => format!("MCP tool call failed: {error}"),
                    };
                    (summary, content)
                }
            };

            // Persist the tool call and its result so later turns in this
            // session can still see it (previously this context was
            // discarded once the turn finished).
            self.session_store.append_message(
                &session.id,
                Some(&task.id),
                "assistant",
                &assistant_summary,
            )?;
            self.session_store.append_message(
                &session.id,
                Some(&task.id),
                "tool",
                &tool_result_text,
            )?;

            if let Some(call) = &matched_tool_call {
                messages.push(ModelMessage::assistant_with_tool_calls(
                    redacted.clone(),
                    vec![call.clone()],
                    model_run.reasoning_content.clone(),
                ));
                messages.push(ModelMessage::tool(call.id.clone(), tool_result_text));
            } else {
                // Only reachable for `ToolAction::Command` via the
                // `DAMAIAN_COMMAND_V1` text-envelope fallback — every other
                // action only exists as a native tool call.
                messages.push(ModelMessage::assistant(redacted.clone()));
                messages.push(ModelMessage::user(format!(
                    "Command result:\n{tool_result_text}"
                )));
            }

            round += 1;
        };

        self.session_store
            .append_message(&session.id, Some(&task.id), "assistant", &response)?;
        let final_status = if command_proposal.is_some() || patch_proposal.is_some() {
            TaskStatus::WaitingForApproval
        } else if tool_budget_exhausted {
            TaskStatus::ToolBudgetExhausted
        } else {
            TaskStatus::Complete
        };
        task = self
            .session_store
            .update_task_status(&task, final_status, None)?;
        self.audit_log.record(
            "model_response_completed",
            &[
                ("actor", "model".to_string()),
                ("sessionId", session.id.clone()),
                ("taskId", task.id.clone()),
                ("provider", final_run.provider.clone()),
                ("model", final_run.model.clone()),
                (
                    "status",
                    if command_proposal.is_some() {
                        "command_approval_required".to_string()
                    } else if patch_proposal.is_some() {
                        "patch_proposal_ready".to_string()
                    } else if tool_budget_exhausted {
                        "tool_budget_exhausted".to_string()
                    } else if round > 0 {
                        "complete_with_sandbox_command".to_string()
                    } else {
                        "complete".to_string()
                    },
                ),
            ],
        )?;
        // A turn waiting on a command decision is not over: it resumes through
        // `resume_after_command_decision` and seals then. Anything else has
        // left the repository in the state a rewind must compare against.
        if command_proposal.is_none() {
            self.seal_turn_checkpoint(repository_root, &session, &task);
        }
        Ok(ChatTurnResult {
            session,
            task,
            model_run: final_run,
            context_files,
            response,
            command_proposal,
            patch_proposal,
            cancelled: false,
        })
    }

    /// Closes out a turn the user stopped.
    ///
    /// The single place `ClientError::Cancelled` is turned back into a result:
    /// it persists whatever was generated, marks the task terminal so no
    /// `Running` record is left wedged, and reports the stop as an outcome
    /// rather than a failure — a stop and a provider error need to stay
    /// distinguishable in the badge and in the task history alike.
    #[allow(clippy::too_many_arguments)]
    fn finish_cancelled_turn(
        &self,
        repository_root: &Path,
        session: Session,
        task: Task,
        context_files: Vec<String>,
        partial: &str,
        model_run: ModelRun,
    ) -> Result<ChatTurnResult> {
        // Through the same scanner as a completed turn: stopping does not
        // suspend the redaction guarantee.
        let response = self.scanner.redact(partial).text;
        if !response.is_empty() {
            self.session_store.append_message(
                &session.id,
                Some(&task.id),
                "assistant",
                &response,
            )?;
        }
        let task = self
            .session_store
            .update_task_status(&task, TaskStatus::Cancelled, None)?;
        self.audit_log.record(
            "chat_turn_cancelled",
            &[
                ("actor", "user".to_string()),
                ("sessionId", session.id.clone()),
                ("taskId", task.id.clone()),
                ("partialLength", response.len().to_string()),
            ],
        )?;
        // A stopped turn may still have applied a patch or run a command
        // before it stopped, so what it left behind is what a rewind has to
        // compare against.
        self.seal_turn_checkpoint(repository_root, &session, &task);
        Ok(ChatTurnResult {
            session,
            task,
            model_run,
            context_files,
            response,
            command_proposal: None,
            patch_proposal: None,
            cancelled: true,
        })
    }
}

/// Conversation state saved when a chat turn pauses on a command that needs
/// human approval, so [`ChatOrchestrator::resume_after_command_decision`]
/// can pick the turn back up once the user decides.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct PendingChatTurn {
    proposal_id: String,
    session: Session,
    task: Task,
    repository_root: String,
    context_files: Vec<String>,
    round: u32,
    messages: Vec<ModelMessage>,
    matched_tool_call: Option<ToolCall>,
    last_content: String,
    #[serde(default)]
    turn_options: ChatTurnOptions,
    /// Thinking-mode reasoning behind `matched_tool_call`, which must be
    /// replayed with it when the turn resumes — a pause for human approval
    /// must not lose it, or the resumed request is rejected outright.
    /// `#[serde(default)]` keeps pending turns written before this field
    /// existed loadable.
    #[serde(default)]
    reasoning_content: Option<String>,
    /// Present when the paused action is an MCP tool call rather than a shell
    /// command; carries everything needed to execute it on resume. `#[serde(default)]`
    /// keeps older on-disk pending turns (which predate MCP) loadable.
    #[serde(default)]
    mcp_call: Option<PendingMcpCall>,
    #[serde(default)]
    web_diagnostic_call: Option<PendingWebDiagnosticCall>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct PendingMcpCall {
    server_id: String,
    tool_name: String,
    arguments_json: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct PendingWebDiagnosticCall {
    call: WebDiagnosticCall,
}

#[derive(Debug, Clone)]
struct PendingCommandStore {
    data_dir: PathBuf,
}

impl PendingCommandStore {
    fn new(data_dir: impl AsRef<Path>) -> Self {
        Self {
            data_dir: data_dir.as_ref().to_path_buf(),
        }
    }

    fn path_for(&self, proposal_id: &str) -> PathBuf {
        self.data_dir
            .join("chat")
            .join("pending")
            .join(format!("{proposal_id}.json"))
    }

    fn has(&self, proposal_id: &str) -> bool {
        self.path_for(proposal_id).exists()
    }

    fn save(&self, pending: &PendingChatTurn) -> Result<()> {
        let path = self.path_for(&pending.proposal_id);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let json = serde_json::to_string(pending).map_err(|error| {
            ClientError::InvalidInput(format!("Failed to serialize pending chat turn: {error}"))
        })?;
        fs::write(path, json)?;
        Ok(())
    }

    /// Loads and removes the pending state — a command decision can only be
    /// resumed once.
    fn take(&self, proposal_id: &str) -> Result<PendingChatTurn> {
        let path = self.path_for(proposal_id);
        let content = fs::read_to_string(&path).map_err(|_| {
            ClientError::InvalidInput(format!("No pending chat turn for proposal: {proposal_id}"))
        })?;
        let pending: PendingChatTurn = serde_json::from_str(&content).map_err(|error| {
            ClientError::InvalidInput(format!("Failed to parse pending chat turn: {error}"))
        })?;
        let _ = fs::remove_file(&path);
        Ok(pending)
    }
}

/// Absolute upper bound for config-driven tool budgets. Raising past this
/// requires explicit UI work so an agent cannot silently spend a whole session
/// on repeated tool calls.
const ABSOLUTE_TOOL_ROUND_CAP: u32 = 16;

fn system_prompt() -> String {
    "You are a local-first coding assistant. Answer using only the provided repository context when possible. Cite relevant file paths. Do not request or expose secrets.\n\nRepository context sections named `agent_instruction` contain AGENTS.md instructions for this repository. Follow them when they apply to the files you discuss or edit. More specific nested AGENTS.md instructions override broader ones. The user's request and Damaian's safety policy take precedence over repository instructions.\n\nIf the user asks about current Git state, recent commits, latest changes, uncommitted changes, repository history, or another fact that requires a local command, your entire response must be exactly one command request envelope. Do not add prose before or after the envelope:\nDAMAIAN_COMMAND_V1\nCOMMAND: git log -1 --stat --oneline\nREASON: Inspect the latest commit for the user's question.\nEND_COMMAND\n\nPrefer read-only commands such as git status, git log, git show, git diff, ls, and pwd when they are sufficient. The app will run sandbox-safe commands automatically. When the user's task requires a command with side effects, network access, Docker access, shell control, or unknown risk, request the command and Damaian will pause for user approval before running it."
        .to_string()
}

fn build_model_prompt(
    prompt: &str,
    items: &[crate::context_manager::ContextItem],
    prior_messages: &[ChatMessage],
    command_context: Option<&str>,
) -> String {
    let mut output = String::new();
    let recent_messages = prior_messages
        .iter()
        .rev()
        .take(8)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect::<Vec<_>>();
    if !recent_messages.is_empty() {
        output.push_str("Recent conversation:\n");
        for message in recent_messages {
            output.push_str(&message.role);
            output.push_str(": ");
            output.push_str(&truncate_for_prompt(&message.content, 2_000));
            output.push('\n');
        }
        output.push('\n');
    }
    output.push_str("User request:\n");
    output.push_str(prompt);
    output.push_str("\n\nRepository context:\n");
    for item in items {
        output.push_str("\n--- ");
        output.push_str(&item.kind);
        if let Some(path) = &item.path {
            output.push_str(": ");
            output.push_str(path);
        }
        output.push_str(" ---\n");
        output.push_str(&item.content);
        if !item.content.ends_with('\n') {
            output.push('\n');
        }
    }
    if let Some(command_context) = command_context {
        output.push_str("\n--- sandbox_command_result ---\n");
        output.push_str(command_context);
        if !command_context.ends_with('\n') {
            output.push('\n');
        }
    }
    output
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CommandRequest {
    command: String,
    reason: String,
}

/// What a matched tool call (native `tools`/`tool_calls`, or the
/// `DAMAIAN_COMMAND_V1` text envelope for `Command`) asked the client to do.
/// `run_agentic_turn` dispatches on this rather than on raw tool names so
/// the text-envelope fallback and native tool calls funnel through the same
/// handling per variant.
#[derive(Debug, Clone, PartialEq, Eq)]
enum ToolAction {
    Command(CommandRequest),
    ProposePatch(GeneratedEdit),
    ReadFile(String),
    SearchCodebase {
        query: String,
        semantic: bool,
        limit: usize,
    },
    ReadGitStatus,
    ReadGitDiff {
        staged: bool,
    },
    WebDiagnostic(WebDiagnosticCall),
    McpCall {
        server_id: String,
        tool_name: String,
        arguments_json: String,
    },
}

/// What to show the user while a tool runs. Lives here rather than in the UI so
/// the frontend never has to map tool names to prose.
fn tool_action_label(action: &ToolAction) -> String {
    match action {
        ToolAction::Command(request) => format!("Proposing `{}`", request.command),
        ToolAction::ProposePatch(_) => "Preparing a patch".to_string(),
        ToolAction::ReadFile(path) => format!("Reading {path}"),
        ToolAction::SearchCodebase { query, .. } => format!("Searching for \"{query}\""),
        ToolAction::ReadGitStatus => "Reading git status".to_string(),
        ToolAction::ReadGitDiff { staged } => {
            if *staged {
                "Reading the staged diff".to_string()
            } else {
                "Reading the working diff".to_string()
            }
        }
        ToolAction::WebDiagnostic(call) => match call.kind {
            WebDiagnosticKind::Inspect => format!("Inspecting {}", call.url),
            WebDiagnosticKind::Scenario => format!("Running web scenario on {}", call.url),
        },
        ToolAction::McpCall {
            server_id,
            tool_name,
            ..
        } => format!("Calling {tool_name} on {server_id}"),
    }
}

/// The tools offered to providers configured with `supports_native_tools`.
/// `run_command` mirrors the `DAMAIAN_COMMAND_V1` envelope's capability
/// through a real `tools`/`tool_calls` contract instead of a text
/// convention; the rest have no text-envelope equivalent — they only exist
/// as native tool calls.
fn run_command_tool_definition() -> ToolDefinition {
    ToolDefinition {
        name: "run_command".to_string(),
        description: "Request a local shell command in the selected repository. Damaian runs sandbox-safe read-only commands automatically and pauses for user approval before running commands with side effects, network access, Docker access, shell control, or unknown risk.".to_string(),
        parameters_json: "{\"type\":\"object\",\"properties\":{\"command\":{\"type\":\"string\",\"description\":\"The shell command to run\"},\"reason\":{\"type\":\"string\",\"description\":\"Why this command is needed\"}},\"required\":[\"command\"]}".to_string(),
    }
}

/// Mirrors `GeneratedEdit`/`ProposedChange` (the same shape
/// `parse_generated_edit`'s `DAMAIAN_EDIT_V1` envelope produces) so a
/// tool-call-driven proposal converts directly into the same
/// `PatchEngine::create_patch` call the text-envelope edit flow already
/// uses. The user must still approve before anything is written to disk —
/// this only prepares a reviewable patch.
fn propose_patch_tool_definition() -> ToolDefinition {
    ToolDefinition {
        name: "propose_patch".to_string(),
        description: "Propose a code change as a reviewable patch. Nothing is written to disk until the user approves it.".to_string(),
        parameters_json: "{\"type\":\"object\",\"properties\":{\"summary\":{\"type\":\"string\",\"description\":\"Short summary of the change\"},\"files\":{\"type\":\"array\",\"items\":{\"type\":\"object\",\"properties\":{\"path\":{\"type\":\"string\",\"description\":\"Repository-relative file path\"},\"status\":{\"type\":\"string\",\"enum\":[\"added\",\"modified\",\"deleted\"],\"description\":\"Optional; inferred from whether the file currently exists if omitted\"},\"content\":{\"type\":\"string\",\"description\":\"Full replacement file content; use an empty string for deleted files\"}},\"required\":[\"path\",\"content\"]}}},\"required\":[\"summary\",\"files\"]}".to_string(),
    }
}

fn read_file_tool_definition() -> ToolDefinition {
    ToolDefinition {
        name: "read_file".to_string(),
        description: "Read a file from the repository to help answer the user's question.".to_string(),
        parameters_json: "{\"type\":\"object\",\"properties\":{\"path\":{\"type\":\"string\",\"description\":\"Repository-relative file path\"}},\"required\":[\"path\"]}".to_string(),
    }
}

fn search_codebase_tool_definition() -> ToolDefinition {
    ToolDefinition {
        name: "search_codebase".to_string(),
        description: "Search the repository index for files relevant to a query.".to_string(),
        parameters_json: "{\"type\":\"object\",\"properties\":{\"query\":{\"type\":\"string\"},\"mode\":{\"type\":\"string\",\"enum\":[\"keyword\",\"semantic\"],\"description\":\"Defaults to keyword\"},\"limit\":{\"type\":\"integer\",\"description\":\"Max results, defaults to 8\"}},\"required\":[\"query\"]}".to_string(),
    }
}

fn read_git_status_tool_definition() -> ToolDefinition {
    ToolDefinition {
        name: "read_git_status".to_string(),
        description: "Read the repository's current git status (modified, staged, untracked, and conflicted files).".to_string(),
        parameters_json: "{\"type\":\"object\",\"properties\":{}}".to_string(),
    }
}

fn read_git_diff_tool_definition() -> ToolDefinition {
    ToolDefinition {
        name: "read_git_diff".to_string(),
        description: "Read the repository's current git diff.".to_string(),
        parameters_json: "{\"type\":\"object\",\"properties\":{\"staged\":{\"type\":\"boolean\",\"description\":\"Read the staged diff instead of the working tree diff; defaults to false\"}},\"required\":[]}".to_string(),
    }
}

fn inspect_web_page_tool_definition() -> ToolDefinition {
    ToolDefinition {
        name: "inspect_web_page".to_string(),
        description: "Inspect a web page in a browser diagnostic runner. Use this for local web-app troubleshooting to capture page errors, console output, failed requests, visible DOM summary, accessibility summary, and screenshot metadata without writing files in the repository.".to_string(),
        parameters_json: "{\"type\":\"object\",\"properties\":{\"url\":{\"type\":\"string\",\"description\":\"The page URL to inspect, usually a localhost URL from the user's dev server\"},\"viewport\":{\"type\":\"object\",\"properties\":{\"width\":{\"type\":\"integer\",\"minimum\":320,\"maximum\":4096},\"height\":{\"type\":\"integer\",\"minimum\":240,\"maximum\":2160}},\"required\":[\"width\",\"height\"]},\"wait_ms\":{\"type\":\"integer\",\"minimum\":0,\"maximum\":10000,\"description\":\"How long to wait after navigation before collecting diagnostics\"},\"capture\":{\"type\":\"object\",\"properties\":{\"screenshot\":{\"type\":\"boolean\"},\"dom\":{\"type\":\"boolean\"},\"accessibility\":{\"type\":\"boolean\"},\"network\":{\"type\":\"boolean\"},\"console\":{\"type\":\"boolean\"}}}},\"required\":[\"url\"]}".to_string(),
    }
}

fn run_web_scenario_tool_definition() -> ToolDefinition {
    ToolDefinition {
        name: "run_web_scenario".to_string(),
        description: "Run a short browser interaction scenario for web-app troubleshooting, then return page errors, console output, failed requests, visible DOM state, and screenshot metadata. Prefer one diagnostic-rich scenario over many small browser calls.".to_string(),
        parameters_json: format!(
            "{{\"type\":\"object\",\"properties\":{{\"url\":{{\"type\":\"string\",\"description\":\"The starting page URL\"}},\"viewport\":{{\"type\":\"object\",\"properties\":{{\"width\":{{\"type\":\"integer\",\"minimum\":320,\"maximum\":4096}},\"height\":{{\"type\":\"integer\",\"minimum\":240,\"maximum\":2160}}}},\"required\":[\"width\",\"height\"]}},\"actions\":{{\"type\":\"array\",\"items\":{{\"type\":\"object\",\"properties\":{{\"action\":{{\"type\":\"string\",\"enum\":[{}]}},\"selector\":{{\"type\":\"string\"}},\"value\":{{\"type\":\"string\"}},\"text\":{{\"type\":\"string\"}},\"key\":{{\"type\":\"string\"}},\"ms\":{{\"type\":\"integer\",\"minimum\":0,\"maximum\":10000}},\"path\":{{\"type\":\"string\"}}}},\"required\":[\"action\"]}}}},\"capture\":{{\"type\":\"object\",\"properties\":{{\"screenshot\":{{\"type\":\"boolean\"}},\"dom\":{{\"type\":\"boolean\"}},\"network\":{{\"type\":\"boolean\"}},\"console\":{{\"type\":\"boolean\"}}}}}}}},\"required\":[\"url\",\"actions\"]}}",
            WEB_SCENARIO_ACTIONS
                .iter()
                .map(|action| format!("\"{action}\""))
                .collect::<Vec<_>>()
                .join(",")
        ),
    }
}

/// Recognizes any of the native tools above by name and extracts a
/// [`ToolAction`] from its arguments. Returns `None` for an unrecognized
/// tool name or malformed/empty arguments — the caller treats that the same
/// as the model not having requested a tool at all, matching the existing
/// (and equally permissive) behavior of `command_request_from_tool_call`.
fn tool_action_from_call(call: &ToolCall) -> Result<Option<ToolAction>> {
    match call.name.as_str() {
        "run_command" => Ok(command_request_from_tool_call(call).map(ToolAction::Command)),
        "propose_patch" => Ok(generated_edit_from_tool_call(call).map(ToolAction::ProposePatch)),
        "read_file" => {
            let Ok(arguments) = serde_json::from_str::<serde_json::Value>(&call.arguments_json)
            else {
                return Ok(None);
            };
            let Some(path) = arguments
                .get("path")
                .and_then(|value| value.as_str())
                .map(str::trim)
                .filter(|value| !value.is_empty())
            else {
                return Ok(None);
            };
            Ok(Some(ToolAction::ReadFile(path.to_string())))
        }
        "search_codebase" => {
            let Ok(arguments) = serde_json::from_str::<serde_json::Value>(&call.arguments_json)
            else {
                return Ok(None);
            };
            let Some(query) = arguments
                .get("query")
                .and_then(|value| value.as_str())
                .map(str::trim)
                .filter(|value| !value.is_empty())
            else {
                return Ok(None);
            };
            let query = query.to_string();
            if query.is_empty() {
                return Ok(None);
            }
            let semantic =
                arguments.get("mode").and_then(|value| value.as_str()) == Some("semantic");
            let limit = arguments
                .get("limit")
                .and_then(|value| value.as_u64())
                .map(|value| value as usize)
                .filter(|value| *value > 0)
                .unwrap_or(8)
                .min(20);
            Ok(Some(ToolAction::SearchCodebase {
                query,
                semantic,
                limit,
            }))
        }
        "read_git_status" => Ok(Some(ToolAction::ReadGitStatus)),
        "read_git_diff" => {
            let staged = serde_json::from_str::<serde_json::Value>(&call.arguments_json)
                .ok()
                .and_then(|value| value.get("staged").and_then(|value| value.as_bool()))
                .unwrap_or(false);
            Ok(Some(ToolAction::ReadGitDiff { staged }))
        }
        "inspect_web_page" | "run_web_scenario" => {
            WebDiagnosticCall::from_tool_call(&call.name, &call.arguments_json)
                .map(|call| call.map(ToolAction::WebDiagnostic))
        }
        name => Ok(
            parse_namespaced_tool_name(name).map(|(server_id, tool_name)| ToolAction::McpCall {
                server_id,
                tool_name,
                arguments_json: call.arguments_json.clone(),
            }),
        ),
    }
}

fn first_decodable_tool_action(
    calls: &[ToolCall],
) -> (Option<ToolCall>, Option<ToolAction>, Option<String>) {
    for call in calls {
        match tool_action_from_call(call) {
            Ok(Some(action)) => return (Some(call.clone()), Some(action), None),
            Ok(None) => {}
            Err(error) => {
                return (
                    Some(call.clone()),
                    None,
                    Some(format!(
                        "Your `{}` call was rejected before execution: {error}. Retry with well-formed arguments matching the tool schema.",
                        call.name
                    )),
                );
            }
        }
    }
    (None, None, None)
}

/// Feedback for a tool call whose arguments couldn't be decoded. When the
/// provider truncated the response, says so explicitly and asks for a smaller
/// call — retrying the same oversized patch would just truncate again.
fn undecodable_tool_call_note(call: &ToolCall, truncated: bool) -> String {
    let name = &call.name;
    if truncated {
        format!(
            "Your `{name}` call was cut off because the response reached the model's maximum output length, so its arguments were incomplete and could not be used. Nothing was changed. Retry with a smaller call: for a patch, propose fewer files at a time — a single file per call if needed — and send the rest in follow-up calls."
        )
    } else {
        format!(
            "Your `{name}` call could not be decoded: the arguments were not valid JSON matching the tool's schema. Nothing was changed. Retry with well-formed arguments."
        )
    }
}

fn generated_edit_from_tool_call(call: &ToolCall) -> Option<GeneratedEdit> {
    if call.name != "propose_patch" {
        return None;
    }
    let arguments: serde_json::Value = serde_json::from_str(&call.arguments_json).ok()?;
    let summary = arguments
        .get("summary")
        .and_then(|value| value.as_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("Proposed model edit")
        .to_string();
    let files = arguments.get("files")?.as_array()?;
    if files.is_empty() {
        return None;
    }
    let mut changes = Vec::new();
    for file in files {
        let path = file.get("path")?.as_str()?.trim().to_string();
        if path.is_empty() {
            return None;
        }
        let content = file
            .get("content")
            .and_then(|value| value.as_str())
            .unwrap_or_default()
            .to_string();
        let status = file
            .get("status")
            .and_then(|value| value.as_str())
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string);
        changes.push(ProposedChange {
            path,
            new_content: content,
            status,
            allow_restricted: false,
        });
    }
    Some(GeneratedEdit { summary, changes })
}

fn command_request_from_tool_call(call: &ToolCall) -> Option<CommandRequest> {
    if call.name != "run_command" {
        return None;
    }
    let arguments: serde_json::Value = serde_json::from_str(&call.arguments_json).ok()?;
    let command = arguments.get("command")?.as_str()?.trim().to_string();
    if command.is_empty() {
        return None;
    }
    let reason = arguments
        .get("reason")
        .and_then(|value| value.as_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("Assistant requested a local command")
        .to_string();
    Some(CommandRequest { command, reason })
}

fn parse_command_request(value: &str) -> Option<CommandRequest> {
    let marker_start = value.find("DAMAIAN_COMMAND_V1")?;
    let envelope = &value[marker_start..];
    let envelope = if let Some(end_start) = envelope.find("END_COMMAND") {
        &envelope[..end_start + "END_COMMAND".len()]
    } else {
        envelope
    };
    let mut command = String::new();
    let mut reason = String::new();
    for raw_line in envelope.lines() {
        let line = raw_line.trim();
        if let Some(value) = line.strip_prefix("COMMAND:") {
            command = value.trim().to_string();
        } else if let Some(value) = line.strip_prefix("REASON:") {
            reason = value.trim().to_string();
        } else if line.trim() == "END_COMMAND" {
            break;
        }
    }
    if command.is_empty() {
        return None;
    }
    if reason.is_empty() {
        reason = "Assistant requested a local command".to_string();
    }
    Some(CommandRequest { command, reason })
}

fn command_proposal_response(proposal: &CommandProposal) -> String {
    if proposal.blocked {
        format!(
            "I cannot run `{}` in sandbox mode, and local policy blocks this command. Review or reject the command request below.",
            proposal.command
        )
    } else {
        format!(
            "I need your approval before running `{}` because it cannot run in sandbox mode.",
            proposal.command
        )
    }
}

fn agent_command_proposal(config: &Config, proposal: &CommandProposal) -> AgentCommandProposal {
    AgentCommandProposal {
        id: proposal.id.clone(),
        command: proposal.command.clone(),
        prompt: command_approval_prompt(proposal),
        risk: proposal.risk.as_str().to_string(),
        requires_approval: proposal.requires_approval,
        blocked: proposal.blocked,
        allow_always: allow_always_eligible(config, &proposal.command, proposal.blocked),
        allow_browser_diagnostics_for_session: false,
    }
}

fn mcp_call_summary(server_id: &str, tool_name: &str) -> String {
    format!("Called MCP tool `{tool_name}` on server `{server_id}`")
}

fn web_diagnostic_summary(call: &WebDiagnosticCall) -> String {
    match call.kind {
        WebDiagnosticKind::Inspect => format!("Inspected web page `{}`", call.url),
        WebDiagnosticKind::Scenario => format!("Ran web scenario against `{}`", call.url),
    }
}

fn web_diagnostic_signature(call: &WebDiagnosticCall) -> String {
    format!(
        "{}:{}",
        call.name(),
        normalize_tool_arguments(&call.arguments_json)
    )
}

fn normalize_tool_arguments(arguments_json: &str) -> String {
    serde_json::from_str::<serde_json::Value>(arguments_json)
        .map(|value| value.to_string())
        .unwrap_or_else(|_| arguments_json.trim().to_string())
}

fn browser_tool_result_failed(content: &str) -> bool {
    let lower = content.to_ascii_lowercase();
    lower.starts_with("browser diagnostic failed")
        || lower.contains("mcp tool reported an error")
        || lower.contains("tool call failed")
}

fn browser_retry_limit_note(retry_limit: u32) -> String {
    format!(
        "This browser diagnostic has already failed {retry_limit} time(s) with substantially similar arguments. Change approach: inspect the relevant source files, simplify the scenario, or ask for a different page state instead of repeating the same call."
    )
}

fn prompt_enters_web_debug_mode(prompt: &str) -> bool {
    let lower = prompt.to_ascii_lowercase();
    let has_url = lower.contains("http://localhost")
        || lower.contains("https://localhost")
        || lower.contains("http://127.0.0.1")
        || lower.contains("https://127.0.0.1")
        || lower.contains("http://[::1]")
        || lower.contains("https://[::1]");
    let has_web_symptom = [
        "web page", "web app", "website", "browser", "frontend", "button",
    ]
    .iter()
    .any(|term| lower.contains(term));
    has_url && has_web_symptom
}

fn looks_like_browser_diagnostics_mcp_server(server: &crate::config::McpServerConfig) -> bool {
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

fn model_output_requests_tool(model_run: &ModelRun, content: &str) -> bool {
    !model_run.tool_calls.is_empty()
        || parse_command_request(content).is_some()
        || content.contains("DAMAIAN_COMMAND_V1")
        || content.contains("tool_calls")
        || content.contains("DSML")
}

fn tool_budget_exhausted_response(max_rounds: u32) -> String {
    format!(
        "This turn reached the configured tool round limit ({max_rounds}) while the model was still trying to call another tool. I stopped instead of treating the raw tool request as a completed answer. Continue debugging with a narrower next request or approve a larger diagnostic budget if available."
    )
}

/// Builds the approval proposal surfaced to the user for a pending MCP tool
/// call, reusing the same [`AgentCommandProposal`] shape the command-approval
/// UI already renders. The arguments are truncated so a large payload can't
/// blow up the prompt.
fn mcp_approval_proposal(
    proposal_id: &str,
    server_id: &str,
    tool_name: &str,
    arguments_json: &str,
) -> AgentCommandProposal {
    let arguments = truncate_for_prompt(arguments_json.trim(), 500);
    let prompt = format!(
        "The assistant wants to call the MCP tool `{tool_name}` on server `{server_id}` with arguments:\n{arguments}\n\nThis runs outside the local sandbox and may have side effects. Approve to run it, or decline."
    );
    AgentCommandProposal {
        id: proposal_id.to_string(),
        command: format!("{server_id}/{tool_name}"),
        prompt,
        risk: "mcp".to_string(),
        requires_approval: true,
        blocked: false,
        // An MCP tool call is not a shell command, so `command_allowlist` has
        // nothing to say about it. Per-server `require_approval` in the MCP
        // config is the knob for making these stop prompting.
        allow_always: false,
        allow_browser_diagnostics_for_session: false,
    }
}

fn web_diagnostic_approval_proposal(
    proposal_id: &str,
    call: &WebDiagnosticCall,
) -> AgentCommandProposal {
    let arguments = truncate_for_prompt(call.arguments_json.trim(), 500);
    let risk = if call.is_low_risk() {
        "browser-low"
    } else if matches!(call.kind, WebDiagnosticKind::Scenario) {
        "browser-medium"
    } else {
        "browser-high"
    };
    let origin = url_origin_for_prompt(&call.url);
    let prompt = format!(
        "The assistant wants to run `{}` against `{}`.\nTarget origin: `{origin}`\n\nArguments:\n{arguments}\n\nBrowser diagnostics may navigate pages, use current browser state, or interact with forms. Approve to run it, or decline.",
        call.name(),
        call.url,
    );
    AgentCommandProposal {
        id: proposal_id.to_string(),
        command: format!("{} {}", call.name(), call.url),
        prompt,
        risk: risk.to_string(),
        requires_approval: true,
        blocked: false,
        allow_always: false,
        allow_browser_diagnostics_for_session: !call.is_low_risk(),
    }
}

fn url_origin_for_prompt(url: &str) -> String {
    let trimmed = url.trim();
    let Some((scheme, rest)) = trimmed.split_once("://") else {
        return trimmed.to_string();
    };
    let authority = rest
        .split(['/', '?', '#'])
        .next()
        .unwrap_or_default()
        .trim();
    if authority.is_empty() {
        trimmed.to_string()
    } else {
        format!("{scheme}://{authority}")
    }
}

fn patch_proposal_response(patch: &ProposedPatch) -> String {
    format!(
        "I've prepared a patch (`{}`) for {} file{}. Review the diff and apply or reject it when ready.",
        patch.id,
        patch.files.len(),
        if patch.files.len() == 1 { "" } else { "s" }
    )
}

fn agent_patch_proposal(patch: &ProposedPatch) -> AgentPatchProposal {
    AgentPatchProposal {
        patch_id: patch.id.clone(),
        summary: patch.summary.clone(),
        files: patch.files.clone(),
    }
}

fn format_search_results(results: &[SearchResult]) -> String {
    if results.is_empty() {
        return "No matching files found.".to_string();
    }
    results
        .iter()
        .map(|result| {
            format!(
                "{} (score {})\n{}",
                result.path,
                result.score,
                truncate_for_prompt(&result.snippet, 500)
            )
        })
        .collect::<Vec<_>>()
        .join("\n\n")
}

fn format_git_status(status: &GitStatus) -> String {
    if status.clean {
        "Working tree clean.".to_string()
    } else {
        status
            .files
            .iter()
            .map(|file| file.raw.clone())
            .collect::<Vec<_>>()
            .join("\n")
    }
}

fn tool_call_summary(command_request: &CommandRequest) -> String {
    format!(
        "Ran `{}` — {}",
        command_request.command, command_request.reason
    )
}

fn sandbox_command_context(execution: &CommandExecution) -> String {
    let mut output = String::new();
    output.push_str("Command: ");
    output.push_str(&execution.command);
    output.push('\n');
    output.push_str("Working directory: ");
    output.push_str(&execution.working_directory);
    output.push('\n');
    output.push_str("Exit code: ");
    output.push_str(&execution.exit_code.unwrap_or(-1).to_string());
    output.push_str("\n\nSTDOUT:\n");
    output.push_str(&truncate_for_prompt(&execution.stdout, 8_000));
    output.push_str("\n\nSTDERR:\n");
    output.push_str(&truncate_for_prompt(&execution.stderr, 4_000));
    output
}

fn truncate_for_prompt(value: &str, max_chars: usize) -> String {
    let mut output = value.chars().take(max_chars).collect::<String>();
    if value.chars().count() > max_chars {
        output.push_str("\n[truncated]");
    }
    output
}

/// What the checkpoint list shows for a turn: the prompt it precedes, short
/// enough to read in a list and long enough to recognise.
fn checkpoint_summary(prompt: &str) -> String {
    let summary = prompt
        .split_whitespace()
        .take(12)
        .collect::<Vec<_>>()
        .join(" ");
    if summary.is_empty() {
        "Before this turn".to_string()
    } else {
        format!("Before: {summary}")
    }
}

fn session_title(prompt: &str) -> String {
    let title = prompt
        .split_whitespace()
        .take(8)
        .collect::<Vec<_>>()
        .join(" ");
    if title.is_empty() {
        format!("Chat {}", create_id("session_title"))
    } else {
        title
    }
}
