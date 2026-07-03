use crate::audit::AuditLog;
use crate::chat::ChatOrchestrator;
use crate::command_policy::CommandPolicy;
use crate::command_runner::CommandRunner;
use crate::config::Config;
use crate::context_manager::ContextManager;
use crate::file_access::FileAccessController;
use crate::git_service::GitService;
use crate::indexer::ProjectIndexer;
use crate::patch_engine::PatchEngine;
use crate::path_policy::PathPolicy;
use crate::secret_scanner::SecretScanner;
use crate::session::SessionStore;

#[derive(Debug, Clone)]
pub struct WorkspaceEngine {
    pub config: Config,
    pub scanner: SecretScanner,
    pub audit_log: AuditLog,
    pub path_policy: PathPolicy,
    pub file_access: FileAccessController,
    pub indexer: ProjectIndexer,
    pub context_manager: ContextManager,
    pub command_policy: CommandPolicy,
    pub command_runner: CommandRunner,
    pub git: GitService,
    pub patch_engine: PatchEngine,
    pub session_store: SessionStore,
    pub chat_orchestrator: ChatOrchestrator,
}

impl WorkspaceEngine {
    pub fn new(config: Config) -> Self {
        let scanner = SecretScanner::default();
        let audit_log = AuditLog::new(&config.data_dir, true, scanner.clone());
        let path_policy = PathPolicy::new(&config);
        let file_access = FileAccessController::new(
            config.clone(),
            audit_log.clone(),
            scanner.clone(),
            path_policy.clone(),
        );
        let indexer = ProjectIndexer::new(config.clone(), scanner.clone(), audit_log.clone());
        let context_manager = ContextManager::new(file_access.clone(), scanner.clone());
        let command_policy = CommandPolicy::new(config.clone());
        let command_runner = CommandRunner::new(
            config.clone(),
            command_policy.clone(),
            audit_log.clone(),
            scanner.clone(),
        );
        let git = GitService::new(audit_log.clone());
        let patch_engine = PatchEngine::new(
            config.clone(),
            audit_log.clone(),
            scanner.clone(),
            path_policy.clone(),
        );
        let session_store = SessionStore::new(&config.data_dir);
        let chat_orchestrator = ChatOrchestrator::new(
            config.clone(),
            scanner.clone(),
            audit_log.clone(),
            indexer.clone(),
            context_manager.clone(),
            session_store.clone(),
        );

        Self {
            config,
            scanner,
            audit_log,
            path_policy,
            file_access,
            indexer,
            context_manager,
            command_policy,
            command_runner,
            git,
            patch_engine,
            session_store,
            chat_orchestrator,
        }
    }
}

impl Default for WorkspaceEngine {
    fn default() -> Self {
        Self::new(Config::default())
    }
}
