use crate::audit::AuditLog;
use crate::chat::ChatOrchestrator;
use crate::checkpoint::CheckpointStore;
use crate::command_policy::CommandPolicy;
use crate::command_runner::CommandRunner;
use crate::config::{Config, RepositoryConfigReport};
use crate::context_manager::{ContextManager, RepositoryMapSource};
use crate::edit::{EditOrchestrator, PatchStore};
use crate::error::Result;
use crate::file_access::FileAccessController;
use crate::finding::{Finding, FindingSource};
use crate::git_service::{GitReadCaps, GitService};
use crate::hash::{file_hash, repository_id_for_root};
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
use std::collections::{BTreeMap, BTreeSet};
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
        // No `RepositoryConfigReport`, so the classifier cannot attribute a
        // setting's source; `CommandPolicy::new` leaves it `None` (unknown)
        // rather than falsely blaming "default" (spec 62 Correction C4).
        Self::build(config.clone(), CommandPolicy::new(config))
    }

    /// [`Self::new`] with the source attribution recorded at load, so the
    /// classifier's `Setting` cause can name where each `require_approval_for_*`
    /// key came from without re-resolving config (spec 62 Correction C4).
    pub fn new_with_report(config: Config, report: &RepositoryConfigReport) -> Self {
        Self::build(
            config.clone(),
            CommandPolicy::new_with_report(config, report),
        )
    }

    fn build(config: Config, command_policy: CommandPolicy) -> Self {
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
        let git = GitService::new(
            audit_log.clone(),
            scanner.clone(),
            path_policy.clone(),
            GitReadCaps::from_config(&config),
        );
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

    /// Records the global hub's diagnostics for this repository as findings.
    /// See [`Self::record_language_server_findings_with`].
    pub fn record_language_server_findings(
        &self,
        session_id: &str,
        repository_root: impl AsRef<Path>,
    ) -> Result<usize> {
        self.record_language_server_findings_with(
            LanguageServerHub::global(),
            session_id,
            repository_root,
        )
    }

    /// Records, in `session_id`, each diagnostic `hub` holds for this
    /// repository that the session does not hold yet, and returns how many
    /// it recorded (spec 25 `context.md` §10). A diagnostic is held when a
    /// language-server finding with the same range (path included), code,
    /// summary and file hash is in the session, whatever its status: a
    /// dismissed one stays dismissed, and a file the server has since
    /// cleared removes nothing, because statuses belong to the user (spec
    /// 22). No task id: a finding recorded outside the turn loop is linked
    /// to no plan step and cannot block one.
    ///
    /// It reads what the hub holds and never waits on or starts a server.
    pub fn record_language_server_findings_with(
        &self,
        hub: &LanguageServerHub,
        session_id: &str,
        repository_root: impl AsRef<Path>,
    ) -> Result<usize> {
        let root = repository_root.as_ref();
        // Canonical, as the hub keys its entries (`LanguageServerLaunch`).
        let canonical = std::fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
        let files = hub.diagnostics(&repository_id_for_root(&canonical));
        if files.iter().all(|file| file.findings.is_empty()) {
            return Ok(0);
        }
        let mut held: BTreeSet<FindingKey> = self
            .session_store
            .read_findings(session_id, root)?
            .iter()
            .filter(|finding| finding.source() == FindingSource::LanguageServer)
            .map(finding_key)
            .collect();
        let mut recorded = 0;
        for file in files {
            // The hub dropped these already, by the policy its entry was
            // started with; this is the policy in force now.
            if file.findings.is_empty() || self.path_policy.is_restricted(&file.path, false) {
                continue;
            }
            // A file gone since the server published has nothing to point at.
            let Ok(hash) = file_hash(canonical.join(&file.path)) else {
                continue;
            };
            for draft in file.findings {
                let finding = Finding::new(draft, &self.scanner).with_file_hash(hash.clone());
                if held.insert(finding_key(&finding)) {
                    self.session_store.record_finding(session_id, &finding)?;
                    recorded += 1;
                }
            }
        }
        Ok(recorded)
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

/// A range's path, lines and columns.
type RangeKey = (String, u32, Option<u32>, Option<u32>, Option<u32>);

/// What makes two language-server findings the same diagnostic: range, code,
/// summary (as redacted) and file hash.
type FindingKey = (Option<RangeKey>, Option<String>, String, Option<String>);

fn finding_key(finding: &Finding) -> FindingKey {
    (
        finding.range().map(|range| {
            (
                range.path.clone(),
                range.start_line,
                range.start_column,
                range.end_line,
                range.end_column,
            )
        }),
        finding.code().map(str::to_string),
        finding.summary().to_string(),
        finding.file_hash().map(str::to_string),
    )
}

impl Default for WorkspaceEngine {
    fn default() -> Self {
        Self::new(Config::default())
    }
}
