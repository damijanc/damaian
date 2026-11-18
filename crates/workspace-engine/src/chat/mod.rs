use crate::audit::AuditLog;
use crate::baseline::{BaselineBatch, BaselineCoordinator, BaselinePreparation};
use crate::cancel::CancelToken;
use crate::checkpoint::{
    CheckpointConversation, CheckpointRequest, CheckpointStore, CommandCensus, PendingApproval,
};
use crate::clarification::{
    AskUserInput, OPTION_MAX_CHARS, OPTIONS_MAX, OPTIONS_MIN, PendingQuestionRef,
    QUESTION_MAX_CHARS, validate_answer,
};
use crate::command_policy::{
    ApprovalCause, CommandClassification, CommandRisk, allow_always_eligible,
    is_validation_command, relative_location,
};
use crate::command_purpose::CommandPurpose;
use crate::command_runner::{CommandExecution, CommandTermination};
use crate::compaction::ConversationSummary;
use crate::completion::{CompletionGateService, CompletionReasonCode, CompletionSettlement};
use crate::config::{Config, ConfigOverlay, CostEstimate, McpTransport};
use crate::context_manager::{
    ContextManager, ContextManifest, ContextPlan, ContextRequest, MemoryRecallRef, PreparedContext,
    PromptSectionName, TokenEstimate,
};
use crate::edit::{GeneratedEdit, PatchStore, RegionEdit, region_edits_to_changes};
use crate::error::{ClientError, ProviderRefusal, Result};
use crate::external_reference::{
    ConvertedResponse, Decision, ExternalReference, ExternalUrl, HostAllowlist, Inspection,
    ReadFileContent, inspect_with_policy,
};
use crate::fetch::CurlFetchTransport;
use crate::file_access::{FileAccessController, LineRange, ReadWindow};
use crate::finding::{
    Finding, FindingDraft, FindingSource, FindingStatus, Severity, default_parsers,
    findings_from_execution, findings_from_web_record, resolve_finding_path,
};
use crate::git_service::{DiffOptions, GitService, GitStatus, WorktreeListing};
use crate::hash::{create_id, file_hash, now_millis, repository_id_for_root};
use crate::hooks::{HookContext, HookDispatcher, HookEvent, HookOutcome, Verdict};
use crate::indexer::{ProjectIndexer, SearchResult};
use crate::language::detect_language;
use crate::mcp::{McpRuntime, McpServerRuntime, parse_namespaced_tool_name};
use crate::memory::{
    EvidenceRef as MemoryEvidence, MemoryCategory, MemoryEntry, MemoryMatchReason, MemoryProposal,
    MemoryProvenance, MemoryScope, MemoryState, MemoryStore, ProjectIdentity, ProposalOrigin,
    RecalledMemory, Verification,
};
use crate::mode::{Permission, SessionMode, permits, refusal_message};
use crate::model::{
    ModelAdapter, ModelMessage, ModelRequest, ModelRun, TokenUsage, ToolCall, ToolDefinition,
    model_request_json,
};
use crate::navigation::NavigationController;
use crate::patch_engine::{PatchEngine, ProposedChange, ProposedFilePatch, ProposedPatch};
use crate::path_policy::PathPolicy;
use crate::plan::{
    Evidence, PLANNED_CHECK_VERSION, PlanStep, PlannedCheck, StepStatus, TaskPlan,
    normalize_planned_check_directory, status_from_evidence,
};
use crate::process_registry::ProcessRegistry;
use crate::protected_paths::{
    ProtectedPathAction, ProtectedPathSource, TaskProtectedPaths, ensure_patch_paths_allowed,
    protected_paths_reminder, record_task_protected_paths_from_prompt,
};
use crate::provenance::{PostPatchResult, RepairTarget, classify};
use crate::repair_scope::RepairScopeReview;
use crate::repository_map::RepositoryMapStore;
use crate::request_preparation::{
    RequestPreparationInput, RequestPreparationOutcome, WithinTurnCompactionAttempt,
    prepare_request, request_threshold_tokens,
};
use crate::secret_scanner::SecretScanner;
use crate::session::{
    CallModel, ChatMessage, HandedPlan, PendingApprovalRef, Session, SessionStore, Task,
    TaskStatus, TaskUsage, validate_plan_approval,
};
use crate::stagnation::{RepairStall, detect_repair_stall, repair_state_entry};
use crate::steering::TurnSteering;
use crate::task_budget::{
    TaskBudgetDimension, TaskBudgetPolicy, TaskBudgetReview, TaskExecutionBudget,
};
use crate::task_control::{
    TaskControl, TaskControlService, TurnPresentation, continuation_prompt,
    turn_presentation_with_outcome,
};
use crate::task_outcomes::{
    TaskCompactionFacts, TaskOutcomeFigures, TaskSegmentSnapshot, TaskTerminalOutcome,
    TaskTerminalOutcomeKind, approved_action_round_cap, approved_exploration_round_cap,
};
use crate::task_segments::{
    SegmentContinuationDecision, SegmentContinuationInput, SegmentProgressInput,
    SegmentTokenTotals, SegmentTotals, TASK_SEGMENT_FORMAT_VERSION, TaskRequestTokenAuthority,
    TaskSegmentFinished, TaskSegmentPauseState, TaskSegmentStarted, authorize_task_request,
    decide_segment_continuation, progress_digest, task_request_output_cap,
};
use crate::validation::{
    CommandProposal, CommandStore, ValidationOrchestrator, command_approval_prompt,
    resolve_command_directory,
};
use crate::vector_index::VectorIndexCache;
use crate::verification::{
    BuildErrors, BuildRecord, CheckId, CheckOutcome, CheckReuse, CheckTermination,
    FailureProvenance, FindingEvidence, Selection, SkipReason, VerificationEvent,
    VerificationOutcome, VerificationReport, build_checks_for_change, detected_checks_of_roots,
    environment_termination, fold_verification, propose_checks_exactly, propose_completion_checks,
    propose_scoped_checks, scoped_selection, with_plan_checks,
};
use crate::web_diagnostics::{
    WEB_SCENARIO_ACTIONS, WebDiagnosticCall, WebDiagnosticKind, WebDiagnosticRecord,
    WebDiagnosticReport, WebDiagnosticsRunnerHandle,
};
use crate::within_turn_compaction::{
    CompactedToolKind, CompletedToolCall, CompletedToolRound, ToolIdentity, ToolResultOutcome,
};
use crate::worktree_access::{foreign_worktrees, resolve_worktree};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::fs;
use std::ops::ControlFlow;
use std::path::{Path, PathBuf};
use std::sync::Arc;

mod actions;
mod agentic_turn;
mod ask;
mod command_resume;
mod context_review;
mod dispatch;
use dispatch::*;
mod conversation_compaction;
mod fallback_budget;
use fallback_budget::*;
mod fetch;
mod formatting;
use formatting::*;
mod memory_recall;
mod pending;
pub use pending::*;
mod plan_flow;
use plan_flow::*;
mod prompt;
use prompt::*;
mod protected;
pub(crate) use protected::*;
mod question;
mod repair;
mod segments;
mod task_flow;
mod tool_decoding;
use tool_decoding::*;
mod tool_definitions;
use tool_definitions::*;
mod tool_offer;
use tool_offer::*;
mod tools;
pub(crate) use tools::*;
mod turn_record;
mod turn_request;
mod types;
pub use types::*;
mod verify;
#[cfg(test)]
use verify::plan_defers_check;
mod web_tools;
use web_tools::*;

#[derive(Debug, Clone)]
pub struct ChatOrchestrator {
    config: Config,
    scanner: SecretScanner,
    audit_log: AuditLog,
    indexer: ProjectIndexer,
    context_manager: ContextManager,
    session_store: SessionStore,
    memory_store: MemoryStore,
    validation_orchestrator: ValidationOrchestrator,
    command_store: CommandStore,
    pending_commands: PendingCommandStore,
    /// Failure-time baselines (spec 109 D1): a verification check that failed
    /// with no baseline in its task chain takes one on the task-start tree
    /// before it is classified.
    baselines: BaselineCoordinator,
    file_access: FileAccessController,
    navigation: NavigationController,
    path_policy: PathPolicy,
    git: GitService,
    patch_engine: PatchEngine,
    patch_store: PatchStore,
    checkpoint_store: CheckpointStore,
    hooks: HookDispatcher,
    mcp_token_resolver: Option<McpTokenResolver>,
    web_diagnostics_runner: Option<WebDiagnosticsRunnerHandle>,
    /// See [`Self::allow_loopback_fetch_for_tests`]. False in the product.
    allow_loopback_fetch: bool,
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
        memory_store: MemoryStore,
        validation_orchestrator: ValidationOrchestrator,
        command_store: CommandStore,
        baselines: BaselineCoordinator,
        file_access: FileAccessController,
        navigation: NavigationController,
        path_policy: PathPolicy,
        git: GitService,
        patch_engine: PatchEngine,
        patch_store: PatchStore,
        checkpoint_store: CheckpointStore,
        hooks: HookDispatcher,
    ) -> Self {
        let pending_commands = PendingCommandStore::new(&config.data_dir);
        Self {
            config,
            scanner,
            audit_log,
            indexer,
            context_manager,
            session_store,
            memory_store,
            validation_orchestrator,
            command_store,
            pending_commands,
            baselines,
            file_access,
            navigation,
            path_policy,
            git,
            patch_engine,
            patch_store,
            checkpoint_store,
            hooks,
            mcp_token_resolver: None,
            web_diagnostics_runner: None,
            allow_loopback_fetch: false,
        }
    }

    /// Lets the eval harness and integration tests fetch the `127.0.0.1` stub
    /// page server. Deliberately not a `Config` field: no repository, profile
    /// or user setting can reach it, and the product never calls this. The
    /// transport still resolves the host and checks every address; this only
    /// decides whether a non-public answer is refused.
    #[doc(hidden)]
    pub fn allow_loopback_fetch_for_tests(&mut self) {
        self.allow_loopback_fetch = true;
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

    fn tool_round_limit(&self, web_debug_mode: bool) -> u32 {
        if web_debug_mode {
            self.web_debug_tool_round_limit()
        } else {
            self.default_tool_round_limit()
        }
    }

    /// The per-turn exploration allowance, clamped to
    /// [`ABSOLUTE_EXPLORATION_ROUND_CAP`]. Unlike `agent_max_tool_rounds`, zero
    /// is a valid value ("no exploration allowance", spec 59 requirement 6), so
    /// this clamps from zero rather than from one.
    fn default_exploration_round_limit(&self) -> u32 {
        self.config
            .agent_max_exploration_rounds
            .min(ABSOLUTE_EXPLORATION_ROUND_CAP)
    }

    /// Whether `proposal_id` was raised by a chat turn (and so should be
    /// resumed via [`Self::resume_after_command_decision`]) as opposed to a
    /// standalone command proposal from outside the chat flow.
    ///
    /// A question shares the pending-turn file but not this path: a question
    /// id reaching `/api/run-command` must not be treated as an approval. It
    /// falls through to the standalone proposal path, where no command
    /// proposal has that id and nothing runs (spec 50 `context.md` §2).
    pub fn has_pending_chat_command(&self, proposal_id: &str) -> bool {
        self.pending_commands.has(proposal_id) && !self.pending_commands.is_question(proposal_id)
    }

    /// Refuses a pause of the wrong kind and puts its file back, so the resume
    /// that owns it can still find it.
    ///
    /// `expected` is named by each resume, so every entry point has to say
    /// what it accepts; a new [`PausedFor`] variant fails to compile in
    /// `paused_for` and in the descriptions below until it is classified
    /// (spec 50 `context.md` §2).
    fn refuse_unless_paused_for(
        &self,
        proposal_id: &str,
        pending: &PendingChatTurn,
        expected: PausedFor,
    ) -> Result<()> {
        let actual = paused_for(pending);
        if actual == expected {
            return Ok(());
        }
        self.pending_commands.save(pending)?;
        if actual == PausedFor::Conflicting {
            return Err(ClientError::InvalidInput(format!(
                "Proposal {proposal_id} records more than one kind of pause and cannot be resumed"
            )));
        }
        Err(ClientError::InvalidInput(format!(
            "Proposal {proposal_id} is {}; resume it with {}",
            actual.description(),
            actual.resume_method()
        )))
    }
}

/// How often a model call's growing reasoning count is reported (spec 81 §4).
/// Often enough that the count visibly moves; rare enough that a worker's
/// progress file gains about two lines a second, not one per chunk.
const THINKING_PROGRESS_INTERVAL: std::time::Duration = std::time::Duration::from_millis(500);

/// Absolute upper bound for config-driven tool budgets. Raising past this
/// requires explicit UI work so an agent cannot silently spend a whole session
/// on repeated tool calls.
const ABSOLUTE_TOOL_ROUND_CAP: u32 = 16;

/// Absolute upper bound for the per-turn exploration allowance (spec 59 §5.4).
/// A turn may read all it needs, but not without a bound: with no token ceiling
/// set this allowance is the only thing beyond the action budget that a model
/// can spend, so raising it past the constant requires explicit work, not
/// configuration (requirement 5).
const ABSOLUTE_EXPLORATION_ROUND_CAP: u32 = 32;

/// Absolute bound on the rounds of one turn across all round-budget
/// continuations (spec 61 §5.3): four segments at the default of sixteen
/// rounds. Distinct from [`ABSOLUTE_TOOL_ROUND_CAP`], which
/// bounds a single configured segment — and #47's token-ceiling continuation,
/// which the manual *Continue* is not tied to.
const ABSOLUTE_TURN_ROUND_CAP: u32 = 64;

/// The engine's instruction when a worker has spent its pre-plan exploration
/// budget (spec 103 R17). Exhaustion asks for a plan; it never offers the
/// generic **Continue** against a bound that is already spent.
const PREPLAN_PLAN_REQUIRED_NOTE: &str = "Your pre-plan exploration budget is spent. Call propose_plan now with the steps you intend to take. Do not continue exploring. If you cannot propose a plan, stop and say what is blocking you; do not ask to continue exploring.";

/// The durable kind of [`PREPLAN_PLAN_REQUIRED_NOTE`], so a crash between the
/// instruction and the next model call cannot make the engine hand out another
/// exploration pass.
const PREPLAN_PLAN_REQUIRED_MESSAGE_KIND: &str = "preplan_plan_required";

/// Damaian's own handoff instruction, verbatim from spec 57 §5.0. It is what
/// makes *Start worker* appear without any repository's `AGENTS.md` teaching
/// the `worker-prompt` tag.
pub const WORKER_HANDOFF_PARAGRAPH: &str = "When work should continue in a separate session — because it is a self-contained task, or one of several that can run in parallel — write each task as a prompt in its own fenced block whose opening line is exactly ```worker-prompt, with the tag on the fence line itself and not inside the block. The user can start a worker from that block. A worker starts with a fresh context in its own git worktree and sees only the repository and that prompt, so say what to read, what to change, which checks to run, and when to stop. Do not use the `worker-prompt` tag for anything else.";

/// Said before the handoff paragraph: a session's edits and commands always
/// land in its own checkout, so a request to work in a new branch or worktree
/// is a worker's task. A session that made a worktree itself and then edited
/// the main checkout is how this was found (spec 76).
pub const WORKTREE_TASK_PARAGRAPH: &str = "Your edits and commands always apply to this session's checkout. The `worktree` argument of the read tools only reads. When the user asks for work on a new branch or in a separate git worktree, do not create a worktree yourself: recommend a worker in a ```worker-prompt block, which runs it in a worktree of its own. If the user explicitly chooses to work in this session instead, for example with *Work here instead*, do the work in the current checkout; that choice does not create or select another worktree.";

/// Marks the protected-path reminder inside the system message so a resumed
/// turn can replace it instead of stacking copies.
const PROTECTED_PATHS_REMINDER_MARKER: &str = "\n\n[protected task paths]";

/// Turn options for a turn the engine starts itself rather than one a host
/// started. A worker keeps its question route (spec 104); every other
/// engine-started turn keeps spec 50's headless default.
fn engine_started_turn_options(
    store: &crate::session::SessionStore,
    session_id: &str,
) -> ChatTurnOptions {
    match store.worker_record(session_id) {
        Ok(Some(_)) => ChatTurnOptions::worker(),
        _ => ChatTurnOptions::default(),
    }
}

/// The task prompt of a verification continuation. Damaian's own words, not a
/// user request: the model is told what the turn is for when repair begins
/// (spec 23 `context.md` §13).
const VERIFICATION_TURN_PROMPT: &str = "Verify the patch that was just applied.";

/// A task prompt Damaian wrote for a turn it started itself (a verification or
/// a repair), not one the user typed. Repair prompts start with the
/// verification prompt.
pub(crate) fn is_damaian_task_prompt(prompt: &str) -> bool {
    prompt.starts_with(VERIFICATION_TURN_PROMPT)
}

/// The opening of a continuation after a verification that left the task's
/// plan open (spec 77). Damaian's words, like [`VERIFICATION_TURN_PROMPT`].
/// The continuation prompt for a task with no plan (spec 107 D3). The patch's
/// own checks ran; the full completion checks run once, when the model
/// answers, so the model is told not to stop merely to have them run.
const CONTINUE_PLANLESS_AFTER_VERIFICATION_PROMPT: &str = "The patch was applied and the checks for the code it changed finished. The task is not finished until you say so: if the request needs more changes, propose the next one with `edit_file` or `propose_patch`. When the request is done, answer with what changed and what is left; the full completion checks run once then.";
const CONTINUE_AFTER_VERIFICATION_PROMPT: &str = "The patch was applied and its verification finished. The task is not finished: continue it from its plan. Work through the open steps in order, propose each change with `edit_file` or `propose_patch`, and call `complete_step` as each step is done. Stop when the plan is done, then answer with what changed and what is left.";

/// What a verification turn says when it parks on the check card.
const CHECK_BATCH_PAUSED_RESPONSE: &str =
    "These checks need your approval before verification can continue.";

/// What a verification turn says when it parks on a build check's card
/// (spec 109 D10), and the card's engine title and note.
const BUILD_CHECK_PAUSED_RESPONSE: &str =
    "The build check needs your approval before verification can continue.";
pub const BUILD_CHECK_CARD_TITLE: &str = "Build check";
const BUILD_CHECK_CARD_NOTE: &str = "Damaian compiles each change before anything else checks it, so a change that does not build goes straight back to the model with the compiler's errors. It runs no tests and does not count as a required check.";

/// What a verification says when a failed check's baseline needs approval.
const FAILURE_BASELINE_PAUSED_RESPONSE: &str = "A check failed. Run it on the files as they \
     were before this task's changes to tell whether the failure is new?";

fn root_label(working_directory: &str) -> &str {
    if working_directory.is_empty() {
        "repository root"
    } else {
        working_directory
    }
}

/// The hash of every changed file that still exists. A deleted file is absent,
/// which the report reads as a staleness mismatch rather than a pass.
/// The turn stopped for the user, or was stopped by them, rather than finishing.
fn turn_is_paused(result: &ChatTurnResult) -> bool {
    result.patch_proposal.is_some()
        || result.command_proposal.is_some()
        || result.plan_proposal.is_some()
        || result.question_proposal.is_some()
        || result.fetch_proposal.is_some()
        || result.fallback_proposal.is_some()
        || result.round_budget_proposal.is_some()
        || result.check_batch.is_some()
        || result.context_review.is_some()
        || result.cancelled
}

fn hash_existing_files(root: &Path, paths: &[String]) -> BTreeMap<String, String> {
    let mut hashes = BTreeMap::new();
    for path in paths {
        if let Ok(hash) = file_hash(root.join(path)) {
            hashes.insert(path.clone(), hash);
        }
    }
    hashes
}

/// The tool result for an `ask_user` call nobody can answer.
const NO_USER_CAN_ANSWER: &str = "No user can answer a question in this context, so the question was not asked. Where an ambiguity is small and the default is low-impact, proceed under an explicitly stated assumption, and say in your answer what you assumed and why. Where it would materially change the scope, public behaviour, destructive actions or acceptance criteria, do not guess: stop and report the task incomplete, explaining what you need decided.";

/// What `read_file` returned this task, for the egress guard (spec 51 D9).
///
/// The turn's own tool messages are the exact text the model saw. Re-reading
/// the file would compare against what is on disk *now*: a file rewritten after
/// the read could hide the very line the model is about to put in a URL, and
/// the guard would let it leave.
fn read_files_from_messages(messages: &[ModelMessage]) -> Vec<(String, String)> {
    let mut calls: HashMap<&str, String> = HashMap::new();
    let mut contents = Vec::new();
    for message in messages {
        for call in &message.tool_calls {
            if call.name != "read_file" {
                continue;
            }
            if let Ok(value) = serde_json::from_str::<serde_json::Value>(&call.arguments_json)
                && let Some(path) = value.get("path").and_then(|path| path.as_str())
            {
                calls.insert(call.id.as_str(), path.to_string());
            }
        }
        if message.role == "tool"
            && let Some(id) = &message.tool_call_id
            && let Some(path) = calls.get(id.as_str())
        {
            contents.push((path.clone(), message.content.clone()));
        }
    }
    contents
}

/// The tool result for a `fetch_url` call in a turn that does not offer it
/// (spec 51): nobody can approve a new host, or the per-turn cap is zero.
const NO_FETCH_IN_THIS_TURN: &str = "No page was fetched: this turn does not offer fetch_url, either because no user is available to approve a host or because web_fetch_max_per_turn is 0. Answer from the repository and the conversation instead.";

/// The assistant text a question pause leaves in the transcript. The question
/// itself is its own `question_asked` event (`context.md` §6), so repeating it
/// here would show the same text twice.
const QUESTION_PAUSED_RESPONSE: &str = "I need an answer from you before continuing.";

/// What a turn with no answer route is told instead of being offered
/// `ask_user` (spec 50 `context.md` §4; amended by spec 104). Low-impact
/// ambiguity may still be settled by a stated assumption, but a material one
/// must stop rather than guess, because nobody can correct it later.
const HEADLESS_PARAGRAPH: &str = "No user is available to answer questions during this task. Where an ambiguity is small and the default is low-impact, do not stop: proceed under an explicitly stated assumption, and say in your answer what you assumed and why. Where the answer would materially change the scope, public behaviour, destructive actions or acceptance criteria, do not guess: stop and report the task incomplete, explaining what you need decided.";

/// A turn whose question route exists but whose user is not synchronously
/// present (spec 104), which is every app-managed worker turn. The tool is the
/// route; this paragraph only sets the bar for using it. The ordinary
/// interactive prompt stays byte-identical to what it was before spec 50.
const WORKER_QUESTION_PARAGRAPH: &str = "A user can answer a question, but not while you are working: asking pauses this task until they respond. Prefer proceeding under an explicitly stated assumption for low-impact ambiguity. Ask with `ask_user` when the answer would materially change the scope, public behaviour, destructive actions or acceptance criteria, and say why you cannot decide it from the repository.";

/// How many calls the elision notice and the pre-cut warning list (spec 65
/// §5.3, §5.4), and how long one rendered line may be. The caps keep both
/// blocks a fixed size whatever a turn did.
const ELISION_NOTICE_LINE_CAP: usize = 40;
const PRE_CUT_LINE_CAP: usize = 10;
const CALL_LINE_MAX_CHARS: usize = 120;
/// The system prompt and the user seed, which `bounded_messages` never drops.
const HEAD_MESSAGES: usize = 2;

const NOTES_HEADER: &str = "Notes you recorded earlier in this turn:";
const CARRIED_NOTES_HEADER: &str = "Notes carried from earlier turns in this session, and added to in this one. Files may have changed since they were written:";

#[cfg(test)]
#[path = "tests/environment_classification_tests.rs"]
mod environment_classification_tests;

#[cfg(test)]
#[path = "tests/pending_turn_defaults_tests.rs"]
mod pending_turn_defaults_tests;

#[cfg(test)]
#[path = "tests/repair_scope_ordering_tests.rs"]
mod repair_scope_ordering_tests;

#[cfg(test)]
#[path = "tests/evidence_tests.rs"]
mod evidence_tests;

#[cfg(test)]
#[path = "tests/plan_deferral_tests.rs"]
mod plan_deferral_tests;

/// Layer 1 of spec 20's working modes (`docs/specs/20_working_modes`): the
/// tool list `run_agentic_turn` builds for the model is filtered through
/// `permits` (mode, then profile), so a mode's capability bound is structural
/// rather than a prompt request. These drive a real turn and inspect the `tools` actually
/// sent to the adapter — the same seam spec 49 Task 8 used for its
/// prefix-stability assertions.
#[cfg(test)]
#[path = "tests/mode_tool_list_tests.rs"]
mod mode_tool_list_tests;

/// Layer 2 of spec 20's working modes (`docs/specs/20_working_modes`): the
/// non-native text-envelope fallback's system prompt. `system_prompt()` now
/// takes the turn's mode, so Ask omits the `DAMAIAN_COMMAND_V1` envelope
/// entirely, while Plan and Review keep it but drop the Code-only invitation
/// to request a side-effecting command. `DAMAIAN_EDIT_V1` is not taught here
/// in any mode (`context.md` §7), so it is not part of this layer.
#[cfg(test)]
#[path = "tests/system_prompt_tests.rs"]
mod system_prompt_tests;

/// Spec 63 requirement 7: the model is told which shell runs its commands, and
/// that zsh fails an unquoted glob that matches nothing. Only the shell's
/// basename belongs in the text, never its full path (proposal §5.4).
#[cfg(test)]
#[path = "tests/shell_naming_tests.rs"]
mod shell_naming_tests;

/// Spec 57 §5.0: the handoff instruction. It reaches the model only when the
/// user turned workers on, and never in a worker's own session.
#[cfg(test)]
#[path = "tests/worker_handoff_tests.rs"]
mod worker_handoff_tests;

/// Spec 50 (`docs/specs/50_model_initiated_clarification`): the parts of
/// offering `ask_user` that need `chat.rs`-private items. Engine-level tests
/// live in `tests/clarification.rs`.
#[cfg(test)]
#[path = "tests/clarification_tests.rs"]
mod clarification_tests;

#[cfg(test)]
#[path = "tests/stopped_action_note_tests.rs"]
mod stopped_action_note_tests;

/// Layer 3 of spec 20's working modes (`docs/specs/20_working_modes`): the
/// orchestrator refuses at every action path, independent of what Layers 1
/// and 2 offered the model. One test per refusal point in `context.md` §8's
/// table, plus the cross-cutting guarantees. Every model here calls a tool it
/// may never have been offered — that is the point: withholding is not
/// enforcement.
#[cfg(test)]
#[path = "tests/mode_refusal_tests.rs"]
mod mode_refusal_tests;

/// Spec 62 Task 3: the approval prompt leads with the policy's own reason, and
/// nothing shown to the user or sent to the model about local commands still
/// claims a sandbox exists. Spec 63's shell guidance is checked separately in
/// `shell_naming_tests`; these tests cover the wording this spec changes.
#[cfg(test)]
#[path = "tests/approval_prompt_wording_tests.rs"]
mod approval_prompt_wording_tests;

#[cfg(test)]
#[path = "tests/before_tool_verdict_tests.rs"]
mod before_tool_verdict_tests;

/// Spec 29's two creation paths, at the point the orchestrator owns them. The
/// eval scenarios exercise the same paths end to end; these tests pin the
/// narrower claim that neither writes an entry.
#[cfg(test)]
#[path = "tests/memory_proposal_tests.rs"]
mod memory_proposal_tests;

#[cfg(test)]
#[path = "tests/paused_turn_session_tests.rs"]
mod paused_turn_session_tests;
