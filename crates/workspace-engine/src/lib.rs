pub mod audit;
pub mod branch;
pub mod cancel;
pub mod chat;
pub mod checkpoint;
pub mod clarification;
pub mod command_policy;
pub mod command_purpose;
pub mod command_runner;
pub mod commit;
pub mod compaction;
pub mod config;
pub mod context_manager;
pub mod data_schema;
pub mod diff;
pub mod edit;
pub mod effective_policy;
pub mod embeddings;
pub mod error;
pub mod file_access;
pub mod finding;
pub mod git_service;
pub mod hash;
pub mod hooks;
pub mod ignore;
pub mod index_cache;
pub mod indexer;
pub mod language;
pub mod lsp;
pub mod mcp;
pub mod memory;
pub mod mode;
pub mod model;
pub mod navigation;
pub mod patch_engine;
pub mod path_policy;
pub mod plan;
pub mod process_registry;
pub mod profile;
pub mod recovery;
mod render;
pub mod repository_map;
pub mod repository_trust;
pub mod secret_scanner;
pub mod session;
pub mod steering;
pub mod symbol_index;
pub mod tree_walk;
pub mod validation;
pub mod vector_index;
pub mod verification;
pub mod web_diagnostics;
pub mod worker_progress;
pub mod workspace_engine;
pub mod worktree;
pub mod worktree_access;

pub use audit::AuditLog;
pub use branch::{
    BRANCH_SLUG_MAX_CHARS, BranchEngine, BranchError, BranchInspection, BranchOutcome,
    BranchPreparation, BranchRecord, BranchSuggestion, GitMutation, MergePreparation,
    RebasePreparation, slugify_branch_title, suggest_branch_name, validate_branch_prefix,
};
pub use cancel::CancelToken;
pub use chat::{
    AgentCheckBatch, AgentCommandProposal, AgentContextReview, AgentFallbackProposal,
    AgentPatchProposal, AgentPlanProposal, AgentQuestionProposal, AgentRoundBudgetProposal,
    ChatOrchestrator, ChatTurnOptions, ChatTurnResult, ContextDecision, McpTokenResolver,
    PausedTurns, PhaseKind, PlanRevisionStep, ResumeDecisionOptions, RoundBudgetDecision,
    RoundBudgetPause, TurnPhase, TurnProgress, TurnSink,
};
pub use checkpoint::{
    CheckpointConversation, CheckpointExclusion, CheckpointFile, CheckpointManifest,
    CheckpointOrigin, CheckpointPath, CheckpointRequest, CheckpointRestoreOptions,
    CheckpointRestoreResult, CheckpointStore, CommandCensus, PendingApproval,
};
pub use clarification::{
    ANSWER_MAX_CHARS, AskUserInput, MAX_QUESTIONS_PER_TURN_CEILING, OPTION_MAX_CHARS, OPTIONS_MAX,
    OPTIONS_MIN, PendingDecision, PendingQuestionRef, QUESTION_MAX_CHARS, QuestionState,
    validate_answer,
};
pub use command_policy::{
    ApprovalCause, CommandClassification, CommandPolicy, CommandRisk, PROJECT_MANIFESTS,
    allow_always_eligible,
};
pub use command_purpose::{CommandPurpose, command_purpose};
pub use command_runner::{CommandExecution, CommandRunOptions, CommandRunner, CommandTermination};
pub use commit::{
    CommitCandidate, CommitCandidateSource, CommitEngine, CommitError, CommitHookOutcome,
    CommitOutcome, CommitPreparation, CommitRecord, CommitRequest, CommitStatus,
    CommitVerification, GIT_COMMIT_HOOKS, GitHookInfo, GitHookReport,
};
pub use compaction::{
    ApprovalDigest, ConversationSummary, ConversationView, Narrative, PlanStepDigest,
    ValidationDigest, bound_objective, narrative_prompt, parse_narrative, transcript_for_summary,
};
pub use config::{
    AppliedKey, CommandAccess, Config, ConfigKeyKind, ConfigOverlay, ConfigScope, CostEstimate,
    DEFAULT_CONTEXT_TOKEN_BUDGET, HookConfig, HookConfigOverlay, HookEvent, HookMode, HookReview,
    HookSource, LANGUAGE_SERVER_LANGUAGES, MAX_HOOK_TIMEOUT_MS, McpServerConfig,
    McpServerConfigOverlay, McpTransport, ModelProviderConfig, ModelProviderConfigOverlay,
    OverlayOutcome, RejectedConfigKey, RepositoryConfigReport, RepositoryKeyClass,
    normalize_mcp_server_id, normalize_model_provider, normalize_model_reasoning_level,
    overlay_field_kinds, parse_mcp_transport,
};
pub use context_manager::{
    CategoryUsage, CollectionSource, ContextBudget, ContextCategory, ContextItem, ContextKind,
    ContextManager, ContextManifest, ContextPlan, ContextRequest, ConversationSummaryUsage,
    EffectiveLimit, ExclusionRecord, IncludedRef, LimitSource, PreparedContext, PromptSection,
    PromptSectionName, RepositoryMapSource, SymbolProvenanceCounts, SymbolSource, TokenEstimate,
};
pub use data_schema::{
    CURRENT_DATA_SCHEMA_VERSION, DataSchemaError, DataSchemaOutcome, ensure_data_dir_schema,
};
pub use diff::{DiffLine, Hunk, create_unified_diff, diff_file, reconstruct_content};
pub use edit::{
    EditOrchestrator, EditProposalResult, GeneratedEdit, PatchStore, parse_generated_edit,
    patch_diff_text, patch_hunk_summary,
};
pub use effective_policy::{
    EffectivePolicy, PolicyEntry, PolicyRule, PolicySource, RefusedBy, RefusedRequest, SourceKind,
    rule_label,
};
pub use error::{ClientError, ProviderRefusal, Result};
pub use file_access::{FileAccessController, FileRead, LineRange, ReadWindow};
pub use git_service::{
    BranchInfo, BranchNameCheck, DiffOptions, GitFileStatus, GitHubRepo, GitReadCaps, GitService,
    GitStatus, GitSummary, WorktreeListing, WorktreeListingEntry, parse_github_remote,
};
pub use hash::repository_id_for_root;
pub use hooks::{
    HookActivation, HookContext, HookDispatcher, HookEnvelope, HookEnvelopeParts, HookFailureKind,
    HookFinding, HookOutcome, HookResponse, RequestedCommand, activation, hook_definition_hash,
};
pub use index_cache::IndexCache;
pub use indexer::{ProjectIndexer, RepositoryIndex, SearchResult};
pub use language::{Declaration, SymbolKind, extract_declarations};
pub use lsp::{
    DefinitionPass, DefinitionPassReport, FileDiagnostics, LANGUAGE_SERVER_IDLE_SECS,
    LANGUAGE_SERVER_REQUEST_TIMEOUT, LANGUAGE_SERVER_SHUTDOWN_GRACE, LanguageServerError,
    LanguageServerHub, LanguageServerLaunch, LanguageServerLimits, LanguageServerState,
    LanguageServerStatus, LspClient, LspEvent, LspRequest, MAX_LSP_MESSAGE_BYTES, Position,
    RepositoryUris, findings_from_diagnostics, read_message, run_definition_pass,
    symbol_kind_from_lsp, symbol_refs_from_locations, symbols_from_document_symbols, write_message,
};
pub use mcp::{
    McpClient, McpRuntime, McpServerRuntime, McpTool, McpToolResult, namespaced_tool_name,
    parse_namespaced_tool_name,
};
pub use memory::{
    EvidenceRef, EvidenceRefusal, MAX_SUGGESTIONS_PER_TASK, MEMORY_SCHEMA_VERSION, MemoryCategory,
    MemoryEntry, MemoryError, MemoryExpiry, MemoryProposal, MemoryProposalGate, MemoryProvenance,
    MemoryRefusal, MemoryResult, MemorySchemaOutcome, MemoryScope, MemoryState, MemoryStore,
    MemoryStoreConfig, MemoryTarget, ProjectIdentity, ProjectKeySource, ProposalOrigin,
    STATEMENT_MAX_CHARS, Sensitivity, UserConfirmation, Verification, evidence_ref_for,
    explicit_memory_request, is_instruction_shaped, normalize_remote_url, normalize_statement,
};
pub use mode::SessionMode;
pub use model::{
    CurlModelTransport, MockModelAdapter, MockModelTransport, ModelAdapter, ModelMessage,
    ModelRequest, ModelRun, ModelTransport, OpenAICompatibleAdapter, ResponseMeta, TokenUsage,
    ToolCall, ToolDefinition, UsageSource, extract_model_tokens, model_request_json,
};
pub use navigation::{DirectoryListing, NavigationController};
pub use patch_engine::{
    GeneratedSecretWarning, PatchApplyResult, PatchEngine, PatchRollbackResult, ProposedChange,
    ProposedFilePatch, ProposedPatch, parse_hunk_selection,
};
pub use path_policy::PathPolicy;
pub use plan::{
    Evidence, PatchedFile, PlanReport, PlanStep, ReportedStep, StepOutcome, StepStatus, TaskPhase,
    TaskPlan,
};
pub use process_registry::{
    ProcessIdentity, ProcessKind, ProcessRegistry, RegisteredProcess, RegistrationHandle,
    SweepDecision, SweepReport, SweepScope,
};
pub use profile::{
    ProfileCapabilities, ProfileId, ProfileImport, ProfileImportReview, ProfileSelection,
    custom_profile_ids, export_profile, import_profile, profile_import_base, review_profile_import,
    review_profile_rejections, select_profile, split_profile_keys,
};
pub use recovery::{
    PendingVerification, QUESTION_WAITING_HEADLINE, ReattachedApproval, RecoveredTask, abandon,
    classify_all, classify_session, headline, mark_failed, pending_verifications,
    reattach_pending_approvals, resume, resume_allowed, resume_blocked_reason,
};
pub use render::{
    render_markdown_to_ansi, render_markdown_to_html, render_markdown_to_html_with_file_links,
};
pub use repository_map::{
    Degradation, DetectedRoot, ExcludedPath, ExclusionReason, MAX_ROOT_DEPTH, MapLoad, ProjectRoot,
    REPOSITORY_MAP_SCHEMA_VERSION, RebuildReason, RenderedMap, RepositoryMap, RepositoryMapStore,
    RootCommand, RootDetection, RootEvidence, RootOverride, RootOverrideEdit, VENDOR_DIRECTORIES,
    detect_roots, edit_root_overrides,
};
pub use repository_trust::{
    RepositoryAllowlistMigration, RepositoryConfigNotice, RepositoryTrustStore,
};
pub use secret_scanner::{Redaction, SecretFinding, SecretScanner};
pub use session::{
    ActionMarker, ChatMessage, ContextPin, DanglingAction, ExportFormat, PatchFileDecision,
    PendingApprovalRef, SearchOptions, Session, SessionSearchHit, SessionSearchResult,
    SessionStore, Task, TaskStatus, TaskUsage, TurnNotes, WorkerRecord,
};
pub use steering::TurnSteering;
pub use symbol_index::{
    CachedRelationSet, HEURISTIC_HEDGE, MERGE_LINE_TOLERANCE, MatchQuality, Provenance, Relation,
    RelationKind, RelationTarget, RenderedSymbols, SYMBOL_INDEX_SCHEMA_VERSION, Symbol, SymbolFile,
    SymbolIndex, SymbolIndexStore, SymbolLoad, SymbolMatch, SymbolRef, SymbolReferences, merge,
    symbol_matches_json, symbol_references_json,
};
pub use tree_walk::{WalkEvent, WalkFile, WalkSkip};
pub use validation::{
    CommandProposal, CommandRunRecord, CommandStore, ValidationOrchestrator,
    command_approval_prompt, resolve_command_directory,
};
pub use verification::{
    BROWSER_CHECK_PREFIX, CheckId, CheckOutcome, CheckRun, FailedCheck, Selection, SkipReason,
    SkippedCheck, VerificationDue, VerificationEvent, VerificationOutcome, VerificationReport,
    VerificationState, fold_verification, is_documentation, propose_checks, select_roots,
};
pub use web_diagnostics::{
    WEB_SCENARIO_ACTIONS, WebConsoleEntry, WebDiagnosticArtifact, WebDiagnosticCall,
    WebDiagnosticDetails, WebDiagnosticKind, WebDiagnosticRecord, WebDiagnosticReport,
    WebDiagnosticsRunner, WebDiagnosticsRunnerHandle, WebDomSummary, WebFailedRequest,
    WebScenarioStep, WebSourceLocation,
};
pub use workspace_engine::WorkspaceEngine;
