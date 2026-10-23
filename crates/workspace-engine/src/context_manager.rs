use crate::command_policy::CommandPolicy;
use crate::file_access::FileAccessController;
use crate::indexer::RepositoryIndex;
use crate::path_policy::PathPolicy;
use crate::repository_map::{RepositoryMap, RepositoryMapStore};
use crate::secret_scanner::SecretScanner;
use crate::symbol_index::SymbolIndexStore;
use crate::vector_index::VectorIndexCache;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

pub(crate) const AGENT_INSTRUCTIONS_FILE: &str = "AGENTS.md";
const PROJECT_RULES: &[&str] = &[
    "README.md",
    "CONTRIBUTING.md",
    ".editorconfig",
    "package.json",
    "pyproject.toml",
    "Cargo.toml",
    "go.mod",
    "pom.xml",
    "build.gradle",
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContextItem {
    pub kind: String,
    pub path: Option<String>,
    pub content: String,
    pub tokens: usize,
    pub redaction_status: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContextPlan {
    pub repository_id: String,
    pub task_id: String,
    pub token_estimate: usize,
    pub items: Vec<ContextItem>,
    pub files: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct ContextManager {
    file_access: FileAccessController,
    scanner: SecretScanner,
    data_dir: PathBuf,
    enable_semantic_search: bool,
    repository_map: RepositoryMapSource,
    symbols: SymbolSource,
}

/// What the `repository_map` item is built from (spec 24 `context.md` §5).
#[derive(Debug, Clone)]
pub struct RepositoryMapSource {
    pub store: RepositoryMapStore,
    /// The policy that classifies the map's commands, the engine's own.
    pub command_policy: CommandPolicy,
    /// `repository_map_max_tokens`. `0` leaves the item out.
    pub max_tokens: usize,
}

/// What the `symbols` item is built from (spec 25 `context.md` §9).
#[derive(Debug, Clone)]
pub struct SymbolSource {
    pub store: SymbolIndexStore,
    /// The engine's own, which keeps restricted files out of the index.
    pub path_policy: PathPolicy,
    /// `symbol_context_max_tokens`. `0` leaves the item out.
    pub max_tokens: usize,
}

impl ContextManager {
    pub fn new(
        file_access: FileAccessController,
        scanner: SecretScanner,
        data_dir: PathBuf,
        enable_semantic_search: bool,
        repository_map: RepositoryMapSource,
        symbols: SymbolSource,
    ) -> Self {
        Self {
            file_access,
            scanner,
            data_dir,
            enable_semantic_search,
            repository_map,
            symbols,
        }
    }

    // Each argument is an independent input to the context plan; a params
    // struct would be constructed at the single call site and immediately
    // destructured here.
    #[allow(clippy::too_many_arguments)]
    pub fn build_context(
        &self,
        repository_root: impl AsRef<Path>,
        repository_id: &str,
        task_id: &str,
        prompt: &str,
        index: Option<&RepositoryIndex>,
        explicit_paths: &[String],
        token_budget: usize,
    ) -> ContextPlan {
        let mut items = Vec::new();
        let mut files = Vec::new();
        let mut token_estimate = 0;

        add_text(
            &self.scanner,
            &mut items,
            &mut token_estimate,
            token_budget,
            "user_prompt",
            None,
            prompt,
        );

        let requested_paths: Vec<(String, bool)> = explicit_paths
            .iter()
            .map(|path| (path.clone(), true))
            .collect();
        let mentioned_paths = prompt_file_mentions(prompt, index);
        let mut context_paths = requested_paths
            .iter()
            .map(|(path, _)| path.clone())
            .collect::<Vec<_>>();
        for path in &mentioned_paths {
            if !context_paths.iter().any(|existing| existing == path) {
                context_paths.push(path.clone());
            }
        }

        let search_results = index.map(|index| {
            let mut results = index.keyword_search(prompt, 8);
            results.extend(if self.enable_semantic_search {
                VectorIndexCache::semantic_search(&self.data_dir, index, prompt, 8)
            } else {
                index.semantic_search(prompt, 8)
            });
            results
        });
        if let Some(results) = &search_results {
            for result in results {
                if !context_paths
                    .iter()
                    .any(|existing| existing == &result.path)
                {
                    context_paths.push(result.path.clone());
                }
            }
        }

        for path in context_paths.iter().filter(|path| {
            requested_paths
                .iter()
                .any(|(requested, _)| requested == *path)
                || mentioned_paths.iter().any(|mentioned| mentioned == *path)
        }) {
            let allow_outside_root = requested_paths
                .iter()
                .find(|(requested, _)| *requested == *path)
                .map(|(_, allow)| *allow)
                .unwrap_or(false);
            self.add_file(
                repository_root.as_ref(),
                repository_id,
                task_id,
                path,
                "explicit_file",
                &mut files,
                &mut items,
                &mut token_estimate,
                token_budget,
                allow_outside_root,
            );
        }

        for instruction_path in agent_instruction_paths(&context_paths) {
            self.add_file(
                repository_root.as_ref(),
                repository_id,
                task_id,
                &instruction_path,
                "agent_instruction",
                &mut files,
                &mut items,
                &mut token_estimate,
                token_budget,
                false,
            );
        }

        for rule_path in PROJECT_RULES {
            self.add_file(
                repository_root.as_ref(),
                repository_id,
                task_id,
                rule_path,
                "project_rule",
                &mut files,
                &mut items,
                &mut token_estimate,
                token_budget,
                false,
            );
        }

        // After instructions and rules, before retrieved files: under a tight
        // budget the map displaces retrieved files, never instructions.
        let map = index.and_then(|index| self.load_map(index));
        if let Some(text) = map.as_ref().and_then(|map| self.repository_map_text(map)) {
            add_text(
                &self.scanner,
                &mut items,
                &mut token_estimate,
                token_budget,
                "repository_map",
                None,
                &text,
            );
        }

        // The symbols the prompt names, in the same place for the same
        // reason: it displaces retrieved files, never instructions or the map.
        if let Some(text) = index
            .zip(map.as_ref())
            .and_then(|(index, map)| self.symbols_text(index, map, prompt))
        {
            add_text(
                &self.scanner,
                &mut items,
                &mut token_estimate,
                token_budget,
                "symbols",
                None,
                &text,
            );
        }

        if let Some(results) = search_results {
            for result in results {
                self.add_file(
                    repository_root.as_ref(),
                    repository_id,
                    task_id,
                    &result.path,
                    "retrieved_file",
                    &mut files,
                    &mut items,
                    &mut token_estimate,
                    token_budget,
                    false,
                );
            }
        }

        ContextPlan {
            repository_id: repository_id.to_string(),
            task_id: task_id.to_string(),
            token_estimate,
            items,
            files,
        }
    }

    /// The map both items are built from, or `None` when both ceilings are
    /// `0` or the map cannot be loaded or stored. The map is an aid: a
    /// read-only or broken data directory leaves it out rather than failing
    /// the turn.
    fn load_map(&self, index: &RepositoryIndex) -> Option<RepositoryMap> {
        if self.repository_map.max_tokens == 0 && self.symbols.max_tokens == 0 {
            return None;
        }
        let source = &self.repository_map;
        let (map, _) = source
            .store
            .load_or_build(index, &source.command_policy)
            .ok()?;
        Some(map)
    }

    /// The rendered map, or `None` when its ceiling is `0`.
    fn repository_map_text(&self, map: &RepositoryMap) -> Option<String> {
        let max_tokens = self.repository_map.max_tokens;
        (max_tokens > 0).then(|| map.render_for_model(max_tokens).text)
    }

    /// The symbols the prompt's identifiers name, or `None` when the ceiling
    /// is `0`, nothing matches, or the symbol index cannot be loaded or
    /// stored, which leaves the item out rather than failing the turn. It
    /// reads the stored index as it is, whatever an eager definition pass has
    /// merged so far, and never waits on a language server (`context.md`
    /// §8).
    fn symbols_text(
        &self,
        index: &RepositoryIndex,
        map: &RepositoryMap,
        prompt: &str,
    ) -> Option<String> {
        let source = &self.symbols;
        if source.max_tokens == 0 {
            return None;
        }
        let (symbols, _) = source
            .store
            .load_or_build(index, map, &source.path_policy)
            .ok()?;
        let matches = symbols.find_for_prompt(prompt);
        if matches.is_empty() {
            return None;
        }
        let rendered = symbols.render_for_model(&matches, source.max_tokens);
        (!rendered.text.is_empty()).then_some(rendered.text)
    }

    #[allow(clippy::too_many_arguments)]
    fn add_file(
        &self,
        repository_root: &Path,
        repository_id: &str,
        task_id: &str,
        path: &str,
        kind: &str,
        files: &mut Vec<String>,
        items: &mut Vec<ContextItem>,
        token_estimate: &mut usize,
        token_budget: usize,
        allow_outside_root: bool,
    ) {
        if files.iter().any(|existing| existing == path) {
            return;
        }
        // `Whole`, not the tool's bounded window: this function has its own
        // `token_budget` and `add_text` decides what fits. Capping here would
        // silently shrink what every task sees. Spec 26 owns the ranged version.
        let Ok(file) = self.file_access.read_file(
            repository_root,
            path,
            Some(task_id),
            Some(repository_id),
            false,
            allow_outside_root,
            crate::file_access::ReadWindow::Whole,
        ) else {
            return;
        };
        let added = add_text(
            &self.scanner,
            items,
            token_estimate,
            token_budget,
            kind,
            Some(file.path.clone()),
            &file.content,
        );
        if added {
            files.push(file.path);
        }
    }
}

fn add_text(
    scanner: &SecretScanner,
    items: &mut Vec<ContextItem>,
    token_estimate: &mut usize,
    token_budget: usize,
    kind: &str,
    path: Option<String>,
    content: &str,
) -> bool {
    if content.is_empty() {
        return false;
    }
    let redaction = scanner.redact(content);
    let tokens = redaction.text.len().div_ceil(4);
    if *token_estimate + tokens > token_budget {
        return false;
    }
    *token_estimate += tokens;
    items.push(ContextItem {
        kind: kind.to_string(),
        path,
        content: redaction.text,
        tokens,
        redaction_status: if redaction.findings.is_empty() {
            "clean".to_string()
        } else {
            "redacted".to_string()
        },
    });
    true
}

pub(crate) fn agent_instruction_paths(context_paths: &[String]) -> Vec<String> {
    let mut paths = vec![AGENT_INSTRUCTIONS_FILE.to_string()];
    for context_path in context_paths {
        if context_path.starts_with('/') || context_path.contains("../") || context_path == ".." {
            continue;
        }
        let normalized = context_path.trim_start_matches("./").replace('\\', "/");
        let mut directories = normalized.split('/').collect::<Vec<_>>();
        directories.pop();

        let mut current = String::new();
        for directory in directories {
            if directory.is_empty() {
                continue;
            }
            if !current.is_empty() {
                current.push('/');
            }
            current.push_str(directory);
            paths.push(format!("{current}/{AGENT_INSTRUCTIONS_FILE}"));
        }
    }

    let mut seen = HashSet::new();
    paths
        .into_iter()
        .filter(|path| seen.insert(path.clone()))
        .collect()
}

fn prompt_file_mentions(prompt: &str, index: Option<&RepositoryIndex>) -> Vec<String> {
    let Some(index) = index else {
        return Vec::new();
    };

    let paths = index
        .files
        .iter()
        .map(|file| file.path.clone())
        .collect::<Vec<_>>();
    let exact_paths = paths
        .iter()
        .map(|path| (path.to_lowercase(), path.clone()))
        .collect::<HashMap<_, _>>();
    let mut basename_matches: HashMap<String, Vec<String>> = HashMap::new();
    for path in &paths {
        if let Some(name) = path.rsplit('/').next() {
            basename_matches
                .entry(name.to_lowercase())
                .or_default()
                .push(path.clone());
        }
    }

    let mut mentioned = Vec::new();
    let mut seen = HashSet::new();
    for candidate in prompt_path_candidates(prompt) {
        let lower = candidate.to_lowercase();
        let resolved = if let Some(path) = exact_paths.get(&lower) {
            Some(path.clone())
        } else if !candidate.contains('/') {
            basename_matches
                .get(&lower)
                .filter(|matches| matches.len() == 1)
                .and_then(|matches| matches.first().cloned())
        } else {
            None
        };

        if let Some(path) = resolved
            && seen.insert(path.clone())
        {
            mentioned.push(path);
        }
    }

    mentioned
}

fn prompt_path_candidates(prompt: &str) -> Vec<String> {
    prompt
        .split_whitespace()
        .filter_map(|part| {
            let candidate = part
                .trim_matches(|character: char| {
                    matches!(
                        character,
                        '`' | '"'
                            | '\''
                            | '('
                            | ')'
                            | '['
                            | ']'
                            | '{'
                            | '}'
                            | '<'
                            | '>'
                            | ','
                            | ':'
                            | ';'
                    )
                })
                .trim_end_matches(['.', '?', '!'])
                .replace('\\', "/");

            if candidate.is_empty()
                || candidate.starts_with('/')
                || candidate.starts_with("http://")
                || candidate.starts_with("https://")
                || candidate.contains("../")
                || candidate == ".."
                || candidate.ends_with('/')
            {
                return None;
            }

            let looks_like_path = candidate.contains('/')
                || candidate
                    .rsplit('/')
                    .next()
                    .is_some_and(|name| name.contains('.'));
            looks_like_path.then_some(candidate)
        })
        .collect()
}
