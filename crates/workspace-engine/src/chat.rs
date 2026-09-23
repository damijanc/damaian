use crate::audit::AuditLog;
use crate::cancel::CancelToken;
use crate::checkpoint::{
    CheckpointConversation, CheckpointRequest, CheckpointStore, CommandCensus, PendingApproval,
};
use crate::command_policy::allow_always_eligible;
use crate::command_runner::{CommandExecution, CommandTermination};
use crate::config::{Config, CostEstimate, McpTransport};
use crate::context_manager::ContextManager;
use crate::edit::{GeneratedEdit, PatchStore, RegionEdit, region_edits_to_changes};
use crate::error::{ClientError, Result};
use crate::file_access::{FileAccessController, LineRange, ReadWindow};
use crate::git_service::{GitService, GitStatus};
use crate::hash::{create_id, now_millis};
use crate::indexer::{ProjectIndexer, SearchResult};
use crate::mcp::{McpRuntime, McpServerRuntime, parse_namespaced_tool_name};
use crate::model::{
    ModelAdapter, ModelMessage, ModelRequest, ModelRun, TokenUsage, ToolCall, ToolDefinition,
    model_request_json,
};
use crate::navigation::NavigationController;
use crate::patch_engine::{PatchEngine, ProposedChange, ProposedFilePatch, ProposedPatch};
use crate::path_policy::PathPolicy;
use crate::plan::TaskPlan;
use crate::secret_scanner::SecretScanner;
use crate::session::{ChatMessage, Session, SessionStore, Task, TaskStatus, TaskUsage};
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

/// What a tool arm observed, so its marker is finished with the tool's own
/// answer rather than with the fact that dispatch returned.
///
/// Kept separate from the arm's text output because the text is for the model
/// and this is for the log: a tool that reports an error in prose still has to
/// record that it failed, and reading the prose back to find out would be
/// guessing. See
/// `docs/specs/21_task_plan_progress_and_budget/context.md` §3.4.
#[derive(Debug, Clone, PartialEq, Eq)]
enum ActionOutcome {
    /// Dispatch succeeded and the tool reported nothing to the contrary.
    Ok,
    /// A command ran. The code is whatever the process reported, and `None`
    /// means it was killed or signalled — not that it passed.
    CommandExit(Option<i32>),
    /// A file was read, carrying what was read and the hash of the content.
    ///
    /// Both travel out of the arm for the same reason the exit code does:
    /// nothing downstream can recover them, and a step whose only work was
    /// reading would otherwise report *completed unverified* despite the
    /// engine having watched the read happen.
    FileRead { path: String, hash: String },
    /// The tool itself reported a failure: an MCP `is_error` or transport
    /// error, a browser diagnostic that could not run. Not a command, so there
    /// is no exit code to carry.
    Failed,
    /// A stop reached a concurrent batch before this call started, so it did
    /// nothing. Distinct from `Failed`: nothing was attempted, and the turn is
    /// about to end cancelled rather than treat the tool as having errored.
    Cancelled,
}

/// The evidence an arm's outcome supports, or `None` where the engine observed
/// nothing a step's status could honestly rest on.
///
/// Requirement 6 lives here as much as in the enum: this is the only place a
/// tool dispatch becomes [`Evidence`], the inputs are values the engine holds,
/// and no model output reaches it. Nothing constructs `Evidence` from a tool
/// argument, and nothing should.
///
/// [`ActionOutcome::Ok`] yields `None` rather than a synthetic success record.
/// A `read_file` that worked says nothing about whether the step it served is
/// done, and minting evidence from it would make every step look verified —
/// the letter-not-spirit failure requirement 6 exists to prevent.
/// [`ActionOutcome::Failed`] likewise: it carries no code, so there is nothing
/// to record that would not be invented, and the step's status follows from
/// the *absence* of confirming evidence.
fn evidence_for(outcome: &ActionOutcome, marker_id: &str) -> Option<crate::plan::Evidence> {
    match outcome {
        ActionOutcome::CommandExit(exit_code) => Some(crate::plan::Evidence::CommandExit {
            marker_id: marker_id.to_string(),
            exit_code: *exit_code,
        }),
        // No marker id: unlike a command, the fact worth keeping is *which
        // content* was read, and the hash says that across turns while a
        // marker id only points at the call. §5.3's rule that evidence must
        // carry the fact rather than a pointer into a store that expires.
        ActionOutcome::FileRead { path, hash } => Some(crate::plan::Evidence::FileRead {
            path: path.clone(),
            hash: hash.clone(),
        }),
        ActionOutcome::Ok | ActionOutcome::Failed | ActionOutcome::Cancelled => None,
    }
}

/// Why the agent loop ended.
///
/// An enum rather than two booleans: two flags admit a state where both are
/// set, and the audit status string and the task status would then each have to
/// pick one arbitrarily. The loop can only stop for one reason.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StopReason {
    /// The model produced an answer, or the turn is pausing for a human.
    Answered,
    /// `agent_max_tool_rounds` was reached and the model still wanted a tool.
    ToolBudget,
    /// `agent_max_task_tokens` was reached. Unlike [`Self::ToolBudget`] this is
    /// detected *before* a model call rather than after one, so no tokens are
    /// spent discovering it (`context.md` §3.3).
    TokenBudget,
}

/// Which stage of a turn is running. Drives the progress indicator, so the user
/// can tell a slow provider apart from a running tool apart from a hang.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PhaseKind {
    Context,
    Model,
    Tool,
    Finalizing,
    /// A line of a running command's output, streamed before it exits
    /// (requirement 5). Distinct from the other kinds so a client can render it
    /// as a log line rather than replacing the status badge it is not.
    Output,
}

impl PhaseKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Context => "context",
            Self::Model => "model",
            Self::Tool => "tool",
            Self::Finalizing => "finalizing",
            Self::Output => "output",
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
    /// The turn's plan, whole, each time it changes (spec 21 §5.6).
    ///
    /// Sent whole rather than as a delta because the panel's job is to show
    /// the current state of every step at once, and a client that missed one
    /// delta — a reconnect, a dropped frame — would then render a plan that
    /// never existed. The plan is small and changes a handful of times per
    /// turn, so there is nothing to save by being clever here.
    Plan(TaskPlan),
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

    fn plan(&mut self, plan: &TaskPlan) {
        (self.on_progress)(TurnProgress::Plan(plan.clone()));
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

/// A plan put up for review before the turn takes its first mutating step
/// (spec 21 §5.5).
///
/// Carries the whole plan rather than a summary: the user is being asked to
/// reorder, retitle or delete steps, and cannot do that from a count.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentPlanProposal {
    /// The id to hand back to
    /// [`ChatOrchestrator::resume_after_plan_decision`]. Distinct from the
    /// task id: a task may be reviewed once, but the pending turn behind the
    /// pause is what the id addresses.
    pub id: String,
    pub plan: TaskPlan,
    /// What the turn was about to do when it stopped, in the same words the
    /// progress line uses — so the card can say what the approval unblocks
    /// rather than asking the user to approve in the abstract.
    pub deferred_action: String,
}

/// One step of a user's revision to a proposed plan.
///
/// Identifies the step by id rather than position so a reorder and a deletion
/// in the same revision cannot be misread as each other. A step the plan does
/// not contain is ignored, and one the user omits is dropped — that is how
/// deletion is expressed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlanRevisionStep {
    pub id: String,
    pub title: String,
}

// No `Eq`: it carries a `ModelRun`, whose `reported_cost` is an `Option<f64>`.
#[derive(Debug, Clone, PartialEq)]
pub struct ChatTurnResult {
    pub session: Session,
    pub task: Task,
    pub model_run: ModelRun,
    pub context_files: Vec<String>,
    pub response: String,
    pub command_proposal: Option<AgentCommandProposal>,
    pub patch_proposal: Option<AgentPatchProposal>,
    /// The turn stopped to have its plan reviewed before taking its first
    /// mutating step. Resumed through
    /// [`ChatOrchestrator::resume_after_plan_decision`].
    pub plan_proposal: Option<AgentPlanProposal>,
    /// The user stopped this turn. Distinct from a failure: `response` holds
    /// whatever had been generated, and it is persisted.
    pub cancelled: bool,
    /// What this task has spent, summed over every model call it made — not
    /// just [`Self::model_run`], which is only the last one.
    ///
    /// Read back from the log rather than accumulated in memory, so the
    /// figure a client sees when the turn ends is the same one it sees after
    /// a reload. `None` only if the log could not be read.
    pub usage: Option<TaskUsage>,
    /// Cost computed from the user's own configured rates, when they have set
    /// any. Kept separate from [`TaskUsage::reported_cost`] on purpose: one is
    /// what the provider charged, the other is the user's own arithmetic, and
    /// presenting the second as the first would launder a guess into a fact.
    ///
    /// A [`CostEstimate`] rather than a bare `f64` so the upper-bound label
    /// travels with the number: spec 49 §5.3.
    pub estimated_cost: Option<CostEstimate>,
}

/// What a turn ended holding out for a human, if anything.
///
/// Grouped rather than carried as three separate `Option`s through the loop's
/// break value: every `break` site has to name all of them, and a fourth kind
/// of pause would otherwise mean editing eight places that have no opinion
/// about it.
#[derive(Debug, Default)]
struct TurnProposals {
    command: Option<AgentCommandProposal>,
    patch: Option<AgentPatchProposal>,
    plan: Option<AgentPlanProposal>,
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
    navigation: NavigationController,
    path_policy: PathPolicy,
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
        navigation: NavigationController,
        path_policy: PathPolicy,
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
            navigation,
            path_policy,
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
    fn build_mcp_runtime(&self, session_id: &str) -> McpRuntime {
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
            McpRuntime::new(
                servers,
                self.audit_log.clone(),
                self.config.data_dir.clone(),
                session_id,
            )
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
        // Context assembly is the next thing to happen, and it is read-only,
        // so a crash here is resumable. The finer in-flight states are set at
        // the action sites when their markers go in.
        task = self
            .session_store
            .update_task_status(&task, TaskStatus::PreparingContext, None)?;
        // §5.4: a turn stopped by the token ceiling is recoverable — "the user
        // can raise the ceiling and resume, and the remaining steps are what
        // resumption starts from". A task is one turn, so this new task would
        // otherwise start with no plan at all (`context.md` §3.1).
        //
        // The same is true of a turn a crash interrupted and the user chose to
        // continue (`docs/specs/45_crash_recovery_prompt.md` §5.8).
        //
        // Narrow on purpose: only a task that actually handed its work to this
        // one — a ceiling stop or a recorded supersession — and only while its
        // plan still has work outstanding. Carrying a plan into an unrelated
        // next question would put steps on the panel the user never asked for,
        // and a normally-completed task has nothing to resume.
        self.carry_plan_from_the_previous_turn(&session.id, &task)?;
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
        // A plan review shares the pending-turn file but not this path: there
        // is no dispatched call to execute or decline, and falling through to
        // the shell-command branch below would run `last_content` as a
        // command. Refused loudly rather than guessed at — and the pending
        // turn is put back, so the right resume can still find it.
        if pending.plan_review.is_some() {
            self.pending_commands.save(&pending)?;
            return Err(ClientError::InvalidInput(format!(
                "Proposal {proposal_id} is a plan review; resume it with resume_after_plan_decision"
            )));
        }
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
                let mut mcp = self.build_mcp_runtime(&pending.session.id);
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
                let max_rounds = self.tool_round_limit(false, pending.turn_options);
                let cancel = sink.cancel;
                // Borrow only the progress field, so `cancel` can be read for
                // the same call without two conflicting borrows of the sink.
                let on_progress = &mut *sink.on_progress;
                let mut on_output = |line: &str| {
                    on_progress(TurnProgress::Phase(TurnPhase::new(
                        PhaseKind::Output,
                        line,
                        pending.round,
                        max_rounds,
                    )));
                };
                let record = self.validation_orchestrator.run_proposal(
                    proposal_id,
                    true,
                    approved_by,
                    Some(&pending.task.id),
                    cancel,
                    &mut on_output,
                )?;
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
            messages.push(
                ModelMessage::assistant(pending.last_content.clone())
                    .with_reasoning_content(pending.reasoning_content.clone()),
            );
            messages.push(ModelMessage::user(format!(
                "Command result:\n{tool_result_content}"
            )));
        }

        // The pause is over either way, so the checkpoint stops claiming one.
        self.note_pending_approvals(&pending.session, &pending.task, Vec::new());
        // The approved command has already run — its result is the `tool`
        // message appended above — so the side effect is behind us and what
        // follows is another model round. Verified by reading rather than
        // assumed: labelling a still-executing command as read-only would let
        // the classifier auto-resume it, which is exactly what requirement 5
        // forbids.
        let task = self.session_store.update_task_status(
            &pending.task,
            TaskStatus::PreparingContext,
            None,
        )?;

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

    /// Continues a chat turn that stopped to have its plan reviewed before
    /// taking its first mutating step (spec 21 §5.5).
    ///
    /// `revised` is the user's edit of the step list: steps in the order they
    /// should run, each naming an existing step id, with whatever title the
    /// user wants it to carry. Omitting a step deletes it. `None` means the
    /// plan was approved as proposed.
    ///
    /// A revision is recorded as `plan_revised` and the approval as
    /// `plan_approved`, in that order, so the log holds both what the model
    /// proposed and what the user decided to run — and `approved: false`
    /// records neither, because nothing was approved.
    ///
    /// Nothing was dispatched when the turn paused, so there is no call to
    /// execute or discard here: the turn resumes with the decision put to the
    /// model as a message, and the model asks again for whatever it still
    /// needs. Replaying the deferred call instead would run the step the user
    /// may have just deleted.
    pub fn resume_after_plan_decision(
        &self,
        proposal_id: &str,
        approved: bool,
        revised: Option<Vec<PlanRevisionStep>>,
        approved_by: &str,
        model_adapter: &mut dyn ModelAdapter,
        sink: &mut TurnSink<'_>,
    ) -> Result<ChatTurnResult> {
        let pending = self.pending_commands.take(proposal_id)?;
        let Some(review) = pending.plan_review.clone() else {
            self.pending_commands.save(&pending)?;
            return Err(ClientError::InvalidInput(format!(
                "Proposal {proposal_id} is not a plan review"
            )));
        };
        let repository_root = PathBuf::from(&pending.repository_root);
        let mut messages = pending.messages;

        let plan = self
            .session_store
            .read_task_plan(&pending.session.id, &pending.task.id)?;
        let note = if !approved {
            // The same shape as a declined command: the work does not happen,
            // and the model is told plainly rather than left to infer it from
            // a tool result that never arrives.
            "The user reviewed this plan and declined it. Do not make any change to the repository. Explain briefly what you were going to do and what you would need from the user to proceed differently."
                .to_string()
        } else {
            let applied = match (revised, plan) {
                (Some(revision), Some(current)) => {
                    let revised_plan = apply_plan_revision(&current, &revision);
                    self.session_store
                        .revise_plan(&pending.task, &revised_plan)?;
                    Some(revised_plan)
                }
                // A revision with no plan to revise, or no revision at all:
                // either way there is nothing to rewrite, and approving is
                // still a decision worth recording.
                (_, plan) => plan,
            };
            self.session_store
                .approve_plan(&pending.task, approved_by)?;
            match applied {
                Some(plan) => format!(
                    "The user reviewed the plan and approved it. The plan is now:\n{}\nContinue with the current step. You were about to: {}.",
                    plan.steps
                        .iter()
                        .enumerate()
                        .map(|(index, step)| format!("{}. {}", index + 1, step.title))
                        .collect::<Vec<_>>()
                        .join("\n"),
                    review.deferred_action
                ),
                None => format!(
                    "The user reviewed the plan and approved it. Continue with the current step. You were about to: {}.",
                    review.deferred_action
                ),
            }
        };

        self.audit_log.record(
            "plan_review_decision",
            &[
                ("actor", approved_by.to_string()),
                ("sessionId", pending.session.id.clone()),
                ("taskId", pending.task.id.clone()),
                ("proposalId", proposal_id.to_string()),
                (
                    "decision",
                    if approved { "approved" } else { "declined" }.to_string(),
                ),
            ],
        )?;

        self.session_store.append_message(
            &pending.session.id,
            Some(&pending.task.id),
            "user",
            &note,
        )?;
        messages.push(
            ModelMessage::assistant(pending.last_content.clone())
                .with_reasoning_content(pending.reasoning_content.clone()),
        );
        messages.push(ModelMessage::user(note));

        self.note_pending_approvals(&pending.session, &pending.task, Vec::new());
        let task = self.session_store.update_task_status(
            &pending.task,
            TaskStatus::PreparingContext,
            None,
        )?;

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
                    task_status: TaskStatus::PreparingContext.as_str().to_string(),
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
    /// Carries a plan onto `task` when the previous task in the session handed
    /// its work to this one with steps still outstanding.
    ///
    /// Two ways that happens, and they are the only two: the previous turn hit
    /// the token ceiling and the user raised it (spec 21 §5.4), or it was
    /// interrupted by a crash and the user pressed `Continue`
    /// (`docs/specs/45_crash_recovery_prompt.md` §5.4), which re-sends the same
    /// request as a new turn and records the old task as superseded.
    ///
    /// **The crash case is safe for the same reason the resume was offered at
    /// all.** A plan carries the steps already completed and the evidence
    /// behind them, so carrying one after a crash would be wrong if a step
    /// could have half-happened. It cannot: a resume is refused outright for
    /// `unknown_external_outcome`, so the only crashes that reach here had
    /// nothing side-effecting in flight. Without this the resumed turn
    /// re-plans from nothing and can redo steps whose work already landed —
    /// which is worse than the token-stop case it was already avoiding.
    ///
    /// Best-effort by design: a session whose log cannot be read should not
    /// stop the user asking a question, and the worst case of skipping it is a
    /// turn that starts a fresh plan.
    fn carry_plan_from_the_previous_turn(&self, session_id: &str, task: &Task) -> Result<()> {
        let tasks = self.session_store.read_tasks(session_id)?;
        // The one before this turn's own, which was appended a moment ago.
        let Some(previous) = tasks.iter().rev().find(|candidate| candidate.id != task.id) else {
            return Ok(());
        };
        let handed_over = previous.status == TaskStatus::TokenBudgetExhausted
            || self
                .session_store
                .superseded_task_ids(session_id)?
                .contains(&previous.id);
        if !handed_over {
            return Ok(());
        }
        let Some(plan) = self
            .session_store
            .read_task_plan(session_id, &previous.id)?
        else {
            return Ok(());
        };
        let outstanding = plan.steps.iter().any(|step| {
            matches!(
                step.status,
                crate::plan::StepStatus::Pending | crate::plan::StepStatus::InProgress
            )
        });
        if !outstanding {
            return Ok(());
        }
        self.session_store.resume_plan(task, &previous.id)
    }

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
        let mut mcp = self.build_mcp_runtime(&session.id);
        let browser_mcp_server_ids = self.browser_diagnostic_mcp_server_ids();
        let native_tools = self.config.supports_native_tools().then(|| {
            let mut tools = vec![
                run_command_tool_definition(),
                propose_patch_tool_definition(),
                propose_plan_tool_definition(),
                complete_step_tool_definition(),
                read_file_tool_definition(),
                list_directory_tool_definition(),
                search_content_tool_definition(),
                edit_file_tool_definition(),
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
        // Whether any model call in this turn has been accounted for, so a
        // stop between rounds does not record a zero on top of real spending.
        let mut recorded_any_usage = false;
        let mut web_debug_mode =
            turn_options.continue_debugging || prompt_enters_web_debug_mode(&task.user_prompt);
        let mut failed_browser_calls = HashMap::new();
        // The turn's plan, once the model has proposed one, and the evidence
        // accrued since the current step started.
        //
        // Held here rather than re-read from the log each round because the
        // *accrual* has no home on disk until the step closes: evidence belongs
        // to a step, and which step is open is a fact about this turn. The plan
        // itself is written through as it changes, so a crash loses at most the
        // evidence of the step that was still running — which is the step whose
        // outcome was genuinely unknown.
        // Seeded from the log rather than left empty: a resumed turn already
        // has a plan (see `carry_plan_from_the_previous_turn`), and starting this
        // at `None` would make `complete_step` report "there is no plan" while
        // the panel showed one.
        let mut plan = self
            .session_store
            .read_task_plan(&session.id, &task.id)
            .unwrap_or_default();
        let mut step_evidence: Vec<crate::plan::Evidence> = Vec::new();
        // A turn that arrives with a plan already in hand — resumed after a
        // token stop, or after the user approved or revised it — shows it
        // before doing any work. Without this the panel would stay empty until
        // the first `complete_step`, which is the longest stretch of the turn.
        if let Some(current) = plan.as_ref() {
            sink.plan(current);
        }
        // Read from the log for the same reason the plan is: the approval is a
        // decision the user took, and a turn resumed after a restart must not
        // ask for it again. It is only ever read here — the gate below is the
        // single place it decides anything.
        let plan_approved = self
            .session_store
            .read_plan_approved(&session.id, &task.id)
            .unwrap_or(false);

        // Extra rounds a turn may take past its configured segment because a
        // token ceiling is set (requirement 6). Bounded by the absolute round
        // cap so a provider that reports no usage cannot loop forever.
        let mut round_budget_extension: u32 = 0;
        let (final_run, response, proposals, stop_reason) = loop {
            // Checked before each round rather than only mid-stream: stopping
            // here is what saves a whole model call, and it is the only point
            // that catches a stop arriving during context assembly or a tool.
            if sink.cancel.is_cancelled() {
                let cancelled_run = ModelRun::cancelled_before_start(
                    &self.config.model_provider,
                    &self.config.model_name,
                );
                // Only when nothing was called: a stop between rounds already
                // has its earlier rounds accounted, and adding a zero on top
                // would claim a model call that never happened.
                if !recorded_any_usage {
                    self.session_store.record_task_usage(
                        &task,
                        &cancelled_run.run_id,
                        None,
                        TokenUsage::measured_zero(),
                        None,
                        None,
                    )?;
                }
                return self.finish_cancelled_turn(
                    repository_root,
                    session,
                    task,
                    context_files,
                    &partial_response,
                    cancelled_run,
                );
            }

            // Before the request is built, not after the response arrives.
            // `force_final` — the round budget's shape — deliberately spends
            // one more model call on crossing, which is right for a bound on
            // rounds and backwards for a bound on money: context grows across
            // rounds, so that call is the most expensive of the turn. A ceiling
            // whose enforcement action is to spend more than the ceiling is not
            // a ceiling. `context.md` §3.3.
            if let Some(ceiling) = self.config.agent_max_task_tokens {
                // Absent usage is zero here, not "unknown". `read_task_usage`
                // omits a task with no events because "not recorded" and "used
                // nothing" differ for a *report*; for a *ceiling* nothing spent
                // is nothing spent, and treating absence as unenforceable would
                // disable the bound for the first call of every turn — the only
                // call some turns make. `context.md` §3.9.
                //
                // An `Estimated` total is checked like any other: declining to
                // enforce on one would make the ceiling inoperative for every
                // provider that does not report usage (§5.4).
                let spent = self
                    .session_store
                    .read_task_usage(&session.id)?
                    .get(&task.id)
                    .map(|usage| usage.input_tokens + usage.output_tokens)
                    .unwrap_or(0);
                if spent >= ceiling {
                    let response = token_budget_exhausted_response(ceiling, spent, plan.as_ref());
                    let mut stopped = ModelRun::cancelled_before_start(
                        &self.config.model_provider,
                        &self.config.model_name,
                    );
                    stopped.content = response.clone();
                    break (
                        stopped,
                        response,
                        TurnProposals::default(),
                        StopReason::TokenBudget,
                    );
                }
            }

            let max_rounds = (self.tool_round_limit(web_debug_mode, turn_options)
                + round_budget_extension)
                .min(ABSOLUTE_TOOL_ROUND_CAP);
            let force_final = round >= max_rounds;
            let tools = if force_final {
                None
            } else {
                native_tools.clone()
            };
            let request = ModelRequest {
                provider: self.config.model_provider.clone(),
                model: self.config.model_name.clone(),
                // Bounded, not the raw array: a long turn's tool results would
                // otherwise grow the request without limit (requirement 6).
                messages: bounded_messages(&messages, self.config.agent_max_turn_messages),
                temperature: Some("0".to_string()),
                reasoning_level: Some(self.config.model_reasoning_level.clone()),
                stream: true,
                tools,
                max_tokens: self.config.max_output_tokens(),
                request_usage: self.config.provider_reports_usage(),
                emit_cache_breakpoints: self.config.supports_explicit_cache_breakpoints(),
            };

            let token_estimate: usize = request
                .messages
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
            // `sideEffecting: false` — §4 treats a cut stream as a lost call:
            // the task resumes by making a new one, and spec 19 reports the cost
            // of the lost one rather than hiding it. So a marker left dangling
            // on the cancellation path classifies as `interrupted`, which is the
            // correct answer per §5.1 ("a call may have been billed").
            // The estimate rides on the marker because it must outlive the
            // request: after a crash the request is gone, and recovery would
            // otherwise have only a zero to account a billed call with.
            // Spec 19 §5.5.
            let model_marker = self.session_store.start_action_with_estimate(
                &task,
                "model_call",
                &self.config.model_name,
                false,
                Some(model_request_json(&request).len().div_ceil(4) as u64),
            )?;
            let model_marker_id = model_marker.id().to_string();

            // This round's output, kept here as well as streamed. A stop
            // mid-stream returns `Err(Cancelled)` and no run at all, and these
            // tokens are then the only record that the provider generated —
            // and billed — anything. Spec 19 §5.5.
            let mut round_output = String::new();
            let stream_result = {
                let on_token = &mut *sink.on_token;
                let on_progress = &mut *sink.on_progress;
                let mut accumulate = |token: &str| {
                    round_output.push_str(token);
                    on_token(token);
                };
                // The refusal wait is out-of-band: the token stream stays
                // clean, and the phase carries the remaining seconds so the
                // user watching "retrying in 47s" can stop it. Spec 48 §5.3.
                let mut on_wait = |seconds: u64| {
                    on_progress(TurnProgress::Phase(TurnPhase::new(
                        PhaseKind::Model,
                        format!("Provider refused — retrying in {seconds}s"),
                        round,
                        max_rounds,
                    )));
                };
                model_adapter.stream_response(&request, sink.cancel, &mut accumulate, &mut on_wait)
            };
            let model_run = match stream_result {
                Ok(model_run) => model_run,
                // A stop is not a failure. The transport raises `Cancelled`
                // when it killed the request mid-flight, and it must not be
                // recorded as a provider error.
                Err(ClientError::Cancelled) => {
                    let cancelled_run = ModelRun::cancelled_before_start(
                        &self.config.model_provider,
                        &self.config.model_name,
                    );
                    // The request was sent and the answer was cut short, so
                    // this is not the free case: estimate from what went out
                    // and what came back before the stop.
                    self.session_store.record_task_usage(
                        &task,
                        &cancelled_run.run_id,
                        Some(&model_marker_id),
                        TokenUsage::estimated(
                            model_request_json(&request).len().div_ceil(4) as u64,
                            round_output.len().div_ceil(4) as u64,
                        ),
                        None,
                        Some("stopped_mid_stream"),
                    )?;
                    return self.finish_cancelled_turn(
                        repository_root,
                        session,
                        task,
                        context_files,
                        &partial_response,
                        cancelled_run,
                    );
                }
                Err(ClientError::Provider(refusal, message)) => {
                    // A refusal is a named failure: the marker closes "refused"
                    // and the pre-call estimate is overridden, so a call the
                    // provider rejected before generating anything cannot
                    // inflate the task's reported spend. A refusal that arrived
                    // *mid-stream* is different — the provider streamed and
                    // billed something, so it is estimated from what went out
                    // and what came back, exactly as a cancelled run is
                    // (spec 48 §5.5).
                    self.session_store.finish_action(model_marker, "refused")?;
                    let usage = if round_output.is_empty() {
                        TokenUsage::measured_zero()
                    } else {
                        TokenUsage::estimated(
                            model_request_json(&request).len().div_ceil(4) as u64,
                            round_output.len().div_ceil(4) as u64,
                        )
                    };
                    self.session_store.record_task_usage(
                        &task,
                        &create_id("modelrun"),
                        Some(&model_marker_id),
                        usage,
                        None,
                        Some("refused"),
                    )?;
                    // Requirement 8: every refusal is audited with its
                    // classification — never the request body or the key, so
                    // the fields are ids, names, the typed kind, and the
                    // provider's own message, nothing that was sent.
                    self.audit_log.record(
                        "provider_refusal",
                        &[
                            ("actor", "system".to_string()),
                            ("sessionId", session.id.clone()),
                            ("taskId", task.id.clone()),
                            ("provider", self.config.model_provider.clone()),
                            ("model", self.config.model_name.clone()),
                            ("classification", refusal.code().to_string()),
                            ("message", message.clone()),
                        ],
                    )?;
                    let _ = self.session_store.update_task_status_with_kind(
                        &task,
                        TaskStatus::Failed,
                        Some(message.as_str()),
                        Some(refusal.code()),
                    );
                    return Err(ClientError::Provider(refusal, message));
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
            self.session_store.finish_action(model_marker, "ok")?;

            self.session_store.record_task_usage(
                &task,
                &model_run.run_id,
                Some(&model_marker_id),
                model_run.usage,
                model_run.reported_cost,
                None,
            )?;
            // Requirement 5: an attempt that reached the provider was billed
            // for its input even though its answer never arrived. Same input,
            // no output. Over-counting a connection that failed before the
            // body went out is the deliberate direction of error —
            // under-reporting makes Damaian look cheaper than it is.
            for attempt in 0..model_run.retry_count {
                self.session_store.record_task_usage(
                    &task,
                    &format!("{}_retry{}", model_run.run_id, attempt + 1),
                    Some(&model_marker_id),
                    TokenUsage::estimated(model_run.usage.input_tokens, 0),
                    None,
                    Some("retried_attempt"),
                )?;
            }
            recorded_any_usage = true;

            // The adapter observes this once per provider per process, and
            // has no `AuditLog` of its own — see `ModelRun`. Recording it here
            // is what lets a user set `provider_reports_usage=false` and stop
            // paying for a probe on every process start.
            if model_run.usage_reporting_unsupported {
                self.audit_log.record(
                    "model_usage_reporting_unsupported",
                    &[
                        ("actor", "system".to_string()),
                        ("sessionId", session.id.clone()),
                        ("taskId", task.id.clone()),
                        ("provider", self.config.model_provider.clone()),
                        ("model", self.config.model_name.clone()),
                    ],
                )?;
            }

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
                    // Requirement 6: with a token ceiling set, the round cap is
                    // not the end. The ceiling is the explicit, bounded budget,
                    // so the turn takes another round segment and records that
                    // it did rather than stopping dead. With no ceiling there is
                    // no bound to continue under, so today's `ToolBudget` stop
                    // stands. `round < ABSOLUTE_TOOL_ROUND_CAP` is the safety
                    // net for a provider whose usage never reaches the ceiling.
                    if self.config.agent_max_task_tokens.is_some()
                        && round < ABSOLUTE_TOOL_ROUND_CAP
                    {
                        self.audit_log.record(
                            "turn_continued",
                            &[
                                ("actor", "system".to_string()),
                                ("sessionId", session.id.clone()),
                                ("taskId", task.id.clone()),
                                ("round", round.to_string()),
                                ("roundLimit", max_rounds.to_string()),
                            ],
                        )?;
                        messages.push(
                            ModelMessage::assistant(redacted.clone())
                                .with_reasoning_content(model_run.reasoning_content.clone()),
                        );
                        messages.push(ModelMessage::user(
                            "Your tool-round segment ended and the turn continued under its token ceiling. Continue the task; the ceiling will stop it when it is reached."
                                .to_string(),
                        ));
                        round_budget_extension += self.default_tool_round_limit();
                        round += 1;
                        continue;
                    }
                    let response = tool_budget_exhausted_response(max_rounds);
                    let mut exhausted_run = model_run;
                    exhausted_run.content = response.clone();
                    break (
                        exhausted_run,
                        response,
                        TurnProposals::default(),
                        StopReason::ToolBudget,
                    );
                }
                break (
                    model_run,
                    redacted,
                    TurnProposals::default(),
                    StopReason::Answered,
                );
            }

            let (mut calls_this_round, undecodable_calls) =
                decodable_tool_actions(&model_run.tool_calls, model_run.truncated);
            // The text envelope is the fallback only when no native call
            // decoded, exactly as before: one native call and the envelope is
            // ignored.
            if calls_this_round.is_empty()
                && let Some(command) = parse_command_request(&redacted)
            {
                calls_this_round.push((None, ToolAction::Command(command)));
            }

            if calls_this_round.is_empty() {
                // The model asked for a tool but the request couldn't be
                // decoded. The usual cause is the provider stopping at its
                // output-token ceiling and cutting the `arguments` JSON off
                // mid-string. This used to end the turn silently: the user saw
                // a lead-in like "Let me create all the necessary files:", no
                // patch, and a task marked complete. Feed the failure back
                // instead so the model can retry within the remaining rounds —
                // the same recovery the restricted-path patch arm uses.
                let Some((undecodable, note)) = undecodable_calls.first() else {
                    break (
                        model_run,
                        redacted,
                        TurnProposals::default(),
                        StopReason::Answered,
                    );
                };
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
                    .append_message(&session.id, Some(&task.id), "tool", note)?;
                // Deliberately not echoed back as an `assistant` tool_calls /
                // `tool` pair: the malformed arguments would have to be
                // replayed verbatim, and providers reject a tool result whose
                // call didn't parse. Plain text carries the correction safely.
                messages.push(
                    ModelMessage::assistant(if redacted.trim().is_empty() {
                        summary
                    } else {
                        redacted.clone()
                    })
                    .with_reasoning_content(model_run.reasoning_content.clone()),
                );
                messages.push(ModelMessage::user(note.clone()));
                round += 1;
                continue;
            }

            // Every decoded call runs, in the order the model made them. A
            // terminal arm (a command needing approval, a patch ready for
            // review) ends the turn from inside the loop; a non-terminal one
            // records its result and the loop continues. The break value is
            // carried out in `terminal` because a `break` inside the `for`
            // would only leave the loop, not the turn.
            // An all-read-only round is dispatched concurrently; any other round
            // falls back to sequential, in-order dispatch — which is also what
            // runs the mutating calls one at a time (requirement 8). A single
            // call is never batched: there is nothing to overlap.
            // A stop that arrived before the round starts must not begin a
            // batch of reads; the top-of-loop check finishes the turn.
            let precomputed = if !sink.cancel.is_cancelled()
                && calls_this_round.len() > 1
                && calls_this_round
                    .iter()
                    .all(|(_, action)| action_is_batchable_read_only(action))
            {
                Some(self.run_read_only_batch(
                    repository_root,
                    &session,
                    &task,
                    &calls_this_round,
                    sink.cancel,
                ))
            } else {
                None
            };
            let mut terminal: Option<(ModelRun, String, TurnProposals, StopReason)> = None;
            let mut first_in_round = true;
            for (index, (matched_tool_call, tool_action)) in
                calls_this_round.into_iter().enumerate()
            {
                // A stop between calls of a round ends the batch: no further
                // call starts, and the top of the loop finishes the turn. A
                // read already in flight is not interrupted — it is a bounded
                // local read, and the tools take no cancel token — but none
                // begins after this point.
                if sink.cancel.is_cancelled() {
                    break;
                }
                // The review gate (§5.5). Checked before the action is bracketed,
                // let alone dispatched: there is no marker to finish and nothing
                // to undo, because nothing has happened yet. That is the whole
                // point of putting it here rather than beside the approval card
                // the action would have raised on its own — the user is being
                // asked about the plan, not about this one step, and asking after
                // the first edit had already been prepared would be asking too
                // late to redirect.
                //
                // A turn with no plan is not gated. The gate exists to review a
                // plan, and inventing one to have something to approve would put a
                // panel in front of every trivial question (§5.1).
                if let Some(current) = plan.as_ref()
                    && !plan_approved
                    && action_awaits_plan_review(&tool_action, |command| {
                        self.validation_orchestrator
                            .command_needs_approval(repository_root, command)
                    })
                {
                    let deferred_action = tool_action_label(&tool_action);
                    let response = plan_review_response(current, &deferred_action);
                    let proposal_id = create_id("planreview");
                    self.pending_commands.save(&PendingChatTurn {
                        proposal_id: proposal_id.clone(),
                        session: session.clone(),
                        task: task.clone(),
                        repository_root: repository_root.to_string_lossy().to_string(),
                        context_files: context_files.clone(),
                        round,
                        messages: messages.clone(),
                        // The deferred call is deliberately dropped rather than
                        // stored: it was never dispatched, and the resumed turn
                        // asks the model again rather than replaying a request the
                        // user may have just revised out of the plan.
                        matched_tool_call: None,
                        last_content: redacted.clone(),
                        turn_options,
                        reasoning_content: model_run.reasoning_content.clone(),
                        mcp_call: None,
                        web_diagnostic_call: None,
                        plan_review: Some(PendingPlanReview {
                            deferred_action: deferred_action.clone(),
                        }),
                    })?;
                    self.note_pending_approvals(
                        &session,
                        &task,
                        vec![PendingApproval {
                            kind: "plan".to_string(),
                            proposal_id: proposal_id.clone(),
                        }],
                    );
                    let mut proposal_run = model_run;
                    proposal_run.content = response.clone();
                    terminal = Some((
                        proposal_run,
                        response,
                        TurnProposals {
                            plan: Some(AgentPlanProposal {
                                id: proposal_id,
                                plan: current.clone(),
                                deferred_action,
                            }),
                            ..Default::default()
                        },
                        StopReason::Answered,
                    ));
                    break;
                }

                if matches!(tool_action, ToolAction::WebDiagnostic(_)) {
                    web_debug_mode = true;
                }
                let max_rounds = (self.tool_round_limit(web_debug_mode, turn_options)
                    + round_budget_extension)
                    .min(ABSOLUTE_TOOL_ROUND_CAP);
                sink.phase(
                    PhaseKind::Tool,
                    tool_action_label(&tool_action),
                    round,
                    max_rounds,
                );

                // Bracket the dispatch, not the leaf call: `ValidationOrchestrator`
                // and `McpClient` hold no `SessionStore` and receive no `Task`, so
                // marking inside them would mean threading session state through
                // two modules that have nothing to do with it. The window is
                // slightly wider than the leaf action — it includes proposal and
                // setup — which errs toward reporting an unknown outcome rather
                // than missing one, the safe direction for requirement 5.
                let (marker_action, marker_ref, marker_side_effecting) =
                    tool_action_marker(&tool_action);
                let action_marker = self.session_store.start_action(
                    &task,
                    marker_action,
                    &marker_ref,
                    marker_side_effecting,
                )?;

                // Each non-terminal arm below produces the (assistant summary,
                // tool result, outcome) triple to persist and feed back to the
                // model. Terminal outcomes (a command needing approval, or a patch
                // ready for review) `break` the loop directly instead, since
                // both always require the human before anything continues.
                //
                // The third element is what the tool *reported*, which is not the
                // same as whether dispatching it worked. Every arm used to converge
                // on `finish_action(marker, "ok")`, so a command that exited
                // non-zero was indistinguishable in the log from one that passed —
                // and spec 18's `tool_and_model_error_rate` consequently read 0.000
                // by construction. Spec 21 requirement 6 reads a step's status from
                // this value, so it has to be the tool's answer, not the
                // dispatcher's.
                let (assistant_summary, tool_result_text, action_outcome) =
                    if action_is_batchable_read_only(&tool_action) {
                        match precomputed.as_ref() {
                            // Precomputed by the concurrent batch, indexed by the
                            // model's call order.
                            Some(results) => results[index].clone(),
                            None => self.dispatch_read_only_action(
                                repository_root,
                                &session,
                                &task,
                                &tool_action,
                            ),
                        }
                    } else {
                        match tool_action {
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
                                plan_review: None,
                            })?;
                            self.note_pending_approvals(
                                &session,
                                &task,
                                vec![PendingApproval {
                                    kind: "command".to_string(),
                                    proposal_id: proposal.id.clone(),
                                }],
                            );
                            // A clean stop for a human decision, not a crash: the
                            // action is finished so the classifier does not read a
                            // dangling marker as an unknown outcome.
                            self.session_store
                                .finish_action(action_marker, "awaiting_approval")?;
                            let mut proposal_run = model_run;
                            proposal_run.content = response.clone();
                            terminal = Some((
                                proposal_run,
                                response,
                                TurnProposals {
                                    command: Some(agent_command_proposal(&self.config, &proposal)),
                                    ..Default::default()
                                },
                                StopReason::Answered,
                            ));
                            break;
                        }

                        let cancel = sink.cancel;
                        // Borrow only the progress field, so `cancel` can be read
                        // for the same call without two conflicting borrows of the
                        // sink.
                        let on_progress = &mut *sink.on_progress;
                        let mut on_output = |line: &str| {
                            on_progress(TurnProgress::Phase(TurnPhase::new(
                                PhaseKind::Output,
                                line,
                                round,
                                max_rounds,
                            )));
                        };
                        let record = self.validation_orchestrator.run_proposal(
                            &proposal.id,
                            false,
                            "sandbox",
                            Some(&task.id),
                            cancel,
                            &mut on_output,
                        )?;
                        let command_context = sandbox_command_context(&record.execution);
                        // The exit code is in hand right here, one statement before
                        // the marker is finished. Nothing downstream can recover it
                        // — `CommandExecution` is never persisted to the session log
                        // — so it is carried out of the arm rather than looked up.
                        let exit_code = record.execution.exit_code;
                        (
                            tool_call_summary(&command_request),
                            command_context,
                            ActionOutcome::CommandExit(exit_code),
                        )
                    }
                    ToolAction::ProposePatch(generated_edit) => {
                        match self.patch_engine.create_patch(
                            repository_root,
                            &generated_edit.changes,
                            Some(&task.id),
                            &generated_edit.summary,
                        ) {
                            Ok(patch) => {
                                // See `ProposedPatch::session_id`: the engine does
                                // not know the session, this orchestrator does.
                                let mut patch = patch;
                                patch.session_id = session.id.clone();
                                self.patch_store.save(&patch)?;
                                let response = patch_proposal_response(&patch);
                                // A patch waiting for review is a clean stop, not
                                // a crash — finish the marker before breaking.
                                self.session_store
                                    .finish_action(action_marker, "awaiting_review")?;
                                let proposal = agent_patch_proposal(&patch);
                                let mut proposal_run = model_run;
                                proposal_run.content = response.clone();
                                terminal = Some((
                                    proposal_run,
                                    response,
                                    TurnProposals {
                                        patch: Some(proposal),
                                        ..Default::default()
                                    },
                                    StopReason::Answered,
                                ));
                                break;
                            }
                            // Fed back as a tool result rather than aborting the
                            // turn, so the model can see why (e.g. a restricted
                            // or out-of-repo path) and correct itself within the
                            // remaining rounds instead of the turn just failing.
                            Err(error) => (
                                format!("Attempted to propose a patch: {}", generated_edit.summary),
                                format!("Cannot propose that patch: {error}"),
                                ActionOutcome::Failed,
                            ),
                        }
                    }
                    ToolAction::ProposePlan(steps) => {
                        if plan.is_some() {
                            // Refused rather than replaced. Steps already carry
                            // evidence tied to a state of the repository, and
                            // rewriting the plan underneath that evidence produces
                            // a history that no longer describes what happened —
                            // the same reason §5.5 rules out mid-execution edits.
                            (
                            "Attempted to propose a second plan.".to_string(),
                            "This turn already has a plan. Work through its remaining steps with complete_step, or stop and start a new turn if the plan is wrong.".to_string(),
                            ActionOutcome::Failed,
                        )
                        } else {
                            let now = now_millis();
                            let mut proposed = crate::plan::TaskPlan::new(&task.id, now);
                            for (index, step) in steps.iter().enumerate() {
                                proposed.steps.push(crate::plan::PlanStep {
                                    id: format!("step_{}", index + 1),
                                    // Model-authored text, redacted like any other:
                                    // a title is rendered in the panel and written
                                    // to the log, so a secret echoed into one must
                                    // not survive there.
                                    title: self.scanner.redact(&step.title).text,
                                    detail: step
                                        .detail
                                        .as_ref()
                                        .map(|detail| self.scanner.redact(detail).text),
                                    // The engine's to set, not the model's.
                                    status: if index == 0 {
                                        crate::plan::StepStatus::InProgress
                                    } else {
                                        crate::plan::StepStatus::Pending
                                    },
                                    depends_on: Vec::new(),
                                    started_at_ms: (index == 0).then_some(now),
                                    completed_at_ms: None,
                                    evidence: Vec::new(),
                                });
                            }
                            self.session_store.create_plan(&task, &proposed)?;
                            sink.plan(&proposed);
                            let summary = format!("Planned {} steps.", proposed.steps.len());
                            let first = proposed.steps[0].title.clone();
                            plan = Some(proposed);
                            (
                                summary,
                                format!(
                                    "Plan recorded. The current step is: {first}. Call complete_step when its work is done."
                                ),
                                ActionOutcome::Ok,
                            )
                        }
                    }
                    ToolAction::CompleteStep => match plan.as_mut() {
                        None => (
                            "Attempted to complete a step.".to_string(),
                            "There is no plan for this turn, so there is no step to complete."
                                .to_string(),
                            ActionOutcome::Failed,
                        ),
                        Some(current) => {
                            // The model asked to move on; it does not get to say
                            // how the step ended. §5.3: the status is a function
                            // of the evidence, and this is the only place a step
                            // reaches a terminal status.
                            let accrued = std::mem::take(&mut step_evidence);
                            let now = now_millis();
                            let mut finished_title = String::new();
                            let mut status = crate::plan::StepStatus::Completed;
                            if let Some(open) = current
                                .steps
                                .iter_mut()
                                .find(|step| step.status == crate::plan::StepStatus::InProgress)
                            {
                                // Extends rather than replaces. A step can already
                                // carry evidence this turn never saw: a patch
                                // applied after the turn that proposed it appends
                                // through the log (`edit.rs`), and a resumed plan
                                // arrives with everything its earlier turns
                                // recorded. Assigning here would silently drop
                                // both, and the step would then be judged on a
                                // fraction of what is known about it.
                                open.evidence.extend(accrued);
                                status = crate::plan::status_from_evidence(&open.evidence);
                                open.status = status;
                                open.completed_at_ms = Some(now);
                                finished_title = open.title.clone();
                                let closed = open.clone();
                                self.session_store.update_plan_step(&task, &closed)?;
                            }

                            // A blocked step does not hand off: the next step's
                            // prerequisite failed, and starting it anyway would
                            // build on work that did not happen.
                            let mut next_title = None;
                            if status == crate::plan::StepStatus::Completed
                                && let Some(next) = current
                                    .steps
                                    .iter_mut()
                                    .find(|step| step.status == crate::plan::StepStatus::Pending)
                            {
                                next.status = crate::plan::StepStatus::InProgress;
                                next.started_at_ms = Some(now);
                                next_title = Some(next.title.clone());
                                let started = next.clone();
                                self.session_store.update_plan_step(&task, &started)?;
                            }
                            // Once, after the handoff rather than after each of its
                            // two writes: between them the closing step is already
                            // terminal and the next has not opened, so a panel
                            // updated mid-handoff would blink through a state with
                            // no current step. The log keeps both writes; the panel
                            // does not need them.
                            sink.plan(current);

                            let result = match (status, &next_title) {
                                (crate::plan::StepStatus::Blocked, _) => format!(
                                    "Step \"{finished_title}\" is blocked: a command it ran did not succeed. Fix that before moving on; the remaining steps are still pending."
                                ),
                                (_, Some(next)) => format!(
                                    "Step \"{finished_title}\" is complete. The current step is now: {next}."
                                ),
                                (_, None) => format!(
                                    "Step \"{finished_title}\" is complete. That was the last step."
                                ),
                            };
                            (
                                format!("Finished: {finished_title}"),
                                result,
                                ActionOutcome::Ok,
                            )
                        }
                    },
                    ToolAction::EditFile { summary, edits } => {
                        match region_edits_to_changes(repository_root, &self.path_policy, &edits)
                            .and_then(|changes| {
                                self.patch_engine.create_patch(
                                    repository_root,
                                    &changes,
                                    Some(&task.id),
                                    &summary,
                                )
                            }) {
                            Ok(patch) => {
                                // See `ProposedPatch::session_id`: the engine does
                                // not know the session, this orchestrator does.
                                let mut patch = patch;
                                patch.session_id = session.id.clone();
                                self.patch_store.save(&patch)?;
                                let response = patch_proposal_response(&patch);
                                // A patch waiting for review is a clean stop, not
                                // a crash — finish the marker before breaking.
                                self.session_store
                                    .finish_action(action_marker, "awaiting_review")?;
                                let proposal = agent_patch_proposal(&patch);
                                let mut proposal_run = model_run;
                                proposal_run.content = response.clone();
                                terminal = Some((
                                    proposal_run,
                                    response,
                                    TurnProposals {
                                        patch: Some(proposal),
                                        ..Default::default()
                                    },
                                    StopReason::Answered,
                                ));
                                break;
                            }
                            // Fed back as a tool result rather than aborting the
                            // turn, so the model can see why (a stale anchor, a
                            // restricted path) and correct itself within the
                            // remaining rounds.
                            Err(error) => (
                                format!("Attempted to edit files: {summary}"),
                                format!("Cannot apply that edit: {error}"),
                                ActionOutcome::Failed,
                            ),
                        }
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
                                plan_review: None,
                            })?;
                            self.note_pending_approvals(
                                &session,
                                &task,
                                vec![PendingApproval {
                                    kind: "browser_diagnostic".to_string(),
                                    proposal_id: proposal.id.clone(),
                                }],
                            );
                            // A clean stop for a human decision, not a crash: the
                            // action is finished so the classifier does not read a
                            // dangling marker as an unknown outcome.
                            self.session_store
                                .finish_action(action_marker, "awaiting_approval")?;
                            let mut proposal_run = model_run;
                            proposal_run.content = response.clone();
                            terminal = Some((
                                proposal_run,
                                response,
                                TurnProposals {
                                    command: Some(proposal),
                                    ..Default::default()
                                },
                                StopReason::Answered,
                            ));
                            break;
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
                        let (content, outcome) = if failed_browser_calls
                            .get(&signature)
                            .copied()
                            .unwrap_or_default()
                            >= retry_limit
                        {
                            // Refused rather than attempted, because the same call
                            // has already failed its retry limit. Still a failure:
                            // the tool produced no diagnostic.
                            (browser_retry_limit_note(retry_limit), ActionOutcome::Failed)
                        } else {
                            let report = self.run_web_diagnostic_report(&call);
                            let content = self.format_web_diagnostic_result(report);
                            let failed = browser_tool_result_failed(&content);
                            if failed {
                                *failed_browser_calls.entry(signature).or_insert(0) += 1;
                            }
                            let outcome = if failed {
                                ActionOutcome::Failed
                            } else {
                                ActionOutcome::Ok
                            };
                            (content, outcome)
                        };
                        (web_diagnostic_summary(&call), content, outcome)
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
                                plan_review: None,
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
                            terminal = Some((
                                proposal_run,
                                response,
                                TurnProposals {
                                    command: Some(proposal),
                                    ..Default::default()
                                },
                                StopReason::Answered,
                            ));
                            break;
                        }

                        // No approval required: run it now and feed the result back.
                        let summary = mcp_call_summary(&server_id, &tool_name);
                        let (content, outcome) =
                            match mcp.call_tool(&server_id, &tool_name, &arguments_json) {
                                Ok(result) => {
                                    let text = self.scanner.redact(&result.text).text;
                                    // `is_error` is the server's own verdict on its
                                    // call. Reaching the server is not the same as
                                    // the call working, and only the server knows
                                    // which happened.
                                    if result.is_error {
                                        (
                                            format!("MCP tool reported an error:\n{text}"),
                                            ActionOutcome::Failed,
                                        )
                                    } else {
                                        (text, ActionOutcome::Ok)
                                    }
                                }
                                Err(error) => (
                                    format!("MCP tool call failed: {error}"),
                                    ActionOutcome::Failed,
                                ),
                            };
                        (summary, content, outcome)
                    }
                        other => unreachable!(
                            "read-only action reached the sequential match: {other:?}"
                        ),
                    }
                    };
                // Defensive: the top-of-loop check normally catches a stop
                // before the marker is even started, so a cancelled batch result
                // is rare. If one arrives, close the marker cleanly and let the
                // turn end cancelled rather than record a result for a call that
                // did nothing.
                if action_outcome == ActionOutcome::Cancelled {
                    self.session_store
                        .finish_action(action_marker, "cancelled")?;
                    break;
                }
                // Accrued before the marker is consumed, since the marker id is
                // what ties this evidence back to the action in the log. Evidence
                // belongs to whichever step is open; with no plan there is nothing
                // to attach it to and this is a no-op.
                if plan.is_some()
                    && let Some(evidence) = evidence_for(&action_outcome, action_marker.id())
                {
                    step_evidence.push(evidence);
                }
                // One finish per dispatch, on the tool's own answer. A command goes
                // through `finish_command_action` so the outcome is derived from
                // the exit code rather than passed beside it — the two cannot then
                // disagree in the log.
                match action_outcome {
                    ActionOutcome::CommandExit(exit_code) => self
                        .session_store
                        .finish_command_action(action_marker, exit_code)?,
                    // A successful read is an `ok` dispatch like any other; the
                    // path and hash it carries are for the step's evidence, not
                    // for the marker.
                    ActionOutcome::Ok | ActionOutcome::FileRead { .. } => {
                        self.session_store.finish_action(action_marker, "ok")?
                    }
                    ActionOutcome::Failed => {
                        self.session_store.finish_action(action_marker, "failed")?
                    }
                    // Unreachable after the early break above, kept so the
                    // match stays exhaustive if that guard ever changes.
                    ActionOutcome::Cancelled => self
                        .session_store
                        .finish_action(action_marker, "cancelled")?,
                }

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

                // Feed this call back before the next one in the round, so a
                // terminal break still carries everything that already ran.
                // Only the first assistant message of a round repeats the
                // model's prose and reasoning; a later one would duplicate
                // them.
                if let Some(call) = &matched_tool_call {
                    let content = if first_in_round {
                        redacted.clone()
                    } else {
                        String::new()
                    };
                    let reasoning = if first_in_round {
                        model_run.reasoning_content.clone()
                    } else {
                        None
                    };
                    messages.push(ModelMessage::assistant_with_tool_calls(
                        content,
                        vec![call.clone()],
                        reasoning,
                    ));
                    messages.push(ModelMessage::tool(
                        call.id.clone(),
                        tool_result_text.clone(),
                    ));
                } else {
                    // Only reachable for `ToolAction::Command` via the
                    // `DAMAIAN_COMMAND_V1` text-envelope fallback — every other
                    // action only exists as a native tool call.
                    messages.push(
                        ModelMessage::assistant(redacted.clone())
                            .with_reasoning_content(model_run.reasoning_content.clone()),
                    );
                    messages.push(ModelMessage::user(format!(
                        "Command result:\n{tool_result_text}"
                    )));
                }
                first_in_round = false;
            }

            if let Some(finished) = terminal {
                break finished;
            }

            // A call in the same round that could not be decoded is reported
            // after the ones that ran, as plain text: its arguments cannot be
            // replayed and providers reject a tool result whose call never
            // parsed.
            if let Some((undecodable, note)) = undecodable_calls.first() {
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
                    .append_message(&session.id, Some(&task.id), "tool", note)?;
                messages.push(ModelMessage::assistant(summary));
                messages.push(ModelMessage::user(note.clone()));
            }

            round += 1;
        };

        self.session_store
            .append_message(&session.id, Some(&task.id), "assistant", &response)?;
        // A turn that ends awaiting a decision records *which* proposal, so a
        // restart reattaches it instead of rebuilding a card from partial data
        // (§5.5).
        let pending = proposals
            .command
            .as_ref()
            .map(|proposal| crate::session::PendingApprovalRef {
                kind: "command".to_string(),
                proposal_id: proposal.id.clone(),
            })
            .or_else(|| {
                proposals
                    .patch
                    .as_ref()
                    .map(|proposal| crate::session::PendingApprovalRef {
                        kind: "patch".to_string(),
                        proposal_id: proposal.patch_id.clone(),
                    })
            })
            .or_else(|| {
                proposals
                    .plan
                    .as_ref()
                    .map(|proposal| crate::session::PendingApprovalRef {
                        kind: "plan".to_string(),
                        proposal_id: proposal.id.clone(),
                    })
            });
        task = match &pending {
            Some(pending) => self.session_store.await_approval(&task, pending)?,
            None => {
                let final_status = match stop_reason {
                    StopReason::ToolBudget => TaskStatus::ToolBudgetExhausted,
                    StopReason::TokenBudget => TaskStatus::TokenBudgetExhausted,
                    StopReason::Answered => TaskStatus::Complete,
                };
                self.session_store
                    .update_task_status(&task, final_status, None)?
            }
        };
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
                    if proposals.command.is_some() {
                        "command_approval_required".to_string()
                    } else if proposals.patch.is_some() {
                        "patch_proposal_ready".to_string()
                    } else if proposals.plan.is_some() {
                        "plan_review_required".to_string()
                    } else if stop_reason == StopReason::ToolBudget {
                        "tool_budget_exhausted".to_string()
                    } else if stop_reason == StopReason::TokenBudget {
                        "token_budget_exhausted".to_string()
                    } else if round > 0 {
                        "complete_with_sandbox_command".to_string()
                    } else {
                        "complete".to_string()
                    },
                ),
            ],
        )?;
        // A turn waiting on a command decision is not over: it resumes through
        // `resume_after_command_decision` and seals then. A plan review is the
        // same shape — nothing has happened yet and the turn continues through
        // `resume_after_plan_decision`. Anything else has left the repository
        // in the state a rewind must compare against.
        if proposals.command.is_none() && proposals.plan.is_none() {
            self.seal_turn_checkpoint(repository_root, &session, &task);
        }
        // From the log, not from a running total: the number the client shows
        // now must be the number it shows after a reload.
        let usage = self
            .session_store
            .read_task_usage(&session.id)
            .ok()
            .and_then(|usage| usage.get(&task.id).copied());
        let estimated_cost = usage.as_ref().and_then(|usage| {
            self.config.estimated_cost(&TokenUsage {
                input_tokens: usage.input_tokens,
                output_tokens: usage.output_tokens,
                cached_input_tokens: usage.cached_input_tokens,
                source: usage.source,
            })
        });
        Ok(ChatTurnResult {
            session,
            task,
            model_run: final_run,
            context_files,
            response,
            command_proposal: proposals.command,
            patch_proposal: proposals.patch,
            plan_proposal: proposals.plan,
            cancelled: false,
            usage,
            estimated_cost,
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
        let usage = self
            .session_store
            .read_task_usage(&session.id)
            .ok()
            .and_then(|usage| usage.get(&task.id).copied());
        let estimated_cost = usage.as_ref().and_then(|usage| {
            self.config.estimated_cost(&TokenUsage {
                input_tokens: usage.input_tokens,
                output_tokens: usage.output_tokens,
                cached_input_tokens: usage.cached_input_tokens,
                source: usage.source,
            })
        });
        Ok(ChatTurnResult {
            session,
            task,
            model_run,
            context_files,
            response,
            command_proposal: None,
            patch_proposal: None,
            plan_proposal: None,
            cancelled: true,
            usage,
            estimated_cost,
        })
    }

    /// Dispatch one of the read-only tools. Shared by the sequential path (a
    /// lone call in a round) and [`Self::run_read_only_batch`], so the two
    /// cannot drift. The returned triple is what the turn records.
    fn dispatch_read_only_action(
        &self,
        repository_root: &Path,
        session: &Session,
        task: &Task,
        action: &ToolAction,
    ) -> (String, String, ActionOutcome) {
        match action {
            ToolAction::ReadFile { path, range } => {
                let window = match range {
                    Some(range) => ReadWindow::Range(*range),
                    None => ReadWindow::Default,
                };
                let (content, outcome) = match self.file_access.read_file(
                    repository_root,
                    path,
                    Some(&task.id),
                    Some(&session.repository_id),
                    false,
                    false,
                    window,
                ) {
                    Ok(file_read) => {
                        let mut content = format!(
                            "Content of {}, lines {}–{} of {}:",
                            file_read.path,
                            file_read.line_range.start,
                            file_read.line_range.end,
                            file_read.total_lines,
                        );
                        if let Some(reason) = &file_read.truncated_by {
                            content.push_str(&format!(
                                " (truncated by {reason}; ask for a narrower range)"
                            ));
                        }
                        content.push_str(&format!("\n{}", file_read.content));
                        (
                            content,
                            ActionOutcome::FileRead {
                                path: file_read.path.clone(),
                                hash: file_read.hash.clone(),
                            },
                        )
                    }
                    Err(error) => (
                        format!("Cannot read {path}: {error}"),
                        ActionOutcome::Failed,
                    ),
                };
                (format!("Read `{path}`"), content, outcome)
            }
            ToolAction::ListDirectory { dir, depth } => {
                match self.navigation.list_directory(
                    repository_root,
                    dir.as_deref(),
                    *depth,
                    Some(&task.id),
                    Some(&session.repository_id),
                ) {
                    Ok(listing) => {
                        let dir_name = dir.as_deref().unwrap_or(".");
                        let content = if listing.truncated {
                            format!(
                                "Listing of {dir_name} ({} of {} paths):\n{}",
                                listing.paths.len(),
                                listing.total_found,
                                listing.paths.join("\n")
                            )
                        } else {
                            format!(
                                "Listing of {dir_name} ({} paths):\n{}",
                                listing.total_found,
                                listing.paths.join("\n")
                            )
                        };
                        (format!("Listed {dir_name}"), content, ActionOutcome::Ok)
                    }
                    Err(error) => (
                        "Attempted to list the directory.".to_string(),
                        format!("Cannot list that directory: {error}"),
                        ActionOutcome::Failed,
                    ),
                }
            }
            ToolAction::SearchContent {
                pattern,
                path_glob,
                max_matches,
            } => {
                match self.navigation.search_content(
                    repository_root,
                    pattern,
                    path_glob.as_deref(),
                    *max_matches,
                    Some(&task.id),
                    Some(&session.repository_id),
                ) {
                    Ok(found) => {
                        let mut lines = found
                            .matches
                            .iter()
                            .map(|m| format!("{}:{}: {}", m.path, m.line, m.text))
                            .collect::<Vec<_>>()
                            .join("\n");
                        if !lines.is_empty() {
                            lines.push('\n');
                        }
                        let content = format!(
                            "{} matches for \"{}\" across {} files (showing {}):\n{}",
                            found.total_found,
                            pattern,
                            found.files_searched,
                            found.matches.len(),
                            lines,
                        );
                        (
                            format!("Searched for \"{pattern}\""),
                            content,
                            ActionOutcome::Ok,
                        )
                    }
                    Err(error) => (
                        "Attempted to search the repository.".to_string(),
                        format!("Cannot search: {error}"),
                        ActionOutcome::Failed,
                    ),
                }
            }
            ToolAction::SearchCodebase {
                query,
                semantic,
                limit,
            } => {
                let index = match crate::index_cache::IndexCache::get_or_build(
                    &self.indexer,
                    repository_root,
                ) {
                    Ok(index) => index,
                    Err(error) => {
                        return (
                            format!("Searched codebase for \"{query}\""),
                            format!("Cannot search the codebase: {error}"),
                            ActionOutcome::Failed,
                        );
                    }
                };
                let results = if *semantic {
                    if self.config.enable_semantic_search {
                        VectorIndexCache::semantic_search(
                            &self.config.data_dir,
                            &index,
                            query,
                            *limit,
                        )
                    } else {
                        index.semantic_search(query, *limit)
                    }
                } else {
                    index.keyword_search(query, *limit)
                };
                (
                    format!("Searched codebase for \"{query}\""),
                    format_search_results(&results),
                    // A search that matched nothing still ran. "No results"
                    // is an answer, not a failure.
                    ActionOutcome::Ok,
                )
            }
            ToolAction::ReadGitStatus => {
                let (content, outcome) = match self.git.status(repository_root) {
                    Ok(status) => (format_git_status(&status), ActionOutcome::Ok),
                    Err(error) => (
                        format!("Cannot read git status: {error}"),
                        ActionOutcome::Failed,
                    ),
                };
                ("Checked git status".to_string(), content, outcome)
            }
            ToolAction::ReadGitDiff { staged } => {
                let (content, outcome) = match self.git.diff(repository_root, *staged) {
                    Ok(diff) if diff.trim().is_empty() => {
                        ("No differences.".to_string(), ActionOutcome::Ok)
                    }
                    Ok(diff) => (diff, ActionOutcome::Ok),
                    Err(error) => (
                        format!("Cannot read git diff: {error}"),
                        ActionOutcome::Failed,
                    ),
                };
                (
                    format!("Read git diff{}", if *staged { " (staged)" } else { "" }),
                    content,
                    outcome,
                )
            }
            other => unreachable!("not a read-only action: {other:?}"),
        }
    }

    /// Run an all-read-only round's calls concurrently, collecting results in
    /// the model's call order. `thread::scope` plus joining in order is the
    /// determinism: whichever thread finishes first, result `i` is the one the
    /// model asked for at position `i`, so the recorded log is identical to the
    /// sequential one (`context.md` §3 on why this is the primitive).
    fn run_read_only_batch(
        &self,
        repository_root: &Path,
        session: &Session,
        task: &Task,
        actions: &[DecodedCall],
        cancel: &CancelToken,
    ) -> Vec<(String, String, ActionOutcome)> {
        std::thread::scope(|scope| {
            let handles: Vec<_> = actions
                .iter()
                .map(|(_, action)| {
                    scope.spawn(move || {
                        // Requirement 8: a stop cancels every call that has not
                        // started. A read already in flight is not interrupted
                        // — it is a bounded local read and takes no cancel token
                        // — but no call begins after this check.
                        if cancel.is_cancelled() {
                            return (String::new(), String::new(), ActionOutcome::Cancelled);
                        }
                        self.dispatch_read_only_action(repository_root, session, task, action)
                    })
                })
                .collect();
            handles
                .into_iter()
                .map(|handle| handle.join().expect("read-only tool thread panicked"))
                .collect()
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
    /// Present when the pause is a plan review rather than an action approval
    /// (spec 21 §5.5). Nothing was dispatched, so there is no call to carry —
    /// only what the turn was about to do, which the review card shows and the
    /// resumed turn puts back in front of the model. `#[serde(default)]` keeps
    /// pending turns written before the gate existed loadable.
    #[serde(default)]
    plan_review: Option<PendingPlanReview>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct PendingPlanReview {
    deferred_action: String,
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

/// A read-only view of the turns [`ChatOrchestrator`] has parked on disk.
///
/// Crash recovery needs one fact out of a paused turn — what a plan review is
/// holding back — and needs it without an orchestrator, which takes thirteen
/// dependencies to build and belongs to a repository rather than to the
/// data directory recovery sweeps. Read-only on purpose: *resuming* a paused
/// turn stays the orchestrator's, and nothing here can take one.
///
/// It parses the file as loose JSON rather than as [`PendingChatTurn`], so a
/// pending turn written by a different version still answers this question
/// instead of failing to deserialise and looking like an absent one.
#[derive(Debug, Clone)]
pub struct PausedTurns {
    data_dir: PathBuf,
}

impl PausedTurns {
    pub fn new(data_dir: impl AsRef<Path>) -> Self {
        Self {
            data_dir: data_dir.as_ref().to_path_buf(),
        }
    }

    /// What a paused plan review is holding back, or `None` when the id names
    /// no paused turn, or one that is not a plan review.
    ///
    /// `None` is the signal that a plan review cannot be re-presented: the
    /// turn behind it is gone, so approving the plan would have nothing to
    /// continue.
    pub fn plan_review_deferred_action(&self, proposal_id: &str) -> Option<String> {
        let path = PendingCommandStore::new(&self.data_dir).path_for(proposal_id);
        let content = fs::read_to_string(path).ok()?;
        let value: serde_json::Value = serde_json::from_str(&content).ok()?;
        value
            .get("plan_review")?
            .get("deferred_action")?
            .as_str()
            .map(str::to_string)
    }
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

/// Clamp the within-turn message array to a bounded window, stating what was
/// elided.
///
/// The first two messages are the initial system prompt and the user request
/// and are always kept. The rest are call/result pairs — an assistant message
/// and its tool results — and are dropped from the oldest end in whole pairs, so
/// a tool result is never left without the call it answers. The session log
/// still holds every round; only the request is bounded. Spec 47 requirement 6,
/// the half `OBSERVATIONS.md` entry 6 names.
fn bounded_messages(messages: &[ModelMessage], max_messages: usize) -> Vec<ModelMessage> {
    if messages.len() <= max_messages {
        return messages.to_vec();
    }
    // The head and the notice are never dropped, so the effective floor is
    // three messages whatever a repository sets.
    let head = messages[..2].to_vec();
    let tail = &messages[2..];
    let budget = max_messages.saturating_sub(3);
    let mut drop_count = tail.len().saturating_sub(budget);
    // Whole pairs: an odd drop would leave a tool result without its call.
    if !drop_count.is_multiple_of(2) {
        drop_count += 1;
    }
    let drop_count = drop_count.min(tail.len());
    let mut bounded = head;
    bounded.push(ModelMessage::system(format!(
        "Earlier rounds of this turn were elided to bound the request: {drop_count} messages ({} call results) omitted. The session log still holds them; repeat a read if you need it.",
        drop_count / 2
    )));
    bounded.extend_from_slice(&tail[drop_count..]);
    bounded
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CommandRequest {
    command: String,
    reason: String,
}

/// One step as the model proposed it. Only a title and an optional detail: the
/// status, the timings and the evidence are the engine's to write, and letting
/// the model supply them would hand it the very field requirement 6 exists to
/// keep out of its reach.
#[derive(Debug, Clone, PartialEq, Eq)]
struct ProposedStep {
    title: String,
    detail: Option<String>,
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
    /// The model's plan for this turn. §5.1: a turn is non-trivial when the
    /// model proposes more than one step for it, so this call *is* the
    /// triviality decision rather than a guess made before the turn starts.
    ProposePlan(Vec<ProposedStep>),
    /// The model asks to move on from the current step. It does not say how
    /// the step ended — the engine derives that from the evidence accrued
    /// while the step was in progress (§5.3).
    CompleteStep,
    ReadFile {
        path: String,
        range: Option<LineRange>,
    },
    ListDirectory {
        dir: Option<String>,
        depth: Option<usize>,
    },
    SearchContent {
        pattern: String,
        path_glob: Option<String>,
        max_matches: Option<usize>,
    },
    EditFile {
        summary: String,
        edits: Vec<RegionEdit>,
    },
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

/// What dispatching a tool can change beyond its own result.
///
/// The one place that question is answered. Spec 17's recovery reads the
/// [`ActionEffect::WritesRepository`] answer through [`tool_action_marker`]'s
/// boolean; requirement 8's batching reads [`ActionEffect::ReadOnly`] through
/// [`action_is_batchable_read_only`]. Both come off one exhaustive match, so a
/// new tool cannot become batchable or side-effecting by omission, and there is
/// no second list to keep in sync with spec 17.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ActionEffect {
    /// Reads repository or session state and returns it. No write, no turn
    /// control, so it is safe to run beside another read.
    ReadOnly,
    /// Can leave a side effect outside the session log. A crash mid-flight
    /// leaves an unknown outcome, and it must not run concurrently.
    WritesRepository,
    /// Writes nothing outside the session log, but advances or ends the turn:
    /// planning, completing a step, proposing a patch or an edit. Safe for
    /// crash recovery, not a candidate for batching.
    ShapesTurn,
}

fn action_effect(action: &ToolAction) -> ActionEffect {
    match action {
        ToolAction::Command(_) | ToolAction::WebDiagnostic(_) | ToolAction::McpCall { .. } => {
            ActionEffect::WritesRepository
        }
        ToolAction::ProposePatch(_)
        | ToolAction::ProposePlan(_)
        | ToolAction::CompleteStep
        | ToolAction::EditFile { .. } => ActionEffect::ShapesTurn,
        ToolAction::ReadFile { .. }
        | ToolAction::ListDirectory { .. }
        | ToolAction::SearchContent { .. }
        | ToolAction::SearchCodebase { .. }
        | ToolAction::ReadGitStatus
        | ToolAction::ReadGitDiff { .. } => ActionEffect::ReadOnly,
    }
}

/// Whether a tool may be dispatched alongside another in the same round.
/// Derived from [`action_effect`] rather than declared beside it, so there is
/// no second list to drift.
fn action_is_batchable_read_only(action: &ToolAction) -> bool {
    matches!(action_effect(action), ActionEffect::ReadOnly)
}

/// The action marker parameters for a dispatched tool: a stable name, a
/// reference identifying *which* invocation, and whether a crash mid-flight
/// could have left a side effect.
///
/// Derived here rather than at each match arm so the side-effect answer for a
/// tool exists in exactly one place. `ProposePatch` is **not** side-effecting:
/// proposing writes nothing to the repository. Applying one is, and is bracketed
/// separately in `edit.rs`.
fn tool_action_marker(action: &ToolAction) -> (&'static str, String, bool) {
    // Spec 17's answer, unchanged in value: turn-shaping writes are confined to
    // the append-only session log, so a crash leaves `interrupted`, not
    // `unknown_external_outcome`.
    let side_effecting = matches!(action_effect(action), ActionEffect::WritesRepository);
    match action {
        ToolAction::Command(request) => ("run_command", request.command.clone(), side_effecting),
        ToolAction::ProposePatch(edit) => ("propose_patch", edit.summary.clone(), side_effecting),
        ToolAction::ProposePlan(steps) => ("propose_plan", steps.len().to_string(), side_effecting),
        ToolAction::CompleteStep => ("complete_step", String::new(), side_effecting),
        ToolAction::ReadFile { path, .. } => ("read_file", path.clone(), side_effecting),
        ToolAction::ListDirectory { dir, .. } => (
            "list_directory",
            dir.clone().unwrap_or_default(),
            side_effecting,
        ),
        ToolAction::SearchContent { pattern, .. } => {
            ("search_content", pattern.clone(), side_effecting)
        }
        ToolAction::EditFile { summary, .. } => ("edit_file", summary.clone(), side_effecting),
        ToolAction::SearchCodebase { query, .. } => {
            ("search_codebase", query.clone(), side_effecting)
        }
        ToolAction::ReadGitStatus => ("read_git_status", String::new(), side_effecting),
        ToolAction::ReadGitDiff { staged } => ("read_git_diff", staged.to_string(), side_effecting),
        ToolAction::WebDiagnostic(call) => ("web_diagnostic", call.url.clone(), side_effecting),
        ToolAction::McpCall {
            server_id,
            tool_name,
            ..
        } => (
            "mcp_call",
            format!("{server_id}/{tool_name}"),
            side_effecting,
        ),
    }
}

/// Whether taking this action would change something outside the session log,
/// and so must wait for the plan to be reviewed (spec 21 §5.5).
///
/// Derived from [`tool_action_marker`]'s side-effect answer so that question
/// keeps living in one place, with two deliberate departures:
///
/// * `propose_patch` writes nothing to the repository and is correctly *not*
///   side-effecting for crash recovery — but it is the front door to an edit,
///   and §5.5 exists to put the plan in front of the user before edits start.
/// * `run_command` is answered by `command_needs_approval`, not by the action
///   alone. Whether a command mutates depends on the command, and the command
///   policy is the only thing that knows: a sandbox-safe `ls` is a read, and
///   gating on it would put a plan up for approval for a turn that only looks
///   at things — the noise §5.5 warns the user learns to click through.
fn action_awaits_plan_review(
    action: &ToolAction,
    command_needs_approval: impl FnOnce(&str) -> bool,
) -> bool {
    match action {
        ToolAction::ProposePatch(_) => true,
        ToolAction::Command(request) => command_needs_approval(&request.command),
        other => tool_action_marker(other).2,
    }
}

/// Rebuilds a plan from the user's revision, in the order the user put the
/// steps in.
///
/// A step the user omits is dropped — **unless it has already reached a
/// terminal status**. A completed or blocked step carries evidence tied to a
/// state of the repository, and deleting it would leave a plan whose history
/// no longer describes what happened: the reason §5.5 rules out mid-execution
/// editing in the first place. Terminal steps keep their original order, ahead
/// of whatever the user chose to keep, so what already ran still reads as
/// having run first.
///
/// A revision naming a step the plan does not have is ignored, for the same
/// reason `read_task_plan` ignores an update for an unknown step id: the step
/// list is not something a revision may grow.
///
/// Everything but the title is carried over from the existing step. The user
/// is reordering and retitling work, not asserting anything about its status
/// or its evidence — those remain the engine's to decide (§5.3).
fn apply_plan_revision(current: &TaskPlan, revision: &[PlanRevisionStep]) -> TaskPlan {
    let mut revised = current.clone();
    let mut steps: Vec<crate::plan::PlanStep> = current
        .steps
        .iter()
        .filter(|step| step.status.is_terminal())
        .cloned()
        .collect();
    for edit in revision {
        let Some(existing) = current.steps.iter().find(|step| step.id == edit.id) else {
            continue;
        };
        if existing.status.is_terminal() {
            continue;
        }
        let mut step = existing.clone();
        step.title = edit.title.clone();
        steps.push(step);
    }
    revised.steps = steps;
    revised
}

/// The text a paused turn shows while its plan is under review.
fn plan_review_response(plan: &TaskPlan, deferred_action: &str) -> String {
    let mut text = String::from("Before going further, here is the plan:\n");
    for (index, step) in plan.steps.iter().enumerate() {
        text.push_str(&format!("{}. {}\n", index + 1, step.title));
    }
    text.push_str(&format!(
        "\nWaiting for your review before the first step that changes anything ({deferred_action}). You can reorder the steps, retitle them, remove any you do not want, or approve the plan as it stands."
    ));
    text
}

/// What to show the user while a tool runs. Lives here rather than in the UI so
/// the frontend never has to map tool names to prose.
fn tool_action_label(action: &ToolAction) -> String {
    match action {
        ToolAction::Command(request) => format!("Proposing `{}`", request.command),
        ToolAction::ProposePatch(_) => "Preparing a patch".to_string(),
        ToolAction::ProposePlan(steps) => format!("Planning {} steps", steps.len()),
        ToolAction::CompleteStep => "Finishing a step".to_string(),
        ToolAction::ReadFile { path, .. } => format!("Reading {path}"),
        ToolAction::ListDirectory { dir, .. } => match dir {
            Some(dir) => format!("Listing {dir}"),
            None => "Listing the repository".to_string(),
        },
        ToolAction::SearchContent { pattern, .. } => {
            format!("Searching for \"{pattern}\"")
        }
        ToolAction::EditFile { summary, .. } => format!("Preparing an edit: {summary}"),
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

fn propose_plan_tool_definition() -> ToolDefinition {
    ToolDefinition {
        name: "propose_plan".to_string(),
        description: "Propose an ordered plan for non-trivial work, before starting it. Use this when the task needs more than one step — a single question or a single file read needs no plan. Do not report a step's outcome here; call complete_step when a step's work is done and Damaian will record how it ended from what it observed.".to_string(),
        parameters_json: "{\"type\":\"object\",\"properties\":{\"steps\":{\"type\":\"array\",\"minItems\":2,\"items\":{\"type\":\"object\",\"properties\":{\"title\":{\"type\":\"string\",\"description\":\"Short imperative title for the step\"},\"detail\":{\"type\":\"string\",\"description\":\"Optional longer description\"}},\"required\":[\"title\"]}}},\"required\":[\"steps\"]}".to_string(),
    }
}

fn complete_step_tool_definition() -> ToolDefinition {
    ToolDefinition {
        name: "complete_step".to_string(),
        description: "Move on from the current plan step. Damaian decides whether the step is completed or blocked from the commands and patches it observed while the step was running, so there is no outcome to supply here.".to_string(),
        parameters_json: "{\"type\":\"object\",\"properties\":{},\"required\":[]}".to_string(),
    }
}

fn read_file_tool_definition() -> ToolDefinition {
    ToolDefinition {
        name: "read_file".to_string(),
        description: "Read a file from the repository to help answer the user's question. The result states the lines returned and the file's total line count, so a truncated read says so rather than reading as the whole file.".to_string(),
        parameters_json: "{\"type\":\"object\",\"properties\":{\"path\":{\"type\":\"string\",\"description\":\"Repository-relative file path\"},\"start_line\":{\"type\":\"integer\",\"description\":\"First line to return, 1-based; default 1\"},\"end_line\":{\"type\":\"integer\",\"description\":\"Last line to return, inclusive; default is the last line or the read cap\"}},\"required\":[\"path\"]}".to_string(),
    }
}

fn list_directory_tool_definition() -> ToolDefinition {
    ToolDefinition {
        name: "list_directory".to_string(),
        description: "List repository-relative file paths, honouring .gitignore. Prefer this over a shell command: it needs no approval.".to_string(),
        parameters_json: "{\"type\":\"object\",\"properties\":{\"dir\":{\"type\":\"string\",\"description\":\"Repository-relative directory; defaults to the repository root\"},\"depth\":{\"type\":\"integer\",\"description\":\"Maximum directory depth to descend\"}},\"required\":[]}".to_string(),
    }
}

fn search_content_tool_definition() -> ToolDefinition {
    ToolDefinition {
        name: "search_content".to_string(),
        description: "Search file contents by regular expression and return path, line number and the matching line. Use this to find call sites; use search_codebase to find which files are about a topic.".to_string(),
        parameters_json: "{\"type\":\"object\",\"properties\":{\"pattern\":{\"type\":\"string\",\"description\":\"Regular expression\"},\"path_glob\":{\"type\":\"string\",\"description\":\"Optional glob limiting which files are searched\"},\"max_matches\":{\"type\":\"integer\",\"description\":\"Fewer matches than the configured cap; it cannot raise it\"}},\"required\":[\"pattern\"]}".to_string(),
    }
}

fn edit_file_tool_definition() -> ToolDefinition {
    ToolDefinition {
        name: "edit_file".to_string(),
        description: "Propose a change to part of a file by replacing an exact snippet. old_text must appear exactly once, or the edit is refused. Nothing is written to disk until the user approves it.".to_string(),
        parameters_json: "{\"type\":\"object\",\"properties\":{\"summary\":{\"type\":\"string\",\"description\":\"Short summary of the change\"},\"edits\":{\"type\":\"array\",\"items\":{\"type\":\"object\",\"properties\":{\"path\":{\"type\":\"string\"},\"old_text\":{\"type\":\"string\",\"description\":\"Exact text to replace; must match once\"},\"new_text\":{\"type\":\"string\"}},\"required\":[\"path\",\"old_text\",\"new_text\"]}}},\"required\":[\"summary\",\"edits\"]}".to_string(),
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
        "propose_plan" => {
            let Ok(arguments) = serde_json::from_str::<serde_json::Value>(&call.arguments_json)
            else {
                return Ok(None);
            };
            let Some(entries) = arguments.get("steps").and_then(|value| value.as_array()) else {
                return Ok(None);
            };
            let steps: Vec<ProposedStep> = entries
                .iter()
                .filter_map(|entry| {
                    let title = entry
                        .get("title")
                        .and_then(|value| value.as_str())
                        .map(str::trim)
                        .filter(|value| !value.is_empty())?;
                    Some(ProposedStep {
                        title: title.to_string(),
                        detail: entry
                            .get("detail")
                            .and_then(|value| value.as_str())
                            .map(str::trim)
                            .filter(|value| !value.is_empty())
                            .map(str::to_string),
                    })
                })
                .collect();
            // A plan of one step is not a plan (§5.1). Decoding it to `None`
            // rather than accepting it keeps the ceremony out of trivial turns
            // at the one place that decides.
            if steps.len() < 2 {
                return Ok(None);
            }
            Ok(Some(ToolAction::ProposePlan(steps)))
        }
        "complete_step" => Ok(Some(ToolAction::CompleteStep)),
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
            let start_line = arguments
                .get("start_line")
                .and_then(|value| value.as_u64())
                .map(|value| value as usize);
            let end_line = arguments
                .get("end_line")
                .and_then(|value| value.as_u64())
                .map(|value| value as usize);
            let range = match (start_line, end_line) {
                (Some(start), Some(end)) => Some(LineRange { start, end }),
                _ => None,
            };
            Ok(Some(ToolAction::ReadFile {
                path: path.to_string(),
                range,
            }))
        }
        "list_directory" => {
            let Ok(arguments) = serde_json::from_str::<serde_json::Value>(&call.arguments_json)
            else {
                return Ok(None);
            };
            let dir = arguments
                .get("dir")
                .and_then(|value| value.as_str())
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_string);
            let depth = arguments
                .get("depth")
                .and_then(|value| value.as_u64())
                .map(|value| value as usize);
            Ok(Some(ToolAction::ListDirectory { dir, depth }))
        }
        "search_content" => {
            let Ok(arguments) = serde_json::from_str::<serde_json::Value>(&call.arguments_json)
            else {
                return Ok(None);
            };
            let Some(pattern) = arguments
                .get("pattern")
                .and_then(|value| value.as_str())
                .map(str::trim)
                .filter(|value| !value.is_empty())
            else {
                return Ok(None);
            };
            let path_glob = arguments
                .get("path_glob")
                .and_then(|value| value.as_str())
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_string);
            let max_matches = arguments
                .get("max_matches")
                .and_then(|value| value.as_u64())
                .map(|value| value as usize);
            Ok(Some(ToolAction::SearchContent {
                pattern: pattern.to_string(),
                path_glob,
                max_matches,
            }))
        }
        "edit_file" => {
            let Ok(arguments) = serde_json::from_str::<serde_json::Value>(&call.arguments_json)
            else {
                return Ok(None);
            };
            let Some(summary) = arguments
                .get("summary")
                .and_then(|value| value.as_str())
                .map(str::trim)
                .filter(|value| !value.is_empty())
            else {
                return Ok(None);
            };
            let Some(entries) = arguments.get("edits").and_then(|value| value.as_array()) else {
                return Ok(None);
            };
            let edits: Vec<RegionEdit> = entries
                .iter()
                .filter_map(|entry| {
                    let path = entry
                        .get("path")
                        .and_then(|value| value.as_str())
                        .map(str::trim)
                        .filter(|value| !value.is_empty())?;
                    let old_text = entry.get("old_text").and_then(|value| value.as_str())?;
                    let new_text = entry.get("new_text").and_then(|value| value.as_str())?;
                    Some(RegionEdit {
                        path: path.to_string(),
                        old_text: old_text.to_string(),
                        new_text: new_text.to_string(),
                    })
                })
                .collect();
            if edits.is_empty() {
                return Ok(None);
            }
            Ok(Some(ToolAction::EditFile {
                summary: summary.to_string(),
                edits,
            }))
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

/// A decoded call and what it asks for; `None` is the text-envelope fallback.
type DecodedCall = (Option<ToolCall>, ToolAction);
/// A call that could not be decoded, with the note to feed back to the model.
type UndecodableCall = (ToolCall, String);

/// Every call in a round that decodes, in the order the model made them, plus
/// the ones that did not and the note to feed back for each.
///
/// A first-only walk used to silently drop the rest of a round, so the
/// multi-call shape spec 47 requirement 8 batches never actually reached
/// dispatch. An `Err` is a rejected call; an `Ok(None)` is a recognized tool
/// whose required arguments were missing or empty, and is reported through
/// [`undecodable_tool_call_note`] so a truncated `arguments` string still says
/// so.
fn decodable_tool_actions(
    calls: &[ToolCall],
    truncated: bool,
) -> (Vec<DecodedCall>, Vec<UndecodableCall>) {
    let mut actions = Vec::new();
    let mut undecodable = Vec::new();
    for call in calls {
        match tool_action_from_call(call) {
            Ok(Some(action)) => actions.push((Some(call.clone()), action)),
            Ok(None) => undecodable.push((call.clone(), undecodable_tool_call_note(call, truncated))),
            Err(error) => undecodable.push((
                call.clone(),
                format!(
                    "Your `{}` call was rejected before execution: {error}. Retry with well-formed arguments matching the tool schema.",
                    call.name
                ),
            )),
        }
    }
    (actions, undecodable)
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

/// Names the ceiling, what was actually spent, and what is left undone.
///
/// §5.4 requires the usage consumed and the remaining steps. Without the
/// figures the user cannot tell a ceiling set too low from a turn that
/// genuinely ran away, which are opposite problems with opposite fixes.
fn token_budget_exhausted_response(
    ceiling: u64,
    spent: u64,
    plan: Option<&crate::plan::TaskPlan>,
) -> String {
    let remaining: Vec<&str> = plan
        .map(|plan| {
            plan.steps
                .iter()
                .filter(|step| {
                    matches!(
                        step.status,
                        crate::plan::StepStatus::Pending | crate::plan::StepStatus::InProgress
                    )
                })
                .map(|step| step.title.as_str())
                .collect()
        })
        .unwrap_or_default();
    let mut message = format!(
        "This task reached its token ceiling of {ceiling} after spending {spent}. I stopped at a step boundary rather than continue, so nothing is half-done."
    );
    if !remaining.is_empty() {
        message.push_str(&format!("\n\nStill to do: {}.", remaining.join("; ")));
    }
    message.push_str(
        "\n\nRaise `agent_max_task_tokens` and ask again to carry on, or narrow the request.",
    );
    message
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
    match execution.termination {
        CommandTermination::Exited => {
            output.push_str(&execution.exit_code.unwrap_or(-1).to_string());
        }
        CommandTermination::TimedOut => output.push_str("none (timed out)"),
        CommandTermination::Cancelled => output.push_str("none (cancelled by user)"),
    }
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

#[cfg(test)]
mod evidence_tests {
    use super::*;
    use crate::plan::Evidence;

    /// Requirement 6, structurally. The rule is that a step's status is a
    /// function of what Damaian observed, and the enum is where that is
    /// enforced: there is no variant a model claim could be poured into.
    ///
    /// Asserted against the source rather than the type system because the
    /// failure mode is someone *adding* a variant under deadline, which no
    /// type-level assertion can catch. This makes that a deliberate, visible
    /// act — the test names the requirement it would be breaking.
    ///
    /// Comments are stripped before the scan, and that is not a convenience:
    /// the first version of this test failed on `plan.rs`'s own doc comment
    /// explaining why the variant does not exist. A guard that cannot tell a
    /// declaration from prose about the absence of one would be removed by the
    /// next person who documents the rule, which is precisely the person it
    /// exists to protect.
    #[test]
    fn evidence_has_no_variant_a_model_could_fill() {
        let code: String = include_str!("plan.rs")
            .lines()
            .filter(|line| !line.trim_start().starts_with("//"))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            !code.contains("ModelAsserted"),
            "requirement 6: evidence is what Damaian observed, never what the model claimed"
        );
    }

    #[test]
    fn a_failing_command_produces_evidence_carrying_its_code() {
        assert_eq!(
            evidence_for(&ActionOutcome::CommandExit(Some(1)), "action_1"),
            Some(Evidence::CommandExit {
                marker_id: "action_1".to_string(),
                exit_code: Some(1),
            })
        );
    }

    #[test]
    fn a_command_with_no_exit_code_still_produces_evidence() {
        // Not `None`. "We ran it and learned nothing" is a different fact from
        // "we did not run it", and the status rule needs the difference to
        // block the step rather than complete it unverified.
        assert_eq!(
            evidence_for(&ActionOutcome::CommandExit(None), "action_1"),
            Some(Evidence::CommandExit {
                marker_id: "action_1".to_string(),
                exit_code: None,
            })
        );
    }

    #[test]
    fn a_read_only_tool_that_succeeded_produces_no_evidence() {
        // A `read_file` that worked says nothing about whether the step it
        // served is done. Manufacturing a success record from it is exactly
        // the letter-not-spirit failure requirement 6 guards against — it
        // would make every step look verified.
        assert_eq!(evidence_for(&ActionOutcome::Ok, "action_1"), None);
    }

    #[test]
    fn a_tool_that_reported_failure_produces_no_false_success() {
        // `Failed` carries no exit code, so there is nothing to record that
        // would not be invented. The step's own status comes from the absence
        // of confirming evidence, not from a fabricated failure record.
        assert_eq!(evidence_for(&ActionOutcome::Failed, "action_1"), None);
    }

    /// Requirement 5: a command that was killed reports why it stopped. `-1` is
    /// what `exit_code.unwrap_or(-1)` prints, and a killed command reads as a
    /// failed one if its termination is not named.
    #[test]
    fn a_timed_out_command_reports_its_termination_not_a_negative_exit_code() {
        let execution = CommandExecution {
            id: "cmd_1".to_string(),
            command: "sleep 30".to_string(),
            working_directory: "/tmp".to_string(),
            risk: crate::command_policy::CommandRisk::Low,
            approved_by: None,
            started_at_ms: 0,
            completed_at_ms: 1,
            exit_code: None,
            termination: crate::command_runner::CommandTermination::TimedOut,
            stdout: String::new(),
            stderr: String::new(),
        };

        let context = sandbox_command_context(&execution);

        assert!(context.contains("timed out"), "{context}");
        assert!(
            !context.contains("-1"),
            "a killed command must not read as exit -1: {context}"
        );
    }

    /// Requirement 6's second half: the within-turn array is bounded, and the
    /// elision is stated. A request that grows with every round is what reaches
    /// the token ceiling on array size rather than on work.
    #[test]
    fn a_long_turn_is_clamped_and_says_what_it_cut() {
        let mut messages = vec![ModelMessage::system("system"), ModelMessage::user("prompt")];
        for index in 0..20 {
            messages.push(ModelMessage::assistant(format!("assistant {index}")));
            messages.push(ModelMessage::tool(
                format!("call_{index}"),
                format!("tool {index}"),
            ));
        }

        let bounded = bounded_messages(&messages, 10);

        assert!(
            bounded.len() <= 10,
            "the request must be bounded: {}",
            bounded.len()
        );
        assert_eq!(bounded[0].content, "system");
        assert_eq!(bounded[1].content, "prompt");
        assert!(
            bounded[2].content.contains("elided"),
            "the cut must be stated: {}",
            bounded[2].content
        );
        assert!(
            bounded[3].content.starts_with("assistant"),
            "a retained tool result must keep the call before it: {}",
            bounded[3].content
        );
    }

    #[test]
    fn a_turn_inside_the_window_is_left_alone() {
        let messages = vec![ModelMessage::system("system"), ModelMessage::user("prompt")];
        let bounded = bounded_messages(&messages, 24);
        assert_eq!(bounded.len(), 2);
        assert!(
            !bounded
                .iter()
                .any(|message| message.content.contains("elided"))
        );
    }

    // -----------------------------------------------------------------
    // Requirement 8's concurrent batch, at the level the turn calls it
    // -----------------------------------------------------------------

    fn batch_repo(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("damaian-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("src")).unwrap();
        fs::write(dir.join("src/a.rs"), "pub fn alpha() {}\n").unwrap();
        fs::write(dir.join("src/b.rs"), "pub fn beta() {}\n").unwrap();
        dir
    }

    fn batch_engine(repo: &Path) -> crate::workspace_engine::WorkspaceEngine {
        crate::workspace_engine::WorkspaceEngine::new(Config {
            data_dir: repo.join(".damaian"),
            enable_index_watcher: false,
            ..Config::default()
        })
    }

    fn batch_session() -> Session {
        Session {
            id: "session-1".to_string(),
            repository_id: "repo-1".to_string(),
            title: "batch".to_string(),
            created_at_ms: 0,
            updated_at_ms: 0,
            summary: String::new(),
            origin: "user".to_string(),
        }
    }

    fn batch_task() -> Task {
        Task {
            id: "task-1".to_string(),
            session_id: "session-1".to_string(),
            status: TaskStatus::PreparingContext,
            user_prompt: "batch".to_string(),
            model_provider: "mock".to_string(),
            model_name: "mock".to_string(),
            created_at_ms: 0,
            completed_at_ms: None,
        }
    }

    fn two_reads() -> Vec<DecodedCall> {
        vec![
            (
                None,
                ToolAction::ReadFile {
                    path: "src/a.rs".to_string(),
                    range: None,
                },
            ),
            (
                None,
                ToolAction::ReadFile {
                    path: "src/b.rs".to_string(),
                    range: None,
                },
            ),
        ]
    }

    /// Requirement 8: the concurrent batch must be byte-identical to the same
    /// calls dispatched one after another, in the model's order. This is the
    /// "comparing a concurrent run against a sequential one" half of the
    /// acceptance criterion, and it is exact rather than timing-based.
    #[test]
    fn a_read_only_batch_matches_a_sequential_dispatch() {
        let repo = batch_repo("batch-equivalence");
        let engine = batch_engine(&repo);
        let orchestrator = &engine.chat_orchestrator;
        let session = batch_session();
        let task = batch_task();
        let actions = two_reads();

        let sequential: Vec<_> = actions
            .iter()
            .map(|(_, action)| {
                orchestrator.dispatch_read_only_action(&repo, &session, &task, action)
            })
            .collect();
        let concurrent =
            orchestrator.run_read_only_batch(&repo, &session, &task, &actions, &CancelToken::new());

        assert_eq!(
            sequential, concurrent,
            "concurrent results must equal sequential ones, in the same order"
        );
    }

    /// Requirement 8: a stop cancels every call in the batch that has not
    /// started. Deterministic here because the token is set before the batch
    /// runs, so every thread checks it first; no `file_read` audit entry may be
    /// written. An already in-flight read is still not interruptible, which the
    /// method's comment states rather than hides.
    #[test]
    fn a_cancelled_batch_dispatches_nothing() {
        let repo = batch_repo("batch-cancelled");
        let engine = batch_engine(&repo);
        let orchestrator = &engine.chat_orchestrator;
        let session = batch_session();
        let task = batch_task();
        let actions = two_reads();

        let cancel = CancelToken::new();
        cancel.cancel();
        let results = orchestrator.run_read_only_batch(&repo, &session, &task, &actions, &cancel);

        assert_eq!(
            results.len(),
            2,
            "one result slot per call, so the turn can abandon them"
        );
        assert!(
            results
                .iter()
                .all(|(_, _, outcome)| *outcome == ActionOutcome::Cancelled),
            "every call must report cancelled: {results:?}"
        );
        let audit =
            fs::read_to_string(repo.join(".damaian/audit/events.jsonl")).unwrap_or_default();
        assert!(
            !audit.contains("file_read"),
            "nothing may be read once the batch is cancelled: {audit}"
        );
    }

    /// The timing half of requirement 8's first clause. It is `#[ignore]`d
    /// because a wall-clock comparison is exactly the kind of assertion that
    /// flakes under load, and the exact sequential-equals-concurrent test above
    /// is the real gate. Run it by hand:
    ///
    /// ```sh
    /// cargo test -p workspace-engine --lib -- --ignored --exact \
    ///   chat::evidence_tests::a_concurrent_batch_is_faster_than_a_sequential_search
    /// ```
    #[test]
    #[ignore]
    fn a_concurrent_batch_is_faster_than_a_sequential_search() {
        let repo = batch_repo("batch-timing");
        for index in 0..2000 {
            fs::write(
                repo.join("src").join(format!("f{index}.rs")),
                "let needle = 1;\n",
            )
            .unwrap();
        }
        let engine = batch_engine(&repo);
        let orchestrator = &engine.chat_orchestrator;
        let session = batch_session();
        let task = batch_task();
        let actions: Vec<DecodedCall> = (0..4)
            .map(|_| {
                (
                    None,
                    ToolAction::SearchContent {
                        pattern: "needle".to_string(),
                        path_glob: None,
                        max_matches: None,
                    },
                )
            })
            .collect();

        let sequential_start = std::time::Instant::now();
        for (_, action) in &actions {
            orchestrator.dispatch_read_only_action(&repo, &session, &task, action);
        }
        let sequential = sequential_start.elapsed();

        let concurrent_start = std::time::Instant::now();
        orchestrator.run_read_only_batch(&repo, &session, &task, &actions, &CancelToken::new());
        let concurrent = concurrent_start.elapsed();

        eprintln!("sequential {sequential:?}, concurrent {concurrent:?}");
        assert!(
            concurrent < sequential,
            "four concurrent searches should beat four sequential ones: \
             concurrent {concurrent:?} vs sequential {sequential:?}"
        );
    }
}
