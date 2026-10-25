use crate::command_policy::CommandPolicy;
use crate::file_access::{FileAccessController, LineRange};
use crate::indexer::RepositoryIndex;
use crate::path_policy::PathPolicy;
use crate::plan::TaskPlan;
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
    ///
    /// A search hit is `SearchResults`, not `FileRanges`: §5.3 orders exact
    /// keyword hits before semantic ones and drops semantic hits first when
    /// that category is over budget. `retrieved_file` stays the header spelling
    /// either way, so the prompt is unchanged (Correction C18).
    pub fn category(self) -> ContextCategory {
        match self {
            Self::UserPrompt => ContextCategory::Required,
            Self::AgentInstruction | Self::ProjectRule => ContextCategory::Instructions,
            Self::HookContext => ContextCategory::HookContext,
            Self::Pinned => ContextCategory::Pinned,
            Self::PlanState => ContextCategory::PlanState,
            Self::RepositoryMap => ContextCategory::RepositoryMap,
            Self::Symbols => ContextCategory::Symbols,
            Self::RetrievedFile => ContextCategory::SearchResults,
            Self::ExplicitFile => ContextCategory::FileRanges,
        }
    }
}

/// Where one collection decision came from (proposal §5.3, §5.6).
///
/// The assembler matches on this with no wildcard arm, so a source that skips
/// the collection-time path-policy check does not compile, and
/// `every_collection_source_refuses_a_restricted_path` walks every variant.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CollectionSource {
    /// A path the caller passed as `explicit_paths`.
    Explicit,
    /// A path the prompt mentioned and the index resolved.
    Mention,
    /// An exact keyword-search hit.
    KeywordHit,
    /// A semantic-similarity hit.
    SemanticHit,
    /// An `AGENTS.md` in the hierarchy of a collected path.
    Instruction,
    /// A root project rule such as `README.md`.
    ProjectRule,
    /// The rendered repository map.
    Map,
    /// The rendered symbols block.
    Symbols,
    /// Context a hook returned.
    HookContext,
    /// The plan a resumed turn carries.
    PlanState,
}

/// Why an item was left out of the plan (proposal §5.5).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExclusionReason {
    RestrictedPath,
}

/// One item that did not enter the plan, with the reason. Never content.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExclusionRecord {
    pub reason: ExclusionReason,
    pub path: Option<String>,
    #[serde(rename = "lineRange")]
    pub range: Option<LineRange>,
    pub category: ContextCategory,
    pub detail: Option<String>,
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
    /// Everything collection left out, with the reason (proposal §5.6). Never
    /// content.
    pub excluded: Vec<ExclusionRecord>,
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

/// Everything one assembly needs (Correction C12).
///
/// A struct rather than eight parameters: the argument list had already reached
/// the `too_many_arguments` limit, and spec 27 adds a `pinned` field without
/// touching what either caller passes for everything else. Spec 26 populates
/// no extra source.
pub struct ContextRequest<'a> {
    pub repository_root: &'a Path,
    pub repository_id: &'a str,
    pub task_id: &'a str,
    pub prompt: &'a str,
    pub index: Option<&'a RepositoryIndex>,
    pub explicit_paths: &'a [String],
    pub limit: usize,
    pub hook_context: &'a [(String, String)],
    /// The plan a resumed turn carries, read from `SessionStore` by the caller.
    /// `None` for a new turn, an edit, and every caller that has no plan.
    pub plan: Option<&'a TaskPlan>,
}

#[derive(Debug, Clone)]
pub struct ContextManager {
    file_access: FileAccessController,
    /// The engine's own instance, so collection time applies the same rule the
    /// read path does (Correction C10). It is not a second policy.
    path_policy: PathPolicy,
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
        path_policy: PathPolicy,
        scanner: SecretScanner,
        data_dir: PathBuf,
        enable_semantic_search: bool,
        repository_map: RepositoryMapSource,
        symbols: SymbolSource,
    ) -> Self {
        Self {
            file_access,
            path_policy,
            scanner,
            data_dir,
            enable_semantic_search,
            repository_map,
            symbols,
        }
    }

    /// The collection-time half of proposal §5.6's double check.
    ///
    /// The read path checks again inside [`FileAccessController::read_file`];
    /// this is not a second rule set, it is the engine's own [`PathPolicy`]
    /// applied before a file is even opened (Correction C10). Checking twice is
    /// deliberate: a single check at the end is one new collection path away
    /// from being bypassed.
    ///
    /// Public so `every_collection_source_refuses_a_restricted_path` can walk
    /// every variant. The match has no wildcard arm: a new source has to decide
    /// whether the policy applies before it compiles.
    pub fn collection_exclusion(
        &self,
        source: CollectionSource,
        path: &str,
    ) -> Option<ExclusionReason> {
        let restricted = match source {
            CollectionSource::Explicit => self.path_policy.is_restricted(path, false),
            CollectionSource::Mention => self.path_policy.is_restricted(path, false),
            CollectionSource::KeywordHit => self.path_policy.is_restricted(path, false),
            CollectionSource::SemanticHit => self.path_policy.is_restricted(path, false),
            CollectionSource::Instruction => self.path_policy.is_restricted(path, false),
            CollectionSource::ProjectRule => self.path_policy.is_restricted(path, false),
            // Both stores already filter their own inputs, and the rendered
            // block has no path of its own. The arms still apply the policy so
            // the exhaustive walk has one answer for every source rather than
            // a special case hiding where a path is not checked.
            CollectionSource::Map => self.path_policy.is_restricted(path, false),
            CollectionSource::Symbols => self.path_policy.is_restricted(path, false),
            // Session content, not file content: `add_text` carries it and the
            // path policy has no path to judge.
            CollectionSource::HookContext => false,
            CollectionSource::PlanState => false,
        };
        restricted.then_some(ExclusionReason::RestrictedPath)
    }

    pub fn build_context(&self, request: ContextRequest<'_>) -> ContextPlan {
        let ContextRequest {
            repository_root,
            repository_id,
            task_id,
            prompt,
            index,
            explicit_paths,
            limit,
            hook_context,
            plan,
        } = request;

        let mut items = Vec::new();
        let mut files = Vec::new();
        let mut excluded = Vec::new();
        let mut token_estimate = 0;

        add_text(
            &self.scanner,
            &mut items,
            &mut token_estimate,
            limit,
            ContextKind::UserPrompt,
            None,
            None,
            prompt,
        );

        let requested_paths = explicit_paths.to_vec();
        let mentioned_paths = prompt_file_mentions(prompt, index);
        // Kept apart rather than concatenated (proposal §5.3): exact keyword
        // hits are evidence, semantic hits are suggestions, and when
        // `SearchResults` is over budget the semantic ones go first. Both use
        // the `retrieved_file` header; only the category differs.
        let keyword_hits = index
            .map(|index| index.keyword_search(prompt, 8))
            .unwrap_or_default();
        let semantic_hits = index
            .map(|index| {
                if self.enable_semantic_search {
                    VectorIndexCache::semantic_search(&self.data_dir, index, prompt, 8)
                } else {
                    index.semantic_search(prompt, 8)
                }
            })
            .unwrap_or_default();

        // Candidate paths, in the order they are considered. Instruction paths
        // are derived from every candidate, including the ones collection will
        // refuse: a restricted `secrets/AGENTS.md` beside a restricted path is
        // considered and recorded, not silently never noticed.
        let mut context_paths = requested_paths.clone();
        for path in mentioned_paths
            .iter()
            .chain(keyword_hits.iter().map(|hit| &hit.path))
            .chain(semantic_hits.iter().map(|hit| &hit.path))
        {
            if !context_paths.iter().any(|existing| existing == path) {
                context_paths.push(path.clone());
            }
        }

        for path in context_paths.iter().filter(|path| {
            requested_paths.iter().any(|requested| requested == *path)
                || mentioned_paths.iter().any(|mentioned| mentioned == *path)
        }) {
            let requested = requested_paths.iter().any(|requested| requested == path);
            let source = if requested {
                CollectionSource::Explicit
            } else {
                CollectionSource::Mention
            };
            self.add_file(
                repository_root,
                repository_id,
                task_id,
                path.as_str(),
                source,
                ContextKind::ExplicitFile,
                &mut files,
                &mut items,
                &mut excluded,
                &mut token_estimate,
                limit,
                requested,
            );
        }

        for instruction_path in agent_instruction_paths(&context_paths) {
            self.add_file(
                repository_root,
                repository_id,
                task_id,
                &instruction_path,
                CollectionSource::Instruction,
                ContextKind::AgentInstruction,
                &mut files,
                &mut items,
                &mut excluded,
                &mut token_estimate,
                limit,
                false,
            );
        }

        for rule_path in PROJECT_RULES {
            self.add_file(
                repository_root,
                repository_id,
                task_id,
                rule_path,
                CollectionSource::ProjectRule,
                ContextKind::ProjectRule,
                &mut files,
                &mut items,
                &mut excluded,
                &mut token_estimate,
                limit,
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
                limit,
                ContextKind::RepositoryMap,
                None,
                None,
                &text,
            );
        }

        // The symbols the prompt names, in the same place for the same
        // reason: it displaces retrieved files, never instructions or the map.
        if let Some((text, provenance)) = index
            .zip(map.as_ref())
            .and_then(|(index, map)| self.symbols_text(index, map, prompt))
        {
            add_text(
                &self.scanner,
                &mut items,
                &mut token_estimate,
                limit,
                ContextKind::Symbols,
                None,
                provenance,
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
                limit,
                ContextKind::HookContext,
                Some(format!("hook:{hook_id}")),
                None,
                text,
            );
        }

        // A plan a resumed turn carried (`chat.rs`'s
        // `carry_plan_from_the_previous_turn`) is ordinary, redacted, charged
        // context — not a privileged channel (proposal §5.11).
        if let Some(text) = plan.and_then(plan_state_text) {
            add_text(
                &self.scanner,
                &mut items,
                &mut token_estimate,
                limit,
                ContextKind::PlanState,
                None,
                None,
                &text,
            );
        }

        for (source, hits) in [
            (CollectionSource::KeywordHit, &keyword_hits),
            (CollectionSource::SemanticHit, &semantic_hits),
        ] {
            for hit in hits {
                self.add_file(
                    repository_root,
                    repository_id,
                    task_id,
                    &hit.path,
                    source,
                    ContextKind::RetrievedFile,
                    &mut files,
                    &mut items,
                    &mut excluded,
                    &mut token_estimate,
                    limit,
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
            excluded,
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

    /// The symbols the prompt's identifiers name and the provenance of the
    /// weakest one shown, or `None` when the ceiling is `0`, nothing matches,
    /// or the symbol index cannot be loaded or stored, which leaves the item
    /// out rather than failing the turn. It reads the stored index as it is,
    /// whatever an eager definition pass has merged so far, and never waits on
    /// a language server (`context.md` §8).
    ///
    /// The block's provenance is the **lowest** among the lines it actually
    /// contains (proposal §5.3): one heuristic symbol makes the whole block a
    /// guess, however many exact ones sit beside it.
    fn symbols_text(
        &self,
        index: &RepositoryIndex,
        map: &RepositoryMap,
        prompt: &str,
    ) -> Option<(String, Option<Provenance>)> {
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
        if rendered.text.is_empty() {
            return None;
        }
        // `Provenance::Heuristic` sorts after `LanguageServer`, so the maximum
        // is the least authoritative entry present in the rendered block.
        let provenance = matches
            .iter()
            .take(rendered.shown)
            .map(|matched| matched.symbol.provenance)
            .max();
        Some((rendered.text, provenance))
    }

    #[allow(clippy::too_many_arguments)]
    fn add_file(
        &self,
        repository_root: &Path,
        repository_id: &str,
        task_id: &str,
        path: &str,
        source: CollectionSource,
        kind: ContextKind,
        files: &mut Vec<String>,
        items: &mut Vec<ContextItem>,
        excluded: &mut Vec<ExclusionRecord>,
        token_estimate: &mut usize,
        token_budget: usize,
        allow_outside_root: bool,
    ) {
        if files.iter().any(|existing| existing == path) {
            return;
        }
        // The collection-time check, before the file is opened (proposal §5.6).
        // `read_file` checks again; this one is what keeps a restricted path
        // from being read at all, and what puts it in the record as considered
        // and refused rather than never noticed.
        if let Some(reason) = self.collection_exclusion(source, path) {
            push_exclusion(
                excluded,
                ExclusionRecord {
                    reason,
                    path: Some(path.to_string()),
                    range: None,
                    category: kind.category(),
                    detail: None,
                },
            );
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
            None,
            &file.content,
        );
        if added {
            files.push(file.path);
        }
    }
}

/// Record an exclusion once per `(reason, path)`. The same restricted path can
/// be reached by more than one source — a prompt mention is also a keyword hit
/// — and the manifest wants the fact, not a copy per route.
fn push_exclusion(excluded: &mut Vec<ExclusionRecord>, record: ExclusionRecord) {
    let duplicate = excluded
        .iter()
        .any(|existing| existing.reason == record.reason && existing.path == record.path);
    if !duplicate {
        excluded.push(record);
    }
}

/// A carried plan as the `plan_state` item's text (proposal §5.11).
///
/// Only the steps: evidence is the panel's record of what happened, not a
/// prompt input. `None` for a plan with no steps, which is a plan with nothing
/// to carry.
fn plan_state_text(plan: &TaskPlan) -> Option<String> {
    if plan.steps.is_empty() {
        return None;
    }
    let mut text = String::from("The plan this task is carrying forward:\n");
    for (index, step) in plan.steps.iter().enumerate() {
        let status = match step.status {
            crate::plan::StepStatus::Pending => "pending",
            crate::plan::StepStatus::InProgress => "in progress",
            crate::plan::StepStatus::Completed => "completed",
            crate::plan::StepStatus::Blocked => "blocked",
            crate::plan::StepStatus::Skipped => "skipped",
        };
        text.push_str(&format!("{}. [{status}] {}", index + 1, step.title));
        if let Some(detail) = &step.detail {
            text.push_str(&format!(" — {detail}"));
        }
        text.push('\n');
    }
    Some(text)
}

// The accumulators are borrowed plan state and the rest are the item's own
// fields; a params struct would be built at each call site only to be
// destructured here. Task 4 folds the budget accumulators into one type.
#[allow(clippy::too_many_arguments)]
fn add_text(
    scanner: &SecretScanner,
    items: &mut Vec<ContextItem>,
    token_estimate: &mut usize,
    token_budget: usize,
    kind: ContextKind,
    path: Option<String>,
    provenance: Option<Provenance>,
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
        provenance,
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
