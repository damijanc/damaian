use crate::audit::AuditLog;
use crate::chat::ChatOrchestrator;
use crate::checkpoint::CheckpointStore;
use crate::command_policy::CommandPolicy;
use crate::command_runner::CommandRunner;
use crate::config::{Config, RepositoryConfigReport};
use crate::context_manager::{ContextManager, RepositoryMapSource, SymbolSource};
use crate::edit::{EditOrchestrator, PatchStore};
use crate::error::{ClientError, Result};
use crate::file_access::FileAccessController;
use crate::finding::{Finding, FindingSource};
use crate::git_service::{GitReadCaps, GitService};
use crate::hash::{file_hash, repository_id_for_root};
use crate::hooks::{HookContext, HookDispatcher, HookEvent};
use crate::index_cache::IndexCache;
use crate::indexer::ProjectIndexer;
use crate::language::detect_language;
use crate::lsp::{
    DefinitionPass, LanguageServerHub, LanguageServerLaunch, PassClaim, Position, RepositoryUris,
    run_definition_pass, symbol_refs_from_locations,
};
use crate::navigation::NavigationController;
use crate::patch_engine::PatchEngine;
use crate::path_policy::PathPolicy;
use crate::repository_map::{MapLoad, RepositoryMap, RepositoryMapStore};
use crate::repository_trust::RepositoryTrustStore;
use crate::secret_scanner::SecretScanner;
use crate::session::SessionStore;
use crate::symbol_index::{
    CachedRelationSet, Provenance, Relation, RelationKind, RelationTarget, SymbolIndex,
    SymbolIndexStore, SymbolLoad, SymbolMatch, SymbolRef, SymbolReferences,
};
use crate::validation::{CommandStore, ValidationOrchestrator};
use std::collections::{BTreeMap, BTreeSet};
use std::ops::Deref;
use std::path::Path;

#[derive(Debug, Clone)]
pub struct WorkspaceEngine {
    pub config: Config,
    pub scanner: SecretScanner,
    pub audit_log: AuditLog,
    pub hooks: HookDispatcher,
    pub path_policy: PathPolicy,
    pub file_access: FileAccessController,
    pub indexer: ProjectIndexer,
    pub context_manager: ContextManager,
    pub command_policy: CommandPolicy,
    pub command_runner: CommandRunner,
    pub git: GitService,
    pub patch_engine: PatchEngine,
    pub commit: crate::commit::CommitEngine,
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
            path_policy.clone(),
            scanner.clone(),
            config.data_dir.clone(),
            config.enable_semantic_search,
            RepositoryMapSource {
                store: RepositoryMapStore::new(&config.data_dir, audit_log.clone()),
                command_policy: command_policy.clone(),
                max_tokens: config.repository_map_max_tokens,
            },
            SymbolSource {
                store: SymbolIndexStore::new(&config.data_dir, audit_log.clone()),
                path_policy: path_policy.clone(),
                max_tokens: config.symbol_context_max_tokens,
            },
            config.context_budget,
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
        let hooks = HookDispatcher::new(
            config.clone(),
            scanner.clone(),
            audit_log.clone(),
            validation_orchestrator.clone(),
            session_store.clone(),
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
            hooks.clone(),
        );
        let commit = crate::commit::CommitEngine::new(
            config.clone(),
            audit_log.clone(),
            scanner.clone(),
            path_policy.clone(),
            session_store.clone(),
            checkpoint_store.clone(),
            git.clone(),
            command_runner.clone(),
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
            hooks.clone(),
        );
        Self {
            config,
            scanner,
            audit_log,
            hooks,
            path_policy,
            file_access,
            indexer,
            context_manager,
            command_policy,
            command_runner,
            git,
            patch_engine,
            commit,
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

    /// Ends a session (spec 32 `context.md` §3): `session_end` hooks first,
    /// then the delete itself.
    ///
    /// The outcome is audit-only (§4). A hook cannot cancel a user's explicit
    /// delete, because refusing it is not a narrowing anyone asked for; a
    /// `deny` is recorded in the audit log and the session still goes.
    ///
    /// The repository root is an argument because a session stores only its
    /// repository id, and a hook process needs a working directory and a
    /// repository to run in. It must be the session's own repository: the
    /// root comes from a caller (the shell takes it from the request form),
    /// and running checkout B's hooks with checkout A's session in the
    /// envelope would show one repository another repository's session.
    pub fn delete_session(
        &self,
        session_id: &str,
        repository_root: impl AsRef<Path>,
    ) -> Result<()> {
        let root = repository_root.as_ref();
        if let Some(session) = self.session_store.read_session(session_id)? {
            if repository_id_for_root(root) != session.repository_id {
                return Err(ClientError::InvalidInput(format!(
                    "Session {session_id} does not belong to repository {}",
                    root.display()
                )));
            }
            let ctx = HookContext::root();
            let parts = self
                .hooks
                .envelope_parts(Some(&session), None, serde_json::json!({}));
            let mut outcome = self
                .hooks
                .dispatch(HookEvent::SessionEnd, parts, root, &ctx);
            // Nothing can be recorded against a log that is about to go, but a
            // requested command at an event with no task is still refused and
            // audited rather than dropped (`context.md` §9).
            let mode = self.session_store.session_mode(&session.id);
            let _ =
                self.hooks
                    .run_requested_commands(&mut outcome, &session, None, mode, root, &ctx);
        }
        self.session_store.delete_session(session_id)
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

    /// The shell's symbol search: [`Self::find_symbols`] over the current
    /// snapshot, then an eager definition pass in the background so a
    /// configured language server can improve the *next* read. The matches
    /// returned are the snapshot from before the pass starts, and starting it
    /// returns at once, so neither this request nor a turn waits on a server.
    /// No `language_server.<language>` means the pass is not started at all.
    pub fn symbol_search(
        &self,
        repository_root: impl AsRef<Path>,
        query: &str,
        limit: usize,
    ) -> Result<Vec<SymbolMatch>> {
        self.symbol_search_with(LanguageServerHub::global(), repository_root, query, limit)
    }

    /// [`Self::symbol_search`] against `hub`, for tests that must not use the
    /// global hub.
    pub fn symbol_search_with<H>(
        &self,
        hub: H,
        repository_root: impl AsRef<Path>,
        query: &str,
        limit: usize,
    ) -> Result<Vec<SymbolMatch>>
    where
        H: Deref<Target = LanguageServerHub> + Send + 'static,
    {
        let root = repository_root.as_ref().to_path_buf();
        let matches = self.find_symbols(&root, query, limit)?;
        // A pass that cannot start costs only the next read its improvement;
        // it must never turn a successful search into an error.
        let _ = self.start_definition_pass_with(hub, &root, SYMBOL_SEARCH_SESSION);
        Ok(matches)
    }

    /// The language server's references for `symbol`, fetched on demand
    /// (proposal §5.5). See [`Self::symbol_references_with`].
    pub fn symbol_references(
        &self,
        repository_root: impl AsRef<Path>,
        symbol: &SymbolRef,
    ) -> Result<SymbolReferences> {
        self.symbol_references_with(LanguageServerHub::global(), repository_root, symbol)
    }

    /// `textDocument/references` for `symbol`, with `includeDeclaration:
    /// false`, answered by the language server configured for the symbol's
    /// language. This is the one place a caller waits on a server, because the
    /// user asked for this symbol directly; the wait is the hub's request
    /// timeout. Without a configured server, or if it is unavailable or times
    /// out, the result is empty and `available` is false: references are only
    /// reported where they are reliable, never guessed from source text.
    ///
    /// The answer is cached under the content hash of the defining file and
    /// every referencing file, and returned again only while all of them still
    /// match; otherwise it is fetched again.
    pub fn symbol_references_with<H>(
        &self,
        hub: H,
        repository_root: impl AsRef<Path>,
        symbol: &SymbolRef,
    ) -> Result<SymbolReferences>
    where
        H: Deref<Target = LanguageServerHub>,
    {
        self.symbol_relations_with(hub, repository_root, symbol, RelationKind::References)
    }

    /// The language server's implementations for `symbol`, fetched on demand
    /// like [`Self::symbol_references`].
    pub fn symbol_implementations(
        &self,
        repository_root: impl AsRef<Path>,
        symbol: &SymbolRef,
    ) -> Result<SymbolReferences> {
        self.symbol_implementations_with(LanguageServerHub::global(), repository_root, symbol)
    }

    /// `textDocument/implementation` for `symbol`, with the same availability
    /// and cache rules as [`Self::symbol_references_with`].
    pub fn symbol_implementations_with<H>(
        &self,
        hub: H,
        repository_root: impl AsRef<Path>,
        symbol: &SymbolRef,
    ) -> Result<SymbolReferences>
    where
        H: Deref<Target = LanguageServerHub>,
    {
        self.symbol_relations_with(hub, repository_root, symbol, RelationKind::Implements)
    }

    fn symbol_relations_with(
        &self,
        hub: impl Deref<Target = LanguageServerHub>,
        repository_root: impl AsRef<Path>,
        symbol: &SymbolRef,
        kind: RelationKind,
    ) -> Result<SymbolReferences> {
        let root = repository_root.as_ref();
        let canonical = std::fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
        let language = detect_language(&symbol.path);
        let unavailable = |reason: String| SymbolReferences {
            kind,
            symbol: symbol.clone(),
            language: Some(language.to_string()),
            available: false,
            reason: Some(reason),
            relations: Vec::new(),
        };
        // A restricted file is never sent to a server (`context.md` §7).
        if self.path_policy.is_restricted(&symbol.path, false) {
            return Ok(unavailable(format!("{} is restricted", symbol.path)));
        }
        let Some(launch) = self.language_server_launch(&canonical, language, SYMBOL_LOOKUP_SESSION)
        else {
            return Ok(unavailable(format!("no language server for {language}")));
        };
        let store = SymbolIndexStore::new(&self.config.data_dir, self.audit_log.clone());
        if let Some(relations) =
            store.cached_relations(&launch.repository_id, &canonical, symbol, kind)?
        {
            return Ok(SymbolReferences {
                kind,
                symbol: symbol.clone(),
                language: Some(launch.language.clone()),
                available: true,
                reason: None,
                relations,
            });
        }

        // The name's column, when the index has seen the declaration. A
        // lookup is still worth making without it: the server has the file.
        let index = self.symbol_index(&canonical).ok().map(|(index, _)| index);
        let column = index
            .as_ref()
            .and_then(|index| {
                index
                    .symbols
                    .iter()
                    .find(|candidate| {
                        candidate.path == symbol.path
                            && candidate.name == symbol.name
                            && candidate.line == symbol.line
                    })
                    .and_then(|candidate| candidate.column)
            })
            .unwrap_or(1);
        let position = Position::from_one_based(symbol.line, column);
        let response = match kind {
            RelationKind::References => hub.request(&launch, |client, uris| {
                client.references(&uris.uri_for(&symbol.path), position, false)
            }),
            RelationKind::Implements => hub.request(&launch, |client, uris| {
                client.implementation(&uris.uri_for(&symbol.path), position)
            }),
            other => {
                return Err(crate::error::ClientError::InvalidInput(format!(
                    "{other:?} is not an on-demand relation kind"
                )));
            }
        };
        let value = match response {
            Ok(value) => value,
            Err(error) => return Ok(unavailable(error.to_string())),
        };

        let uris = RepositoryUris::new(&launch.root);
        let mut targets = symbol_refs_from_locations(&value, &uris);
        targets.retain(|target| !self.path_policy.is_restricted(&target.path, false));
        for target in &mut targets {
            target.name = target_name(index.as_ref(), target, symbol, kind);
        }
        let mut relations: Vec<Relation> = targets
            .into_iter()
            .map(|target| Relation {
                kind,
                from: symbol.clone(),
                to: RelationTarget::Symbol(target),
                provenance: Provenance::LanguageServer,
            })
            .collect();
        relations.sort_by(|left, right| {
            (&left.from, left.kind, &left.to).cmp(&(&right.from, right.kind, &right.to))
        });
        relations.dedup();

        let mut hashes = BTreeMap::new();
        let mut cacheable = true;
        let referenced = relations.iter().filter_map(|relation| match &relation.to {
            RelationTarget::Symbol(target) => Some(target.path.as_str()),
            _ => None,
        });
        for path in std::iter::once(symbol.path.as_str()).chain(referenced) {
            match file_hash(canonical.join(path)) {
                Ok(hash) => {
                    hashes.insert(path.to_string(), hash);
                }
                Err(_) => cacheable = false,
            }
        }
        if cacheable {
            // Caching is an optimisation; a write that fails only costs the
            // next lookup another request.
            let _ = store.cache_relations(
                &launch.repository_id,
                &canonical,
                CachedRelationSet {
                    kind,
                    symbol: symbol.clone(),
                    relations: relations.clone(),
                    hashes,
                },
            );
        }
        Ok(SymbolReferences {
            kind,
            symbol: symbol.clone(),
            language: Some(launch.language.clone()),
            available: true,
            reason: None,
            relations,
        })
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

/// The session id recorded for a server that a user's on-demand lookup
/// started. There is no turn or session in hand; the registry keeps it for
/// the audit trail (`context.md` §3).
const SYMBOL_LOOKUP_SESSION: &str = "symbol-lookup";

/// The session id recorded for the eager pass a symbol search starts. Like
/// [`SYMBOL_LOOKUP_SESSION`], it exists for the registry's audit trail; the
/// pass belongs to the process, not to a conversation.
const SYMBOL_SEARCH_SESSION: &str = "symbol-search";

/// The name to give one `references` or `implementation` location. A
/// declaration in the index at that place names itself; a reference site has
/// no declaration there, so it carries the queried symbol's name.
fn target_name(
    index: Option<&SymbolIndex>,
    target: &SymbolRef,
    queried: &SymbolRef,
    kind: RelationKind,
) -> String {
    // A reference site names the symbol it refers to, whatever declaration
    // happens to start on that line.
    if kind == RelationKind::References {
        return queried.name.clone();
    }
    if let Some(found) = index.and_then(|index| {
        index
            .symbols
            .iter()
            .find(|symbol| symbol.path == target.path && symbol.line == target.line)
    }) {
        return found.name.clone();
    }
    String::new()
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
