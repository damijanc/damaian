pub mod audit;
pub mod cancel;
pub mod chat;
pub mod checkpoint;
pub mod command_policy;
pub mod command_runner;
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
pub mod ignore;
pub mod index_cache;
pub mod indexer;
pub mod language;
pub mod mcp;
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
pub mod tree_walk;
pub mod validation;
pub mod vector_index;
pub mod web_diagnostics;
pub mod workspace_engine;

pub use audit::AuditLog;
pub use cancel::CancelToken;
pub use chat::{
    AgentCommandProposal, AgentPatchProposal, AgentPlanProposal, ChatOrchestrator, ChatTurnOptions,
    ChatTurnResult, McpTokenResolver, PausedTurns, PhaseKind, PlanRevisionStep,
    ResumeDecisionOptions, TurnPhase, TurnProgress, TurnSink,
};
pub use checkpoint::{
    CheckpointConversation, CheckpointExclusion, CheckpointFile, CheckpointManifest,
    CheckpointOrigin, CheckpointPath, CheckpointRequest, CheckpointRestoreOptions,
    CheckpointRestoreResult, CheckpointStore, CommandCensus, PendingApproval,
};
pub use command_policy::{
    CommandClassification, CommandPolicy, CommandRisk, PROJECT_MANIFESTS, allow_always_eligible,
};
pub use command_runner::{CommandExecution, CommandRunOptions, CommandRunner, CommandTermination};
pub use config::{
    AppliedKey, CommandAccess, Config, ConfigKeyKind, ConfigOverlay, ConfigScope, CostEstimate,
    DEFAULT_CONTEXT_TOKEN_BUDGET, McpServerConfig, McpServerConfigOverlay, McpTransport,
    ModelProviderConfig, ModelProviderConfigOverlay, OverlayOutcome, RejectedConfigKey,
    RepositoryConfigReport, RepositoryKeyClass, normalize_mcp_server_id, normalize_model_provider,
    normalize_model_reasoning_level, overlay_field_kinds, parse_mcp_transport,
};
pub use context_manager::{ContextItem, ContextManager, ContextPlan};
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
pub use git_service::{GitFileStatus, GitService, GitStatus};
pub use hash::repository_id_for_root;
pub use index_cache::IndexCache;
pub use indexer::{ProjectIndexer, RepositoryIndex, SearchResult};
pub use mcp::{
    McpClient, McpRuntime, McpServerRuntime, McpTool, McpToolResult, namespaced_tool_name,
    parse_namespaced_tool_name,
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
    ReattachedApproval, RecoveredTask, abandon, classify_all, classify_session, headline,
    mark_failed, reattach_pending_approvals, resume, resume_allowed, resume_blocked_reason,
};
pub use render::{
    render_markdown_to_ansi, render_markdown_to_html, render_markdown_to_html_with_file_links,
};
pub use repository_map::{
    DetectedRoot, ExcludedPath, ExclusionReason, MAX_ROOT_DEPTH, ProjectRoot,
    REPOSITORY_MAP_SCHEMA_VERSION, RepositoryMap, RootCommand, RootDetection, RootEvidence,
    RootOverride, RootOverrideEdit, VENDOR_DIRECTORIES, detect_roots, edit_root_overrides,
};
pub use repository_trust::{
    RepositoryAllowlistMigration, RepositoryConfigNotice, RepositoryTrustStore,
};
pub use secret_scanner::{Redaction, SecretFinding, SecretScanner};
pub use session::{
    ActionMarker, ChatMessage, DanglingAction, ExportFormat, PendingApprovalRef, SearchOptions,
    Session, SessionSearchHit, SessionSearchResult, SessionStore, Task, TaskStatus, TaskUsage,
};
pub use tree_walk::{WalkEvent, WalkFile, WalkSkip};
pub use validation::{
    CommandProposal, CommandRunRecord, CommandStore, ValidationOrchestrator,
    command_approval_prompt,
};
pub use web_diagnostics::{
    WEB_SCENARIO_ACTIONS, WebConsoleEntry, WebDiagnosticArtifact, WebDiagnosticCall,
    WebDiagnosticDetails, WebDiagnosticKind, WebDiagnosticRecord, WebDiagnosticReport,
    WebDiagnosticsRunner, WebDiagnosticsRunnerHandle, WebDomSummary, WebFailedRequest,
    WebScenarioStep, WebSourceLocation,
};
pub use workspace_engine::WorkspaceEngine;
