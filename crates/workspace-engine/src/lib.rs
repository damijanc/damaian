pub mod audit;
pub mod chat;
pub mod command_policy;
pub mod command_runner;
pub mod config;
pub mod context_manager;
pub mod diff;
pub mod edit;
pub mod embeddings;
pub mod error;
pub mod file_access;
pub mod git_service;
pub mod hash;
pub mod ignore;
pub mod index_cache;
pub mod indexer;
pub mod language;
pub mod model;
pub mod patch_engine;
pub mod path_policy;
pub mod render;
pub mod secret_scanner;
pub mod session;
pub mod validation;
pub mod vector_index;
pub mod workspace_engine;

pub use audit::AuditLog;
pub use chat::{AgentCommandProposal, AgentPatchProposal, ChatOrchestrator, ChatTurnResult};
pub use command_policy::{CommandClassification, CommandPolicy, CommandRisk};
pub use command_runner::{CommandExecution, CommandRunner};
pub use config::{
    Config, ConfigOverlay, ModelProviderConfig, ModelProviderConfigOverlay,
    normalize_model_provider, normalize_model_reasoning_level,
};
pub use context_manager::{ContextItem, ContextManager, ContextPlan};
pub use diff::{DiffLine, Hunk, create_unified_diff, diff_file, reconstruct_content};
pub use edit::{
    EditOrchestrator, EditProposalResult, GeneratedEdit, PatchStore, parse_generated_edit,
    patch_diff_text,
};
pub use error::{ClientError, Result};
pub use file_access::{FileAccessController, FileRead};
pub use git_service::{GitFileStatus, GitService, GitStatus};
pub use index_cache::IndexCache;
pub use indexer::{ProjectIndexer, RepositoryIndex, SearchResult};
pub use model::{
    CurlModelTransport, MockModelAdapter, MockModelTransport, ModelAdapter, ModelMessage,
    ModelRequest, ModelRun, ModelTransport, OpenAICompatibleAdapter, ToolCall, ToolDefinition,
    extract_model_tokens, model_request_json,
};
pub use patch_engine::{
    PatchApplyResult, PatchEngine, PatchRollbackResult, ProposedChange, ProposedFilePatch,
    ProposedPatch,
};
pub use path_policy::PathPolicy;
pub use render::{render_markdown_to_ansi, render_markdown_to_html};
pub use secret_scanner::{Redaction, SecretFinding, SecretScanner};
pub use session::{ChatMessage, Session, SessionStore, Task, TaskStatus};
pub use validation::{
    CommandProposal, CommandRunRecord, CommandStore, ValidationOrchestrator,
    command_approval_prompt,
};
pub use workspace_engine::WorkspaceEngine;
