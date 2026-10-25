use crate::command_policy::CommandPolicy;
use crate::file_access::{FileAccessController, LineRange};
use crate::indexer::RepositoryIndex;
use crate::path_policy::PathPolicy;
use crate::repository_map::{RepositoryMap, RepositoryMapStore};
use crate::secret_scanner::SecretScanner;
use crate::symbol_index::{Provenance, SymbolIndexStore};
use crate::vector_index::VectorIndexCache;
use serde::{Deserialize, Serialize};
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

/// The budget and priority bucket an item belongs to (proposal §5.1, §5.2).
///
/// Declaration order is the pipeline order and therefore the item order under
/// a tight budget: `Ord` is derived so priority cannot drift from the enum.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContextCategory {
    /// Never dropped. The user's own message.
    Required,
    /// The `AGENTS.md` hierarchy and root project rules.
    Instructions,
    /// A rule or note the user installed through a hook (spec 32).
    HookContext,
    /// User-selected context (spec 27; reserved in spec 26).
    Pinned,
    PlanState,
    RepositoryMap,
    Symbols,
    SearchResults,
    FileRanges,
    /// Reserved for spec 22's repair subset.
    Findings,
    /// Reserved for spec 55.
    ConversationSummary,
}

impl ContextCategory {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Required => "required",
            Self::Instructions => "instructions",
            Self::HookContext => "hook_context",
            Self::Pinned => "pinned",
            Self::PlanState => "plan_state",
            Self::RepositoryMap => "repository_map",
            Self::Symbols => "symbols",
            Self::SearchResults => "search_results",
            Self::FileRanges => "file_ranges",
            Self::Findings => "findings",
            Self::ConversationSummary => "conversation_summary",
        }
    }
}

/// The source label the prompt header shows.
///
/// Separate from [`ContextCategory`] because the model distinguishes "you asked
/// for this file" from "search found it", and the spelling is the one every
/// existing prompt already uses, so a range-less item renders byte-identically.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContextKind {
    UserPrompt,
    ExplicitFile,
    AgentInstruction,
    ProjectRule,
    HookContext,
    RepositoryMap,
    Symbols,
    RetrievedFile,
    PlanState,
    Pinned,
}

impl ContextKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::UserPrompt => "user_prompt",
            Self::ExplicitFile => "explicit_file",
            Self::AgentInstruction => "agent_instruction",
            Self::ProjectRule => "project_rule",
            Self::HookContext => "hook_context",
            Self::RepositoryMap => "repository_map",
            Self::Symbols => "symbols",
            Self::RetrievedFile => "retrieved_file",
            Self::PlanState => "plan_state",
            Self::Pinned => "pinned",
        }
    }

    /// The one place the kind-to-category decision lives (proposal §5.1).
    pub fn category(self) -> ContextCategory {
        match self {
            Self::UserPrompt => ContextCategory::Required,
            Self::AgentInstruction | Self::ProjectRule => ContextCategory::Instructions,
            Self::HookContext => ContextCategory::HookContext,
            Self::Pinned => ContextCategory::Pinned,
            Self::PlanState => ContextCategory::PlanState,
            Self::RepositoryMap => ContextCategory::RepositoryMap,
            Self::Symbols => ContextCategory::Symbols,
            Self::RetrievedFile | Self::ExplicitFile => ContextCategory::FileRanges,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContextItem {
    pub category: ContextCategory,
    pub kind: ContextKind,
    pub path: Option<String>,
    /// `None` means the whole file, or content with no file identity. Serialised
    /// as `lineRange` so a reader cannot mistake it for an index range.
    #[serde(rename = "lineRange")]
    pub range: Option<LineRange>,
    pub content: String,
    pub tokens: usize,
    pub redaction_status: String,
    /// The lowest provenance among what this item contains (spec 25); `None`
    /// for content that has no symbol provenance at all.
    pub provenance: Option<Provenance>,
    /// The repository root that owns `path` (spec 24); `None` when the item has
    /// no file identity.
    pub root_path: Option<String>,
}

/// One named block of the rendered user message, in stable-to-volatile order.
///
/// Named so spec 49's reuse slice can place a cache breakpoint at a boundary
/// without re-parsing the string. Spec 26 emits no breakpoints.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PromptSection {
    pub name: PromptSectionName,
    /// The rendered block without its heading, which the caller owns.
    pub text: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum PromptSectionName {
    RepositoryContext,
    RecentConversation,
    UserRequest,
}

impl PromptSectionName {
    pub fn heading(self) -> &'static str {
        match self {
            Self::RepositoryContext => "Repository context:",
            Self::RecentConversation => "Recent conversation:",
            Self::UserRequest => "User request:",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContextPlan {
    pub repository_id: String,
    pub task_id: String,
    pub token_estimate: usize,
    pub items: Vec<ContextItem>,
    pub files: Vec<String>,
}

impl ContextPlan {
    /// The one item header format. Both prompt builders render through it, so
    /// `--- kind: path ---` cannot drift between the chat and edit flows.
    ///
    /// The range is appended to the path (`src/lib.rs:12-40`), 1-based and
    /// inclusive, so a ranged item names exactly what it contains.
    pub fn render_item(item: &ContextItem) -> String {
        let mut output = String::new();
        output.push_str("\n--- ");
        output.push_str(item.kind.as_str());
        if let Some(path) = &item.path {
            output.push_str(": ");
            output.push_str(path);
            if let Some(range) = item.range {
                output.push_str(&format!(":{}-{}", range.start, range.end));
            }
        }
        output.push_str(" ---\n");
        output.push_str(&item.content);
        if !item.content.ends_with('\n') {
            output.push('\n');
        }
        output
    }

    /// The plan's own prompt sections, stable → volatile.
    ///
    /// Both are always present so the prompt's shape does not depend on whether
    /// an item happened to fit. The conversation block sits between them and is
    /// the caller's, because the messages do not belong to the plan.
    ///
    /// The request section is the redacted `Required` item's content: its
    /// heading already names it, so the `--- user_prompt ---` header is not
    /// repeated. That is also what keeps the request out of the repository
    /// context block, where it used to be sent a second time.
    pub fn sections(&self) -> Vec<PromptSection> {
        let mut context = String::new();
        let mut request = String::new();
        for item in &self.items {
            match item.category {
                ContextCategory::Required => {
                    if !request.is_empty() {
                        request.push('\n');
                    }
                    request.push_str(&item.content);
                }
                _ => context.push_str(&Self::render_item(item)),
            }
        }

        vec![
            PromptSection {
                name: PromptSectionName::RepositoryContext,
                text: context,
            },
            PromptSection {
                name: PromptSectionName::UserRequest,
                text: request,
            },
        ]
    }

    /// The unique included paths, in inclusion order.
    pub fn included_paths(&self) -> &[String] {
        &self.files
    }
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
        self.build_context_with_hooks(
            repository_root,
            repository_id,
            task_id,
            prompt,
            index,
            explicit_paths,
            token_budget,
            &[],
        )
    }

    /// [`Self::build_context`] with the context a `session_start` or
    /// `before_context` hook returned (spec 32 `context.md` §10). Each entry
    /// becomes an ordinary `hook_context` item through the same `add_text`
    /// path as every other item, so it is redacted and charged to the turn's
    /// budget rather than being a privileged channel.
    #[allow(clippy::too_many_arguments)]
    pub fn build_context_with_hooks(
        &self,
        repository_root: impl AsRef<Path>,
        repository_id: &str,
        task_id: &str,
        prompt: &str,
        index: Option<&RepositoryIndex>,
        explicit_paths: &[String],
        token_budget: usize,
        hook_context: &[(String, String)],
    ) -> ContextPlan {
        let mut items = Vec::new();
        let mut files = Vec::new();
        let mut token_estimate = 0;

        add_text(
            &self.scanner,
            &mut items,
            &mut token_estimate,
            token_budget,
            ContextKind::UserPrompt,
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
                ContextKind::ExplicitFile,
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
                ContextKind::AgentInstruction,
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
                ContextKind::ProjectRule,
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
                ContextKind::RepositoryMap,
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
                ContextKind::Symbols,
                None,
                &text,
            );
        }

        // A hook's additions are ordinary items: redacted by `add_text` and
        // charged to the same budget, so an over-long one is dropped rather
        // than displacing the prompt or the instructions (spec 32 §5.2,
        // `context.md` §10). Before the retrieved files, like the map and the
        // symbols, because the hook's rule is about the turn, not the search.
        for (hook_id, text) in hook_context {
            add_text(
                &self.scanner,
                &mut items,
                &mut token_estimate,
                token_budget,
                ContextKind::HookContext,
                Some(format!("hook:{hook_id}")),
                text,
            );
        }

        if let Some(results) = search_results {
            for result in results {
                self.add_file(
                    repository_root.as_ref(),
                    repository_id,
                    task_id,
                    &result.path,
                    ContextKind::RetrievedFile,
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
        kind: ContextKind,
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
    kind: ContextKind,
    path: Option<String>,
    content: &str,
) -> bool {
    if content.is_empty() {
        return false;
    }
    let redaction = scanner.redact(content);
    let tokens = redaction.text.len().div_ceil(4);
    // `Required` is never dropped for budget (proposal §5.1). It is the user's
    // own message, and the prompt is rendered only from this item (C17), so
    // refusing it would send an empty `User request:` while the raw prompt went
    // nowhere. It is still charged to the estimate, so the next item sees a
    // budget that already accounts for it. Task 4 replaces this with a visible
    // "Required item does not fit" error once categories are budgeted.
    if kind.category() != ContextCategory::Required && *token_estimate + tokens > token_budget {
        return false;
    }
    *token_estimate += tokens;
    items.push(ContextItem {
        category: kind.category(),
        kind,
        path,
        range: None,
        content: redaction.text,
        tokens,
        redaction_status: if redaction.findings.is_empty() {
            "clean".to_string()
        } else {
            "redacted".to_string()
        },
        provenance: None,
        root_path: None,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn categories_serialize_snake_case() {
        assert_eq!(
            serde_json::to_string(&ContextCategory::FileRanges).unwrap(),
            "\"file_ranges\""
        );
        assert_eq!(
            serde_json::to_string(&ContextCategory::ConversationSummary).unwrap(),
            "\"conversation_summary\""
        );
        assert_eq!(ContextCategory::HookContext.as_str(), "hook_context");
    }

    #[test]
    fn kinds_serialize_with_todays_prompt_spellings() {
        for (kind, spelling) in [
            (ContextKind::UserPrompt, "user_prompt"),
            (ContextKind::ExplicitFile, "explicit_file"),
            (ContextKind::AgentInstruction, "agent_instruction"),
            (ContextKind::ProjectRule, "project_rule"),
            (ContextKind::HookContext, "hook_context"),
            (ContextKind::RepositoryMap, "repository_map"),
            (ContextKind::Symbols, "symbols"),
            (ContextKind::RetrievedFile, "retrieved_file"),
            (ContextKind::PlanState, "plan_state"),
            (ContextKind::Pinned, "pinned"),
        ] {
            assert_eq!(kind.as_str(), spelling);
            assert_eq!(
                serde_json::to_string(&kind).unwrap(),
                format!("\"{spelling}\"")
            );
        }
    }

    #[test]
    fn items_serialize_camel_case_with_line_range() {
        let item = ContextItem {
            category: ContextCategory::FileRanges,
            kind: ContextKind::RetrievedFile,
            path: Some("src/lib.rs".to_string()),
            range: Some(LineRange { start: 12, end: 40 }),
            content: "fn main() {}".to_string(),
            tokens: 3,
            redaction_status: "clean".to_string(),
            provenance: Some(Provenance::Heuristic),
            root_path: None,
        };
        let value = serde_json::to_value(&item).unwrap();

        assert_eq!(value["category"], "file_ranges");
        assert_eq!(value["kind"], "retrieved_file");
        assert_eq!(value["redactionStatus"], "clean");
        assert_eq!(value["lineRange"]["start"], 12);
        assert_eq!(value["lineRange"]["end"], 40);
        assert_eq!(value["provenance"], "heuristic");
        assert_eq!(value["rootPath"], serde_json::Value::Null);
    }
}
