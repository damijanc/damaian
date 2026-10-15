use crate::audit::AuditLog;
use crate::chat::ChatOrchestrator;
use crate::checkpoint::CheckpointStore;
use crate::command_policy::CommandPolicy;
use crate::command_runner::CommandRunner;
use crate::config::Config;
use crate::context_manager::{ContextManager, RepositoryMapSource};
use crate::edit::{EditOrchestrator, PatchStore};
use crate::error::Result;
use crate::file_access::FileAccessController;
use crate::git_service::GitService;
use crate::index_cache::IndexCache;
use crate::indexer::ProjectIndexer;
use crate::lsp::{
    DefinitionPass, LanguageServerHub, LanguageServerLaunch, PassClaim, run_definition_pass,
};
use crate::navigation::NavigationController;
use crate::patch_engine::PatchEngine;
use crate::path_policy::PathPolicy;
use crate::repository_map::{MapLoad, RepositoryMap, RepositoryMapStore};
use crate::repository_trust::RepositoryTrustStore;
use crate::secret_scanner::SecretScanner;
use crate::session::SessionStore;
use crate::symbol_index::{SymbolIndex, SymbolIndexStore, SymbolLoad, SymbolMatch};
use crate::validation::{CommandStore, ValidationOrchestrator};
use std::collections::BTreeMap;
use std::ops::Deref;
use std::path::Path;

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
    pub checkpoint_store: CheckpointStore,
    pub session_store: SessionStore,
    pub chat_orchestrator: ChatOrchestrator,
    pub patch_store: PatchStore,
    pub edit_orchestrator: EditOrchestrator,
    pub command_store: CommandStore,
    pub validation_orchestrator: ValidationOrchestrator,
    pub repository_trust: RepositoryTrustStore,
}

impl WorkspaceEngine {
    pub fn new(config: Config) -> Self {
        let scanner = SecretScanner::new(config.secret_patterns.clone());
        let audit_log = AuditLog::with_retention(
            &config.data_dir,
            config.audit_enabled,
            config.audit_retention_days,
            scanner.clone(),
        );
        let path_policy = PathPolicy::new(&config);
        let file_access = FileAccessController::new(
            config.clone(),
            audit_log.clone(),
            scanner.clone(),
            path_policy.clone(),
        );
        let indexer = ProjectIndexer::new(config.clone(), scanner.clone(), audit_log.clone());
        let navigation = NavigationController::new(
            config.clone(),
            audit_log.clone(),
            scanner.clone(),
            path_policy.clone(),
        );
        let command_policy = CommandPolicy::new(config.clone());
        let context_manager = ContextManager::new(
            file_access.clone(),
            scanner.clone(),
            config.data_dir.clone(),
            config.enable_semantic_search,
            RepositoryMapSource {
                store: RepositoryMapStore::new(&config.data_dir, audit_log.clone()),
                command_policy: command_policy.clone(),
                max_tokens: config.repository_map_max_tokens,
            },
        );
        let command_runner = CommandRunner::new(
            config.clone(),
            command_policy.clone(),
            audit_log.clone(),
            scanner.clone(),
        );
        let git = GitService::new(audit_log.clone(), scanner.clone());
        let patch_engine = PatchEngine::new(
            config.clone(),
            audit_log.clone(),
            scanner.clone(),
            path_policy.clone(),
        );
        let checkpoint_store =
            CheckpointStore::new(config.clone(), audit_log.clone(), path_policy.clone());
        let repository_trust = RepositoryTrustStore::new(&config.data_dir);
        let session_store = SessionStore::new(&config.data_dir);
        let patch_store = PatchStore::new(&config.data_dir);
        let command_store = CommandStore::new(&config.data_dir);
        let validation_orchestrator = ValidationOrchestrator::new(
            command_policy.clone(),
            command_runner.clone(),
            command_store.clone(),
            audit_log.clone(),
        );
        let chat_orchestrator = ChatOrchestrator::new(
            config.clone(),
            scanner.clone(),
            audit_log.clone(),
            indexer.clone(),
            context_manager.clone(),
            session_store.clone(),
            validation_orchestrator.clone(),
            command_store.clone(),
            file_access.clone(),
            navigation,
            path_policy.clone(),
            git.clone(),
            patch_engine.clone(),
            patch_store.clone(),
            checkpoint_store.clone(),
        );
        let edit_orchestrator = EditOrchestrator::new(
            config.clone(),
            scanner.clone(),
            audit_log.clone(),
            indexer.clone(),
            context_manager.clone(),
            session_store.clone(),
            patch_engine.clone(),
            patch_store.clone(),
            checkpoint_store.clone(),
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
            checkpoint_store,
            session_store,
            chat_orchestrator,
            patch_store,
            edit_orchestrator,
            command_store,
            validation_orchestrator,
            repository_trust,
        }
    }

    /// The repository's map, from the cached index: the stored map when it
    /// is still current, and otherwise a fresh one (spec 24 §5.8).
    pub fn repository_map(
        &self,
        repository_root: impl AsRef<Path>,
    ) -> Result<(RepositoryMap, MapLoad)> {
        let index = IndexCache::get_or_build(&self.indexer, repository_root)?;
        RepositoryMapStore::new(&self.config.data_dir, self.audit_log.clone())
            .load_or_build(&index, &self.command_policy)
    }

    /// The repository's symbol index, from the cached index and the stored
    /// map: the stored index where files are unchanged, and the index pass's
    /// heuristics for the rest (spec 25 §5.10).
    pub fn symbol_index(
        &self,
        repository_root: impl AsRef<Path>,
    ) -> Result<(SymbolIndex, SymbolLoad)> {
        let index = IndexCache::get_or_build(&self.indexer, repository_root)?;
        let (map, _) = RepositoryMapStore::new(&self.config.data_dir, self.audit_log.clone())
            .load_or_build(&index, &self.command_policy)?;
        SymbolIndexStore::new(&self.config.data_dir, self.audit_log.clone()).load_or_build(
            &index,
            &map,
            &self.path_policy,
        )
    }

    /// [`SymbolIndex::find`] over [`Self::symbol_index`].
    pub fn find_symbols(
        &self,
        repository_root: impl AsRef<Path>,
        query: &str,
        limit: usize,
    ) -> Result<Vec<SymbolMatch>> {
        let (index, _) = self.symbol_index(repository_root)?;
        Ok(index.find(query, limit))
    }

    /// The process-wide language-server hub (spec 25 `context.md` §3).
    pub fn language_servers(&self) -> &'static LanguageServerHub {
        LanguageServerHub::global()
    }

    /// Starts the eager definition pass for this repository on the global
    /// hub (spec 25 `context.md` §8). See
    /// [`Self::start_definition_pass_with`].
    pub fn start_definition_pass(
        &self,
        repository_root: impl AsRef<Path>,
        session_id: &str,
    ) -> Result<Option<DefinitionPass>> {
        self.start_definition_pass_with(LanguageServerHub::global(), repository_root, session_id)
    }

    /// Starts [`run_definition_pass`] on a thread of its own and returns at
    /// once. `None` when no language server is configured, or while a pass
    /// for this repository is already running in this process. Nothing waits
    /// for it: [`Self::find_symbols`] and every other reader take the stored
    /// index as the pass leaves it, a mixture of both tiers until it is done.
    pub fn start_definition_pass_with<H>(
        &self,
        hub: H,
        repository_root: impl AsRef<Path>,
        session_id: &str,
    ) -> Result<Option<DefinitionPass>>
    where
        H: Deref<Target = LanguageServerHub> + Send + 'static,
    {
        let root = repository_root.as_ref().to_path_buf();
        let launches: BTreeMap<String, LanguageServerLaunch> = self
            .config
            .language_servers
            .keys()
            .filter_map(|language| {
                self.language_server_launch(&root, language, session_id)
                    .map(|launch| (language.clone(), launch))
            })
            .collect();
        let Some(repository_id) = launches.values().next().map(|launch| &launch.repository_id)
        else {
            return Ok(None);
        };
        let Some(claim) = PassClaim::claim(repository_id) else {
            return Ok(None);
        };
        let indexer = self.indexer.clone();
        let maps = RepositoryMapStore::new(&self.config.data_dir, self.audit_log.clone());
        let command_policy = self.command_policy.clone();
        let path_policy = self.path_policy.clone();
        let store = SymbolIndexStore::new(&self.config.data_dir, self.audit_log.clone());
        let scanner = self.scanner.clone();
        DefinitionPass::spawn(claim, move || {
            let index = IndexCache::get_or_build(&indexer, &root)?;
            let (map, _) = maps.load_or_build(&index, &command_policy)?;
            run_definition_pass(
                &hub,
                &launches,
                &index,
                &map,
                &path_policy,
                &store,
                &scanner,
            )
        })
        .map(Some)
    }

    /// What the hub needs to start `language`'s server for this repository,
    /// or `None` when this config names none. The config is the one loaded
    /// for the repository, so a server its own config tried to name was
    /// already refused.
    pub fn language_server_launch(
        &self,
        repository_root: impl AsRef<Path>,
        language: &str,
        session_id: &str,
    ) -> Option<LanguageServerLaunch> {
        LanguageServerLaunch::from_config(
            &self.config,
            repository_root.as_ref(),
            language,
            session_id,
        )
    }
}

impl Default for WorkspaceEngine {
    fn default() -> Self {
        Self::new(Config::default())
    }
}
