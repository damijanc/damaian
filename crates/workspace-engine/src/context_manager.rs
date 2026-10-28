use crate::command_policy::CommandPolicy;
use crate::compaction::ConversationSummary;
use crate::error::{ClientError, Result};
use crate::external_reference::{ExternalReference, utc_date};
use crate::file_access::{FileAccessController, LineRange};
use crate::hash::now_millis;
use crate::indexer::RepositoryIndex;
use crate::memory::{MemoryMatchReason, MemoryProvenance, RecalledMemory, Verification};
use crate::path_policy::PathPolicy;
use crate::plan::TaskPlan;
use crate::repository_map::{RepositoryMap, RepositoryMapStore};
use crate::secret_scanner::SecretScanner;
use crate::session::ContextPin;
use crate::symbol_index::{Provenance, SymbolIndex, SymbolIndexStore, SymbolMatch};
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
    /// A page fetched by spec 51's `fetch_url`. It precedes every
    /// repository-content category in drop order; spec 30's `Memory` is the one
    /// category declared after it (spec 51 D6).
    ExternalReference,
    /// Remembered statements the user confirmed (spec 30 §5.1). Last, so a
    /// crowded context drops memory before anything else: losing a remembered
    /// fact costs a little convenience, while losing code costs the answer.
    Memory,
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
            Self::ExternalReference => "external_reference",
            Self::Memory => "memory",
        }
    }

    /// The budgeted categories, in pipeline order after `Required`, which has no
    /// share (proposal §5.2). Declaration order of the enum is the same order.
    pub const BUDGETED: [ContextCategory; 12] = [
        ContextCategory::Instructions,
        ContextCategory::HookContext,
        ContextCategory::Pinned,
        ContextCategory::PlanState,
        ContextCategory::RepositoryMap,
        ContextCategory::Symbols,
        ContextCategory::SearchResults,
        ContextCategory::FileRanges,
        ContextCategory::Findings,
        ContextCategory::ConversationSummary,
        ContextCategory::ExternalReference,
        ContextCategory::Memory,
    ];

    /// The index into [`ContextBudget::shares`]; `None` for `Required`.
    pub fn budget_index(self) -> Option<usize> {
        match self {
            Self::Required => None,
            Self::Instructions => Some(0),
            Self::HookContext => Some(1),
            Self::Pinned => Some(2),
            Self::PlanState => Some(3),
            Self::RepositoryMap => Some(4),
            Self::Symbols => Some(5),
            Self::SearchResults => Some(6),
            Self::FileRanges => Some(7),
            Self::Findings => Some(8),
            Self::ConversationSummary => Some(9),
            Self::ExternalReference => Some(10),
            Self::Memory => Some(11),
        }
    }
}

/// The per-category shares and the observed-window margin (proposal §5.2, §5.7).
///
/// Fractions are integer basis points (`u16`, `0..=10_000`): a float would make
/// the last token's allocation depend on rounding and would cost
/// [`ContextItem`]/[`ContextPlan`] their `Eq`, which tests rely on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContextBudget {
    /// One share per [`ContextCategory::BUDGETED`] category, in that order.
    /// The sum must be at most 10,000; a set that oversubscribes is a config
    /// error rather than a silent scale-down.
    pub shares: [u16; 12],
    /// The share of an observed window reserved for estimate error. It is not
    /// applied to the declared `context_token_budget` (proposal §5.7).
    pub safety_margin_bps: u16,
}

impl Default for ContextBudget {
    fn default() -> Self {
        Self {
            shares: [
                1_000, // instructions
                500,   // hook_context
                1_500, // pinned
                500,   // plan_state
                500,   // repository_map
                1_000, // symbols
                1_000, // search_results
                2_500, // file_ranges
                200,   // findings — pays for memory; it has no producer yet
                500,   // conversation_summary
                500,   // external_reference
                300,   // memory
            ],
            safety_margin_bps: 1_000,
        }
    }
}

impl ContextBudget {
    pub fn share(&self, category: ContextCategory) -> u16 {
        category
            .budget_index()
            .map(|index| self.shares[index])
            .unwrap_or(0)
    }

    pub fn share_mut(&mut self, category: ContextCategory) -> Option<&mut u16> {
        category.budget_index().map(|index| &mut self.shares[index])
    }

    pub fn set_share(&mut self, category: ContextCategory, value: u16) {
        if let Some(share) = self.share_mut(category) {
            *share = value;
        }
    }

    pub fn total_share_bps(&self) -> u32 {
        self.shares.iter().map(|share| u32::from(*share)).sum()
    }

    /// A set that allocates more than the whole window is a config error naming
    /// the key family, never a silent scale-down (proposal §5.2).
    pub fn validate(&self) -> Result<()> {
        let total = self.total_share_bps();
        if total > 10_000 {
            return Err(ClientError::InvalidInput(format!(
                "context_budget.* shares total {total} basis points; they must sum to at most 10000 (1.00)"
            )));
        }
        Ok(())
    }
}

/// Where the effective limit came from (proposal §5.7).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LimitSource {
    /// A capability profile's provider window, with the safety margin applied.
    Observed,
    /// `context_token_budget`, used unchanged, with no margin.
    Declared,
}

/// The token limit assembly actually divides (proposal §5.7).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EffectiveLimit {
    pub tokens: usize,
    pub source: LimitSource,
    pub safety_margin_bps: u16,
    /// The provider window the margin was applied to, when one is known.
    pub window: Option<usize>,
}

impl EffectiveLimit {
    /// The config budget used as-is. No margin: `context_token_budget` is a
    /// packing budget, not the provider's window (Correction C6), so shrinking
    /// it by 10% would make every existing user's context smaller for no safety
    /// gain.
    pub fn declared(tokens: usize) -> Self {
        Self {
            tokens,
            source: LimitSource::Declared,
            safety_margin_bps: 0,
            window: None,
        }
    }

    /// An observed provider window with the configured margin reserved.
    pub fn observed(window: usize, safety_margin_bps: u16) -> Self {
        let reserved = (window as u128 * u128::from(safety_margin_bps) / 10_000) as usize;
        Self {
            tokens: window.saturating_sub(reserved),
            source: LimitSource::Observed,
            safety_margin_bps,
            window: Some(window),
        }
    }
}

/// What one category was allowed and what it used (proposal §5.5).
///
/// `estimated` is always true for assembly and exists so a figure can never be
/// read as measured (spec 19 requirement 4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CategoryUsage {
    pub category: ContextCategory,
    /// After redistribution from earlier categories.
    pub allocated: usize,
    pub used: usize,
    pub items: usize,
    /// The category ran out of its share and dropped or shortened an item.
    pub truncated: bool,
    pub estimated: bool,
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
    /// Text of a page `fetch_url` retrieved (spec 51 §5.4).
    ExternalReference,
    /// A remembered statement (spec 30 §5.2).
    Memory,
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
            Self::ExternalReference => "external_reference",
            Self::Memory => "memory",
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
            Self::ExternalReference => ContextCategory::ExternalReference,
            Self::Memory => ContextCategory::Memory,
        }
    }
}

/// The bytes-per-token ratio an item is measured with (proposal §5.8).
///
/// **Measured** (Task 7, 2026-10-25) with
/// `calibration_measures_bytes_per_token` against `deepseek` /
/// `deepseek-v4-flash`, one call per document:
///
/// | type | bytes / token | measured | note |
/// |---|---|---|---|
/// | prose and Markdown | 4 | 4.50 | keeps `len / 4`: the measurement says that ratio over-estimates prose by about 13%, consistent with spec 19 §7's 2.5–6.3% high. Classification therefore never estimates prose below `len / 4`, and the headroom stands in for the margin the declared path does not subtract |
/// | source code | 3.6 (18/5) | 3.61 | short identifiers and punctuation-heavy lines tokenize denser than prose |
/// | structured data | 2.4 (12/5) | 2.45 | quoted keys, braces, commas and short scalar values put a token boundary at nearly every mark |
///
/// The measured figures include about 15 tokens of instruction and chat-template
/// overhead in the provider's input count, so a document-only ratio would be a
/// little higher and these constants over-count tokens slightly — the safe
/// direction. One sample per type from one provider is the evidence this ratio
/// has; it replaces the published rules of thumb Task 6 landed, and the prose
/// ratio is the one figure deliberately left alone.
///
/// Integer arithmetic decides the estimate: no float participates in inclusion,
/// so `prompt_cache`'s determinism guard still holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenEstimate {
    Prose,
    Code,
    Structured,
}

impl TokenEstimate {
    /// The ratio for one item, from its kind and its file extension.
    ///
    /// This is a content *type* choice, not a scan of the text: it costs one
    /// extension lookup and cannot make the same content estimate differently
    /// between two assemblies.
    pub fn for_item(kind: ContextKind, path: Option<&str>) -> Self {
        if let Some(estimate) = path.and_then(estimate_for_extension) {
            return estimate;
        }
        match kind {
            // A file the user or the search named, with no extension in the
            // table below (a `Makefile`, a generated script with no suffix), is
            // measured as code: over-estimating is the safe direction, and
            // under-estimating is what would let an item overflow its share.
            ContextKind::ExplicitFile | ContextKind::RetrievedFile => Self::Code,
            // Everything else keeps today's ratio. That includes the path-less
            // rendered blocks (the map and the symbols list): their own
            // renderer fits them to a ceiling counted with `len / 4`
            // (`render_for_model`), so re-measuring the same text with a denser
            // ratio would report more tokens than the ceiling it was rendered
            // to. Those ceilings move only when their renderers share this
            // estimator, which is outside "change only inside `add_text`".
            _ => Self::Prose,
        }
    }

    /// Bytes per token as a fraction, so tokens are `ceil(len * denominator /
    /// numerator)` in integers.
    pub const fn bytes_per_token(self) -> (u32, u32) {
        match self {
            Self::Prose => (4, 1),
            Self::Code => (18, 5),
            Self::Structured => (12, 5),
        }
    }

    /// An estimate for already-redacted text. Zero only for empty text, which
    /// `add_text` refuses before it gets here.
    pub fn tokens(self, text: &str) -> usize {
        let (numerator, denominator) = self.bytes_per_token();
        (text.len() as u128 * u128::from(denominator)).div_ceil(u128::from(numerator)) as usize
    }
}

/// The extensions the estimate classifies. Indexing's `detect_language` is a
/// different map with a different job (it drives symbol extraction) and is left
/// alone; an extension missing here falls back to the kind's default.
fn estimate_for_extension(path: &str) -> Option<TokenEstimate> {
    let extension = Path::new(path)
        .extension()
        .and_then(|extension| extension.to_str())?
        .to_ascii_lowercase();
    Some(match extension.as_str() {
        "md" | "markdown" | "mdx" | "txt" | "rst" | "adoc" => TokenEstimate::Prose,
        "json" | "jsonc" | "jsonl" | "yaml" | "yml" | "toml" | "xml" | "csv" | "tsv" | "html"
        | "htm" | "css" | "scss" | "lock" | "ipynb" => TokenEstimate::Structured,
        "rs" | "go" | "py" | "js" | "jsx" | "mjs" | "cjs" | "ts" | "tsx" | "java" | "kt"
        | "kts" | "c" | "h" | "cc" | "cpp" | "cxx" | "hpp" | "cs" | "rb" | "php" | "swift"
        | "m" | "mm" | "scala" | "sh" | "bash" | "zsh" | "fish" | "sql" | "lua" | "ex" | "exs"
        | "erl" | "hs" | "ml" | "clj" | "zig" | "vue" | "svelte" | "proto" => TokenEstimate::Code,
        _ => return None,
    })
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
    /// A file or range the user pinned for the session (spec 27 §5.4).
    Pinned,
}

/// Why an item was left out of the plan (proposal §5.5).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExclusionReason {
    RestrictedPath,
    /// A range read as part of another range, or absorbed by a whole-file item
    /// for the same path (proposal §5.4). The record names the range that did
    /// not become its own item.
    DeduplicatedIntoRange,
    /// The item did not fit its category's share (proposal §5.2).
    CategoryBudgetExhausted,
    /// The item is included, but only in part: a block re-rendered at a smaller
    /// ceiling for its category share (proposal §5.2, Correction C14).
    Truncated,
    /// A whole-file read the read path refuses because the file exceeds
    /// `max_file_bytes` (proposal §5.5).
    TooLarge,
    /// A read the read path refuses because the file looks binary.
    Binary,
    /// The user removed an optional item in a paused pre-send review
    /// (spec 27 §5.4). `Required` can never carry this reason.
    UserRemoved,
    /// The session's path restriction admits only selected paths, and this
    /// one is not among them (spec 27 §5.4).
    PathNotSelected,
    /// A pinned file no longer reads: it was deleted, or became unreadable.
    /// Recorded instead of dropping the pin silently (spec 27 §5.4).
    Missing,
    /// A memory recalled at assembly was deleted, expired, superseded, or had
    /// its evidence withdrawn before the request was sent (spec 30 §5.6). The
    /// item and its `memory` manifest row are both dropped.
    MemoryNoLongerActive,
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

/// One included item, named by where it came from rather than what it says.
///
/// The manifest's projection of a [`ContextItem`]: category, kind, path, range,
/// tokens and the provenance-ish fields (root, symbol provenance, origin).
/// Never `content` or `redaction_status` — the manifest is safe to audit and to
/// show.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IncludedRef {
    pub category: ContextCategory,
    pub kind: ContextKind,
    pub path: Option<String>,
    #[serde(rename = "lineRange")]
    pub range: Option<LineRange>,
    pub tokens: usize,
    pub root_path: Option<String>,
    pub provenance: Option<Provenance>,
    #[serde(default)]
    pub origin: Option<String>,
}

impl IncludedRef {
    /// The only place an item is projected into the manifest.
    pub fn from_item(item: &ContextItem) -> Self {
        Self {
            category: item.category,
            kind: item.kind,
            path: item.path.clone(),
            range: item.range,
            tokens: item.tokens,
            root_path: item.root_path.clone(),
            provenance: item.provenance,
            origin: item.origin.clone(),
        }
    }
}

/// The conversation summary a turn is sending (spec 27 §5.2).
///
/// #55 sends the summary inside the conversation block rather than as a
/// `ContextItem`, so it has no `IncludedRef`. This row is what makes its cost
/// visible in the inspector and what lets a paused review remove it for a turn.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConversationSummaryUsage {
    pub tokens: usize,
    /// Messages the summary covers, as #55 recorded them.
    pub messages: usize,
    pub through_seq: u64,
    /// True when the model call failed and only the facts half was kept.
    pub facts_only: bool,
    /// True when the newest summarisation call failed and the narrative lists
    /// were carried from the previous summary. `facts_only` is then false, so
    /// this is how the panel can still say the call failed.
    #[serde(default)]
    pub narrative_carried_forward: bool,
}

/// How many of the symbols actually shown came from each provenance
/// (spec 27 §5.2, spec 25).
///
/// The symbols block is one `ContextItem`, so its `IncludedRef.provenance` can
/// only carry the weakest line. "12 symbols (4 heuristic)" needs the counts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SymbolProvenanceCounts {
    pub total: usize,
    pub language_server: usize,
    pub heuristic: usize,
}

/// One recalled memory as the manifest records it (spec 30 §5.4, C3, C10).
///
/// The id resolves to the statement in the store at display time. The manifest
/// is written to the audit log, so a statement here would copy personal and
/// environmental text into an append-only record — the exposure spec 28 §5.8
/// refuses. The match reason comes from the prompt rather than the entry, and
/// the verification is the state recall left the entry in.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoryRecallRef {
    pub entry_id: String,
    pub match_reason: MemoryMatchReason,
    pub verification: Verification,
}

impl MemoryRecallRef {
    pub fn from_recalled(recalled: &RecalledMemory) -> Self {
        Self {
            entry_id: recalled.entry.id.clone(),
            match_reason: recalled.match_reason.clone(),
            verification: recalled.verification,
        }
    }
}

/// What one assembly included and left out, by path, range, count and reason
/// (proposal §5.5).
///
/// Holds no item content: that is what makes it safe to write to the audit log
/// and to show in a diagnostics view. `assembled_at_ms` is the one
/// time-dependent field and never reaches the prompt; the prompt's determinism
/// guard is `tests/prompt_cache.rs`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContextManifest {
    pub repository_id: String,
    pub task_id: String,
    pub assembled_at_ms: u128,
    /// The limit categories divided and where it came from (requirement 7).
    pub limit: EffectiveLimit,
    pub total_tokens: usize,
    /// Per category allocation and use, in pipeline order.
    pub categories: Vec<CategoryUsage>,
    /// Every included item, in inclusion order.
    pub included: Vec<IncludedRef>,
    /// Everything left out, or included only in part, with a reason.
    pub excluded: Vec<ExclusionRecord>,
    /// The conversation summary this turn sends, when it has one (spec 27
    /// §5.2). `#[serde(default)]` keeps a `context_assembled` event written
    /// before this field readable.
    #[serde(default)]
    pub conversation_summary: Option<ConversationSummaryUsage>,
    /// Provenance counts for the symbols block, when one was included.
    #[serde(default)]
    pub symbol_provenance: Option<SymbolProvenanceCounts>,
    /// One row per included `Memory` item, in item order (spec 30 C3, C10,
    /// C14). Never the statement. `#[serde(default)]` keeps a
    /// `context_assembled` event written before this field readable.
    #[serde(default)]
    pub memory: Vec<MemoryRecallRef>,
}

impl ContextManifest {
    /// The `context_assembled` audit fields, one shape for both callers.
    ///
    /// The manifest is a single JSON field beside the identifiers and the
    /// token estimate (proposal §5.5). Serialisation cannot fail: every field is
    /// a plain value or an enum of plain values.
    pub fn audit_fields(&self) -> Vec<(&'static str, String)> {
        vec![
            ("repositoryId", self.repository_id.clone()),
            ("taskId", self.task_id.clone()),
            ("tokenEstimate", self.total_tokens.to_string()),
            ("manifest", serde_json::to_string(self).unwrap_or_default()),
        ]
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
    /// The provenance of content with no file identity (spec 51 D7): the
    /// canonical URL and its fetch time. `None` for every file-bearing item and
    /// every block. `#[serde(default)]` keeps a manifest written before this
    /// field readable.
    #[serde(default)]
    pub origin: Option<String>,
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContextPlan {
    pub repository_id: String,
    pub task_id: String,
    pub token_estimate: usize,
    pub items: Vec<ContextItem>,
    pub files: Vec<String>,
    /// The record of this assembly: what was included and what was left out,
    /// with the limit and per-category usage (proposal §5.5). Recorded by the
    /// caller as one `context_assembled` audit event.
    pub manifest: ContextManifest,
}

/// One assembly, inspectable and not yet sent (spec 27 §5.1).
///
/// A view of a [`ContextPlan`] rather than a second pipeline: `files` and
/// `sections` are derived from `items`, and `into_plan` reconstructs the plan
/// exactly. The paused pre-send review persists this; the shell is never sent
/// `items`, only `manifest`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PreparedContext {
    pub manifest: ContextManifest,
    pub items: Vec<ContextItem>,
}

impl PreparedContext {
    pub fn from_plan(plan: &ContextPlan) -> Self {
        Self {
            manifest: plan.manifest.clone(),
            items: plan.items.clone(),
        }
    }

    /// The plan shape the prompt renderer and the resumed turn need.
    pub fn into_plan(self) -> ContextPlan {
        let token_estimate = self.items.iter().map(|item| item.tokens).sum();
        let files = included_files(&self.items);
        ContextPlan {
            repository_id: self.manifest.repository_id.clone(),
            task_id: self.manifest.task_id.clone(),
            token_estimate,
            items: self.items,
            files,
            manifest: self.manifest,
        }
    }

    /// The unique included paths, in inclusion order.
    pub fn files(&self) -> Vec<String> {
        included_files(&self.items)
    }

    pub fn sections(&self) -> Vec<PromptSection> {
        prompt_sections(&self.items)
    }

    /// Removes one optional item (spec 27 §5.4, §5.5).
    ///
    /// `Required` is refused before anything changes: a UI bug or a future
    /// caller cannot remove the user's own message from a request. The
    /// exclusion is recorded, the category's use is recomputed from what is
    /// left, and `total_tokens` follows. `allocated` is left alone: it is what
    /// the category was allowed, not what it used.
    pub fn remove_optional(&mut self, index: usize) -> Result<ExclusionRecord> {
        let Some(item) = self.items.get(index) else {
            return Err(ClientError::InvalidInput(format!(
                "No context item at index {index}"
            )));
        };
        if item.category == ContextCategory::Required {
            return Err(ClientError::InvalidInput(
                "Required context cannot be removed".to_string(),
            ));
        }
        Ok(self.remove_item(
            index,
            ExclusionReason::UserRemoved,
            "removed for this turn by the user".to_string(),
        ))
    }

    /// Drops a memory item whose entry may no longer be sent (spec 30 §5.6).
    ///
    /// The send-time re-check names the entry id, not an item index, because
    /// the turn may have paused and resumed since assembly. Its `memory`
    /// manifest row goes with the item (C14), so the inspector cannot name a
    /// statement the request no longer carries. An entry already removed from
    /// the review, or one the budget never included, has no row and yields
    /// `None`.
    pub fn remove_memory(
        &mut self,
        entry_id: &str,
        detail: String,
    ) -> Result<Option<ExclusionRecord>> {
        let Some(position) = self
            .manifest
            .memory
            .iter()
            .position(|reference| reference.entry_id == entry_id)
        else {
            return Ok(None);
        };
        let index = self
            .items
            .iter()
            .enumerate()
            .filter(|(_, item)| item.category == ContextCategory::Memory)
            .nth(position)
            .map(|(index, _)| index)
            .ok_or_else(|| {
                ClientError::InvalidInput(format!(
                    "the manifest names memory entry {entry_id} but the plan has no such item"
                ))
            })?;
        Ok(Some(self.remove_item(
            index,
            ExclusionReason::MemoryNoLongerActive,
            detail,
        )))
    }

    /// The one removal path. Callers decide the reason and the detail; the
    /// manifest bookkeeping is identical for a user removal and a send-time
    /// rejection.
    fn remove_item(
        &mut self,
        index: usize,
        reason: ExclusionReason,
        detail: String,
    ) -> ExclusionRecord {
        let item = &self.items[index];
        let category = item.category;
        let record = ExclusionRecord {
            reason,
            path: item.path.clone(),
            range: item.range,
            category,
            detail: Some(detail),
        };
        // A memory ref exists only for an included `Memory` item and the refs
        // are 1:1 with those items in item order (C14), so the ref for this
        // index is the count of `Memory` items before it. Leaving it behind
        // would show a statement the request no longer carries.
        if category == ContextCategory::Memory {
            let position = self.items[..index]
                .iter()
                .filter(|item| item.category == ContextCategory::Memory)
                .count();
            if position < self.manifest.memory.len() {
                self.manifest.memory.remove(position);
            }
        }
        self.items.remove(index);
        push_exclusion(&mut self.manifest.excluded, record.clone());
        if let Some(usage) = self
            .manifest
            .categories
            .iter_mut()
            .find(|usage| usage.category == category)
        {
            usage.used = self
                .items
                .iter()
                .filter(|item| item.category == category)
                .map(|item| item.tokens)
                .sum();
            usage.items = self
                .items
                .iter()
                .filter(|item| item.category == category)
                .count();
        }
        self.manifest.included = self.items.iter().map(IncludedRef::from_item).collect();
        self.manifest.total_tokens = self.items.iter().map(|item| item.tokens).sum();
        record
    }

    /// Removes the #55 conversation summary for this turn.
    ///
    /// The summary is not an item, so there is no index to remove; the row and
    /// the exclusion are enough. The resumed request passes no summary and
    /// states what was removed (spec 27 §5.4).
    pub fn remove_summary(&mut self) -> Result<ExclusionRecord> {
        let Some(usage) = self.manifest.conversation_summary.take() else {
            return Err(ClientError::InvalidInput(
                "This context has no conversation summary to remove".to_string(),
            ));
        };
        let record = ExclusionRecord {
            reason: ExclusionReason::UserRemoved,
            path: None,
            range: None,
            category: ContextCategory::ConversationSummary,
            detail: Some(format!(
                "conversation summary covering {} message{} removed for this turn by the user",
                usage.messages,
                if usage.messages == 1 { "" } else { "s" }
            )),
        };
        push_exclusion(&mut self.manifest.excluded, record.clone());
        Ok(record)
    }
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
        } else if let Some(origin) = &item.origin {
            // Content with no file identity names where it came from instead
            // (spec 51 D7): a fetched page shows its URL and fetch time.
            output.push_str(": ");
            output.push_str(origin);
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
        prompt_sections(&self.items)
    }

    /// The unique included paths, in inclusion order.
    pub fn included_paths(&self) -> &[String] {
        &self.files
    }
}

/// The two prompt blocks a plan contributes, from its items alone.
///
/// Shared with [`PreparedContext`] so a paused review and an ordinary turn
/// cannot render different text for the same items.
fn prompt_sections(items: &[ContextItem]) -> Vec<PromptSection> {
    let mut context = String::new();
    let mut request = String::new();
    for item in items {
        match item.category {
            ContextCategory::Required => {
                if !request.is_empty() {
                    request.push('\n');
                }
                request.push_str(&item.content);
            }
            _ => context.push_str(&ContextPlan::render_item(item)),
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
fn included_files(items: &[ContextItem]) -> Vec<String> {
    let mut files = Vec::new();
    for item in items {
        if let Some(path) = &item.path
            && !files.iter().any(|existing| existing == path)
        {
            files.push(path.clone());
        }
    }
    files
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
    /// The session's pinned files and ranges (spec 27 §5.4), read from
    /// `SessionStore` by the caller. Each becomes a `Pinned` item.
    pub pinned: &'a [ContextPin],
    /// The session's path restriction (spec 27 §5.4), already read from
    /// `SessionStore`. `None` means no restriction; `Some([])` admits no
    /// file content at all. Instructions are not filtered.
    pub restriction: Option<&'a [String]>,
    /// The #55 summary this turn sends, for the manifest's cost row. The
    /// summary text itself stays in the conversation block, not in the plan.
    pub conversation_summary: Option<&'a ConversationSummary>,
    /// The session's recorded fetched pages (spec 51 §5.4, D5), read from
    /// `SessionStore` by the caller. Each becomes an `ExternalReference` item
    /// with the text already converted by the fetch task.
    pub external_references: &'a [ExternalReference],
    /// The entries recall selected for this turn (spec 30 §5.1), read from
    /// `MemoryStore` by the caller. Each becomes its own `Memory` item, so one
    /// entry can be removed without removing the others (C4). Empty for every
    /// caller with no turn to recall for (C5).
    pub memory: &'a [RecalledMemory],
}

/// A gap of at most this many lines still merges two ranges (proposal §5.4).
///
/// Two hits in one function can be separated by a blank line or a closing
/// brace, and reading them as one range is cheaper than two headers for the
/// same neighbourhood. Eight lines apart is a different region; §7 records the
/// evidence behind the value.
const RANGE_GUTTER: usize = 2;

/// One file a collection source asked for, before dedup and merge.
///
/// The assembler's own type rather than `SearchResult`: a source that can name
/// a line range passes it here, and every source that cannot passes `None` for
/// the whole file.
#[derive(Debug, Clone, PartialEq, Eq)]
struct FileCandidate {
    path: String,
    /// `None` is the whole file. A pin may name a range (spec 27 §5.4); every
    /// automatic source is still whole-file, so automatic collection exercises
    /// the merge only through its unit tests.
    range: Option<LineRange>,
    kind: ContextKind,
    provenance: Option<Provenance>,
    /// The project root that owns `path` (spec 24), resolved once when the
    /// candidate is collected.
    root_path: Option<String>,
    allow_outside_root: bool,
}

/// One read the assembler will make: a merged range, or a whole file when a
/// candidate asked for one (or a whole-file item already absorbed the ranges).
#[derive(Debug, Clone, PartialEq, Eq)]
struct MergedFile {
    path: String,
    range: Option<LineRange>,
    kind: ContextKind,
    provenance: Option<Provenance>,
    root_path: Option<String>,
    allow_outside_root: bool,
}

/// One collected thing, before the category budget decides whether it is
/// included (proposal §5.2).
///
/// Text is already redacted and measured by [`add_text`]. A block is kept as
/// data rather than text because a category share smaller than its configured
/// ceiling must re-render it through its own renderer (Correction C14), never
/// slice what redaction produced.
#[derive(Debug)]
enum PendingItem {
    Item(ContextItem),
    Block(BlockItem),
}

impl PendingItem {
    fn category(&self) -> ContextCategory {
        match self {
            Self::Item(item) => item.category,
            Self::Block(block) => block.kind.category(),
        }
    }

    fn tokens(&self) -> usize {
        match self {
            Self::Item(item) => item.tokens,
            // A block is rendered by the budget pass, which measures it then.
            Self::Block(_) => 0,
        }
    }
}

#[derive(Debug)]
struct BlockItem {
    kind: ContextKind,
    /// The ceiling the renderer was configured with (`repository_map_max_tokens`
    /// or `symbol_context_max_tokens`).
    max_tokens: usize,
    content: BlockContent,
}

/// One block after its own renderer has fitted it to a ceiling.
struct RenderedBlock {
    text: String,
    provenance: Option<Provenance>,
    /// Provenance counts for the symbols block, which has no other carrier.
    symbols: Option<SymbolProvenanceCounts>,
}

#[derive(Debug)]
enum BlockContent {
    Map(RepositoryMap),
    Symbols {
        index: SymbolIndex,
        matches: Vec<SymbolMatch>,
    },
}

/// Group by path, absorb ranges under a whole-file item, and merge overlapping
/// or near ranges with [`RANGE_GUTTER`].
///
/// Paths keep their first-seen order and ranges are sorted by start, so the
/// output does not depend on hash iteration or input order beyond the first
/// appearance of a path. `whole_files` is every path already included whole by
/// an earlier collection stage; those absorb their ranges, because the whole
/// file already contains them. Every range that did not become its own item is
/// recorded as [`ExclusionReason::DeduplicatedIntoRange`].
fn merge_file_ranges(
    candidates: &[FileCandidate],
    whole_files: &[String],
) -> (Vec<MergedFile>, Vec<ExclusionRecord>) {
    let mut groups: Vec<(&str, Vec<&FileCandidate>)> = Vec::new();
    for candidate in candidates {
        if let Some((_, group)) = groups
            .iter_mut()
            .find(|(path, _)| *path == candidate.path.as_str())
        {
            group.push(candidate);
        } else {
            groups.push((candidate.path.as_str(), vec![candidate]));
        }
    }

    let mut merged = Vec::new();
    let mut excluded = Vec::new();

    for (path, group) in groups {
        let included_whole = whole_files.iter().any(|included| included == path);
        let whole = group
            .iter()
            .copied()
            .find(|candidate| candidate.range.is_none());

        // Whole-file absorption: a range within an included whole file is
        // redundant. This is the cross-source case (§5.4): a prompt mention
        // collects the whole file and a search hit collects a range.
        if included_whole || whole.is_some() {
            for candidate in &group {
                if let Some(range) = candidate.range {
                    excluded.push(range_exclusion(
                        path,
                        range,
                        candidate.kind,
                        "absorbed into the whole-file item",
                    ));
                }
            }
            if let Some(whole) = whole {
                merged.push(MergedFile {
                    path: path.to_string(),
                    range: None,
                    kind: whole.kind,
                    provenance: best_provenance(&group),
                    root_path: whole.root_path.clone(),
                    allow_outside_root: whole.allow_outside_root,
                });
            }
            continue;
        }

        let mut ranges = group;
        // A total order, not just by start: two ranges with the same start are
        // still deterministic, so a prompt cannot depend on candidate order.
        ranges.sort_by_key(|candidate| {
            candidate
                .range
                .map(|range| (range.start, range.end))
                .unwrap_or((usize::MAX, usize::MAX))
        });

        let mut index = 0;
        while index < ranges.len() {
            let Some(first_range) = ranges[index].range else {
                index += 1;
                continue;
            };
            let start = first_range.start;
            let mut end = first_range.end;
            let mut members = vec![ranges[index]];
            let mut next = index + 1;
            while next < ranges.len() {
                let Some(following) = ranges[next].range else {
                    break;
                };
                // `next.start <= current.end + 1 + GUTTER` (proposal §5.4).
                if following.start > end.saturating_add(1).saturating_add(RANGE_GUTTER) {
                    break;
                }
                end = end.max(following.end);
                members.push(ranges[next]);
                next += 1;
            }

            let merged_range = LineRange { start, end };
            let first = members[0];
            merged.push(MergedFile {
                path: path.to_string(),
                range: Some(merged_range),
                kind: first.kind,
                provenance: best_provenance(&members),
                root_path: first.root_path.clone(),
                allow_outside_root: first.allow_outside_root,
            });

            // One member that is exactly the merged range is the item itself;
            // every other member was read as part of it and is recorded.
            let mut kept = false;
            for member in members {
                let Some(member_range) = member.range else {
                    continue;
                };
                if !kept && member_range == merged_range {
                    kept = true;
                    continue;
                }
                excluded.push(range_exclusion(
                    path,
                    member_range,
                    member.kind,
                    &format!("merged into {}-{}", merged_range.start, merged_range.end),
                ));
            }
            index = next;
        }
    }

    (merged, excluded)
}

/// The project root that owns `path`, or `None` when nothing can say.
///
/// `RepositoryMap::root_for_path` returns `""` for the repository root — the
/// map's own representation of it — so a root-level file carries `Some("")`
/// rather than `None`. An explicit path outside the repository keeps an
/// absolute spelling after normalisation and has no owning root here.
fn owning_root(map: &RepositoryMap, path: &str) -> Option<String> {
    if Path::new(path).is_absolute() {
        return None;
    }
    Some(map.root_for_path(path).to_string())
}

/// The most authoritative provenance among the parts: `LanguageServer` sorts
/// before `Heuristic`, so the best is the minimum, unlike the symbols block's
/// "weakest shown".
fn best_provenance(candidates: &[&FileCandidate]) -> Option<Provenance> {
    candidates
        .iter()
        .filter_map(|candidate| candidate.provenance)
        .min()
}

fn range_exclusion(
    path: &str,
    range: LineRange,
    kind: ContextKind,
    detail: &str,
) -> ExclusionRecord {
    ExclusionRecord {
        reason: ExclusionReason::DeduplicatedIntoRange,
        path: Some(path.to_string()),
        range: Some(range),
        category: kind.category(),
        detail: Some(detail.to_string()),
    }
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
    /// The shares category budgeting divides the limit by (proposal §5.2).
    budget: ContextBudget,
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
    // The engine's one construction seam: every field is a collaborator the
    // engine already holds, and spec 27 adds a pinned store here. A params
    // struct would be built at this single call site only to be destructured.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        file_access: FileAccessController,
        path_policy: PathPolicy,
        scanner: SecretScanner,
        data_dir: PathBuf,
        enable_semantic_search: bool,
        repository_map: RepositoryMapSource,
        symbols: SymbolSource,
        budget: ContextBudget,
    ) -> Self {
        Self {
            file_access,
            path_policy,
            scanner,
            data_dir,
            enable_semantic_search,
            repository_map,
            symbols,
            budget,
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
            CollectionSource::Pinned => self.path_policy.is_restricted(path, false),
        };
        restricted.then_some(ExclusionReason::RestrictedPath)
    }

    /// The candidate path as the read path will see it (Task 5 Step 3a).
    ///
    /// [`Self::collection_exclusion`] compares strings, while `read_file`
    /// resolves the request first: it strips `./`, collapses `..` and turns an
    /// absolute in-repo path into a repository-relative one before
    /// `assert_not_restricted`. Checking the raw spelling let a non-normalised
    /// restricted path through collection, so it was refused at read and
    /// dropped without a record — the manifest could not say why the file was
    /// absent. This is the read path's own normaliser, not a second one.
    ///
    /// A path that does not resolve (absent, or outside the root when that is
    /// not allowed) is returned as it came: it cannot be read either way, and
    /// rewriting it would invent a path the caller did not name.
    fn normalise_candidate(
        &self,
        repository_root: &Path,
        path: &str,
        allow_outside_root: bool,
    ) -> String {
        self.path_policy
            .resolve_existing(repository_root, path, allow_outside_root)
            .map(|resolved| resolved.relative_path)
            .unwrap_or_else(|_| path.to_string())
    }

    /// Normalise stored restriction entries exactly as assembly does: a path
    /// that resolves to a directory gains a trailing `/`, so the same entry
    /// selects the same files in tool dispatch and in the context panel.
    pub fn normalise_restriction(&self, repository_root: &Path, paths: &[String]) -> Vec<String> {
        paths
            .iter()
            .map(|path| {
                let normalised = self.normalise_candidate(repository_root, path, false);
                if normalised.is_empty() {
                    String::new()
                } else if repository_root.join(&normalised).is_dir() {
                    format!("{}/", normalised.trim_end_matches('/'))
                } else {
                    normalised
                }
            })
            .collect()
    }

    /// Whether a repository-relative path is admitted by the session's path
    /// restriction (spec 27 §5.4).
    ///
    /// `None` is the repository root. The comparison is the same one assembly
    /// uses, so a tool cannot accept a spelling that the panel would refuse.
    pub fn restriction_admits_path(
        &self,
        repository_root: &Path,
        paths: &[String],
        path: Option<&str>,
    ) -> bool {
        let normalised = path
            .map(|path| self.normalise_candidate(repository_root, path, false))
            .unwrap_or_default();
        self.normalise_restriction(repository_root, paths)
            .iter()
            .any(|selected| restriction_admits(&normalised, selected))
    }

    pub fn build_context(&self, request: ContextRequest<'_>) -> Result<ContextPlan> {
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
            pinned,
            restriction,
            conversation_summary,
            external_references,
            memory,
        } = request;

        // Normalise the restriction once, with the read path's own resolver,
        // so `./src/x` and `src/x` cannot disagree about what is selected.
        // A path that does not resolve keeps its spelling: it selects nothing,
        // which is the narrowing direction. A path that resolves to a directory
        // keeps a trailing `/` so `add_file` can prefix-match it; without the
        // marker every entry would have to be treated as a prefix, and a typo
        // like `src/up` would admit `src/upload.rs`.
        let restriction =
            restriction.map(|paths| self.normalise_restriction(repository_root, paths));
        let restriction = restriction.as_deref();
        let summary_usage = conversation_summary.map(|summary| {
            let rendered = self.scanner.redact(&summary.render_for_model()).text;
            ConversationSummaryUsage {
                tokens: TokenEstimate::Prose.tokens(&rendered),
                messages: summary.message_count,
                through_seq: summary.through_seq,
                facts_only: summary.is_facts_only(),
                narrative_carried_forward: summary.narrative_carried_forward,
            }
        });

        let mut pending = Vec::new();
        // Every whole file collected so far, for dedup and range absorption.
        // The plan's `files` is recomputed from the budgeted items at the end,
        // so a path dropped by its category share is not reported as included.
        let mut files = Vec::new();
        let mut excluded = Vec::new();
        // Ranged candidates are read once, after collection, so ranges for one
        // path can be merged and absorbed (proposal §5.4). Whole-file items are
        // still read where they are collected, which is what keeps the prompt's
        // item order unchanged.
        let mut range_candidates = Vec::new();

        // Requirement 6: every included file carries its owning root, so the
        // map is loaded even when neither the map nor the symbols block is
        // rendered. `load_or_build` reuses the stored map, so a repository with
        // both ceilings at 0 does not pay for a second build.
        let repository_map = index.and_then(|index| self.load_map(index));

        push_text(
            &mut pending,
            add_text(
                &self.scanner,
                ContextKind::UserPrompt,
                None,
                None,
                None,
                prompt,
            ),
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
                None,
                None,
                source,
                ContextKind::ExplicitFile,
                &mut files,
                &mut pending,
                &mut excluded,
                &mut range_candidates,
                repository_map.as_ref(),
                requested,
                restriction,
            );
        }

        for instruction_path in agent_instruction_paths(&context_paths) {
            self.add_file(
                repository_root,
                repository_id,
                task_id,
                &instruction_path,
                None,
                None,
                CollectionSource::Instruction,
                ContextKind::AgentInstruction,
                &mut files,
                &mut pending,
                &mut excluded,
                &mut range_candidates,
                repository_map.as_ref(),
                false,
                restriction,
            );
        }

        for rule_path in PROJECT_RULES {
            self.add_file(
                repository_root,
                repository_id,
                task_id,
                rule_path,
                None,
                None,
                CollectionSource::ProjectRule,
                ContextKind::ProjectRule,
                &mut files,
                &mut pending,
                &mut excluded,
                &mut range_candidates,
                repository_map.as_ref(),
                false,
                restriction,
            );
        }

        // The session's pins (spec 27 §5.4). Collected through the same path as
        // every other file, so the path policy still decides and a pin that is
        // restricted, outside the restriction or deleted becomes a record
        // rather than disappearing. The pin list itself is never rewritten by a
        // failing read: restoring the file restores the pin.
        for pin in pinned {
            self.add_file(
                repository_root,
                repository_id,
                task_id,
                &pin.path,
                pin.range,
                None,
                CollectionSource::Pinned,
                ContextKind::Pinned,
                &mut files,
                &mut pending,
                &mut excluded,
                &mut range_candidates,
                repository_map.as_ref(),
                false,
                restriction,
            );
        }

        // After instructions and rules, before retrieved files: under a tight
        // budget the map displaces retrieved files, never instructions. The
        // block is kept unrendered so its category share can choose the ceiling
        // (Correction C14); `symbols_block` needs the map first. The map is
        // cloned into the item because `repository_map` stays available for the
        // root of the files collected after this point.
        let symbols = index
            .zip(repository_map.as_ref())
            .and_then(|(index, map)| self.symbols_block(index, map, prompt));
        if self.repository_map.max_tokens > 0
            && let Some(map) = &repository_map
        {
            pending.push(PendingItem::Block(BlockItem {
                kind: ContextKind::RepositoryMap,
                max_tokens: self.repository_map.max_tokens,
                content: BlockContent::Map(map.clone()),
            }));
        }
        if let Some((index, matches)) = symbols {
            pending.push(PendingItem::Block(BlockItem {
                kind: ContextKind::Symbols,
                max_tokens: self.symbols.max_tokens,
                content: BlockContent::Symbols { index, matches },
            }));
        }

        // A hook's additions are ordinary items: redacted by `add_text` and
        // charged to the same budget, so an over-long one is dropped rather
        // than displacing the prompt or the instructions (spec 32 §5.2,
        // `context.md` §10). Before the retrieved files, like the map and the
        // symbols, because the hook's rule is about the turn, not the search.
        for (hook_id, text) in hook_context {
            push_text(
                &mut pending,
                add_text(
                    &self.scanner,
                    ContextKind::HookContext,
                    Some(format!("hook:{hook_id}")),
                    None,
                    None,
                    text,
                ),
            );
        }

        // A plan a resumed turn carried (`chat.rs`'s
        // `carry_plan_from_the_previous_turn`) is ordinary, redacted, charged
        // context — not a privileged channel (proposal §5.11).
        if let Some(text) = plan.and_then(plan_state_text) {
            push_text(
                &mut pending,
                add_text(
                    &self.scanner,
                    ContextKind::PlanState,
                    None,
                    None,
                    None,
                    &text,
                ),
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
                    None,
                    None,
                    source,
                    ContextKind::RetrievedFile,
                    &mut files,
                    &mut pending,
                    &mut excluded,
                    &mut range_candidates,
                    repository_map.as_ref(),
                    false,
                    restriction,
                );
            }
        }

        // After collection, so ranges for one path can merge and a whole-file
        // item from an earlier stage can absorb them. A session pin is the one
        // production source that can ask for a range (spec 27 §5.4); automatic
        // collection is still whole-file.
        self.flush_file_ranges(
            repository_root,
            repository_id,
            task_id,
            &range_candidates,
            &mut files,
            &mut pending,
            &mut excluded,
        );

        // A fetched page (spec 51 §5.4, D5). Collected after every repository
        // source so it follows repository content in the prompt, and charged to
        // the fetched-page category, which drops before any repository content.
        // Memory (spec 30) is the one category that drops earlier still. The
        // text is already converted, redacted and bounded by the fetch task:
        // this is not a second conversion, and the provenance label travels
        // with it.
        for reference in external_references {
            push_text(
                &mut pending,
                external_reference_item(&self.scanner, reference),
            );
        }

        // Recalled memory (spec 30 §5.1). One item per entry, collected last
        // because `Memory` is the lowest-priority category and first dropped
        // under pressure. A statement is untrusted contextual data rendered
        // inside the §5.2 frame, never instructions. The ref is kept beside the
        // pending index so `apply_budget` can record only what it actually
        // included (C14).
        let mut memory_refs = Vec::new();
        for recalled in memory {
            let Some(item) = memory_item(&self.scanner, recalled) else {
                continue;
            };
            memory_refs.push((pending.len(), MemoryRecallRef::from_recalled(recalled)));
            pending.push(PendingItem::Item(item));
        }

        self.apply_budget(
            repository_id,
            task_id,
            pending,
            EffectiveLimit::declared(limit),
            excluded,
            summary_usage,
            memory_refs,
        )
    }

    /// Divide the limit by category share, drop or shorten over-share items, and
    /// build the plan (proposal §5.2, §5.7).
    ///
    /// `Required` is subtracted before any category is allocated: a required
    /// item that does not fit is not a budgeting problem but an error the user
    /// must see. Unused allocation is released to later categories in pipeline
    /// order; a category over its share truncates within itself rather than
    /// stealing from the next.
    // Each argument is one independent input to the pass, and the manifest
    // fields travel beside the item list they describe; a params struct would
    // only be destructured here, as with `build_manifest`.
    #[allow(clippy::too_many_arguments)]
    fn apply_budget(
        &self,
        repository_id: &str,
        task_id: &str,
        pending: Vec<PendingItem>,
        limit: EffectiveLimit,
        mut excluded: Vec<ExclusionRecord>,
        conversation_summary: Option<ConversationSummaryUsage>,
        memory_refs: Vec<(usize, MemoryRecallRef)>,
    ) -> Result<ContextPlan> {
        let mut symbol_provenance: Option<SymbolProvenanceCounts> = None;
        let required_tokens: usize = pending
            .iter()
            .filter(|item| item.category() == ContextCategory::Required)
            .map(PendingItem::tokens)
            .sum();
        if required_tokens > limit.tokens {
            return Err(ClientError::InvalidInput(format!(
                "Required context needs {required_tokens} tokens but the context budget is {}",
                limit.tokens
            )));
        }
        let available = limit.tokens - required_tokens;

        let mut indices: [Vec<usize>; 13] = std::array::from_fn(|_| Vec::new());
        for (index, item) in pending.iter().enumerate() {
            indices[item.category() as usize].push(index);
        }

        let mut placed: Vec<Option<ContextItem>> = vec![None; pending.len()];
        let mut categories = Vec::with_capacity(13);

        // Required is never budgeted and never dropped (proposal §5.1).
        let required = &indices[ContextCategory::Required as usize];
        for &index in required {
            if let PendingItem::Item(item) = &pending[index] {
                placed[index] = Some(item.clone());
            }
        }
        categories.push(CategoryUsage {
            category: ContextCategory::Required,
            allocated: required_tokens,
            used: required_tokens,
            items: required.len(),
            truncated: false,
            estimated: true,
        });

        // Unused allocation, released to the next category in pipeline order.
        //
        // A zero share is switched off. It takes no allocation and does not
        // intercept what earlier categories released; the carry passes through
        // to the next enabled category. Otherwise the last categories would
        // always spend other categories' leftovers and `context_budget.*=0`
        // could never turn one off — the assumption the test helper that zeroes
        // every share already makes.
        let mut carry = 0usize;
        for category in ContextCategory::BUDGETED {
            let share = self.budget.share(category);
            // Integer basis points all the way: no float decides inclusion.
            let nominal = (available as u128 * u128::from(share) / 10_000) as usize;
            let allocation = if share == 0 {
                0
            } else {
                nominal.saturating_add(carry)
            };
            let mut used = 0usize;
            let mut kept = 0usize;
            let mut truncated = false;
            let mut exhausted = false;

            for index in self.priority_order(category, &pending, &indices[category as usize]) {
                if exhausted {
                    budget_exclusion(&mut excluded, &pending[index], category);
                    truncated = true;
                    continue;
                }
                match &pending[index] {
                    PendingItem::Item(item) => {
                        if used + item.tokens <= allocation {
                            used += item.tokens;
                            kept += 1;
                            placed[index] = Some(item.clone());
                        } else {
                            budget_exclusion(&mut excluded, &pending[index], category);
                            truncated = true;
                            exhausted = true;
                        }
                    }
                    PendingItem::Block(block) => {
                        let Some(rendered) = self.render_block(block, block.max_tokens) else {
                            // Nothing to send even at its configured ceiling.
                            continue;
                        };
                        let Some(full) = add_text(
                            &self.scanner,
                            block.kind,
                            None,
                            None,
                            rendered.provenance,
                            &rendered.text,
                        ) else {
                            continue;
                        };
                        if used + full.tokens <= allocation {
                            used += full.tokens;
                            kept += 1;
                            if let Some(counts) = rendered.symbols {
                                symbol_provenance = Some(counts);
                            }
                            placed[index] = Some(full);
                            continue;
                        }
                        // Re-render at the share, through the block's own
                        // renderer, then measure the re-rendered text through
                        // `add_text` (C14). Never slice redacted text.
                        let remaining = allocation - used;
                        let shortened = self
                            .render_block(block, remaining)
                            .and_then(|rendered| {
                                add_text(
                                    &self.scanner,
                                    block.kind,
                                    None,
                                    None,
                                    rendered.provenance,
                                    &rendered.text,
                                )
                                .map(|item| (item, rendered.symbols))
                            })
                            .filter(|(item, _)| item.tokens <= remaining);
                        if let Some((item, counts)) = shortened {
                            used += item.tokens;
                            kept += 1;
                            if let Some(counts) = counts {
                                symbol_provenance = Some(counts);
                            }
                            placed[index] = Some(item);
                            truncated = true;
                            excluded.push(ExclusionRecord {
                                reason: ExclusionReason::Truncated,
                                path: None,
                                range: None,
                                category,
                                detail: Some(format!(
                                    "re-rendered at its category share of {remaining} tokens"
                                )),
                            });
                        } else {
                            budget_exclusion(&mut excluded, &pending[index], category);
                            truncated = true;
                            exhausted = true;
                        }
                    }
                }
            }

            carry = if share == 0 {
                carry
            } else {
                allocation.saturating_sub(used)
            };
            categories.push(CategoryUsage {
                category,
                allocated: allocation,
                used,
                items: kept,
                truncated,
                estimated: true,
            });
        }

        // Only an included memory item keeps its manifest row. A dropped entry
        // is reported through `excluded` like any other category item (C14).
        let memory = memory_refs
            .into_iter()
            .filter(|(index, _)| placed[*index].is_some())
            .map(|(_, reference)| reference)
            .collect::<Vec<_>>();
        let items: Vec<ContextItem> = placed.into_iter().flatten().collect();
        let token_estimate = items.iter().map(|item| item.tokens).sum();
        let manifest = build_manifest(
            repository_id,
            task_id,
            limit,
            &items,
            categories,
            excluded,
            conversation_summary,
            symbol_provenance,
            memory,
        );

        Ok(ContextPlan {
            repository_id: repository_id.to_string(),
            task_id: task_id.to_string(),
            token_estimate,
            items,
            files: manifest
                .included
                .iter()
                .filter_map(|included| included.path.clone())
                .fold(Vec::new(), |mut paths, path| {
                    if !paths.contains(&path) {
                        paths.push(path);
                    }
                    paths
                }),
            manifest,
        })
    }

    /// A category's items in inclusion priority, highest first.
    ///
    /// Only `Instructions` differs from collection order: prompt order stays
    /// root-before-nested, but truncation keeps the most specific `AGENTS.md`
    /// (nested instructions override broader ones), then broader files, then
    /// the root project rules (proposal §5.3).
    fn priority_order(
        &self,
        category: ContextCategory,
        pending: &[PendingItem],
        indices: &[usize],
    ) -> Vec<usize> {
        let mut order = indices.to_vec();
        if category == ContextCategory::Instructions {
            order.sort_by_key(|&index| {
                let (project_rule, depth) = match &pending[index] {
                    PendingItem::Item(item) if item.kind == ContextKind::AgentInstruction => (
                        false,
                        item.path
                            .as_deref()
                            .map_or(0, |path| path.matches('/').count()),
                    ),
                    _ => (true, 0),
                };
                (project_rule, std::cmp::Reverse(depth), index)
            });
        }
        order
    }

    /// Render a block at `max_tokens` through its own renderer (Correction C14).
    fn render_block(&self, block: &BlockItem, max_tokens: usize) -> Option<RenderedBlock> {
        if max_tokens == 0 {
            return None;
        }
        match &block.content {
            BlockContent::Map(map) => {
                let rendered = map.render_for_model(max_tokens);
                (!rendered.text.is_empty()).then_some(RenderedBlock {
                    text: rendered.text,
                    provenance: None,
                    symbols: None,
                })
            }
            BlockContent::Symbols { index, matches } => {
                let rendered = index.render_for_model(matches, max_tokens);
                if rendered.text.is_empty() {
                    return None;
                }
                // The weakest provenance among the lines actually shown, as
                // before: one heuristic symbol makes the whole block a guess.
                let shown = matches.iter().take(rendered.shown).collect::<Vec<_>>();
                let provenance = shown.iter().map(|matched| matched.symbol.provenance).max();
                // The counts spec 27 §5.2 shows beside the block: a single
                // `IncludedRef` can only carry the weakest provenance, not "12
                // symbols (4 heuristic)".
                let heuristic = shown
                    .iter()
                    .filter(|matched| matched.symbol.provenance == Provenance::Heuristic)
                    .count();
                let symbols = SymbolProvenanceCounts {
                    total: shown.len(),
                    language_server: shown.len() - heuristic,
                    heuristic,
                };
                Some(RenderedBlock {
                    text: rendered.text,
                    provenance,
                    symbols: Some(symbols),
                })
            }
        }
    }

    /// The map both items and root resolution are built from, or `None` when it
    /// cannot be loaded or stored. The map is an aid: a read-only or broken data
    /// directory leaves it out rather than failing the turn.
    ///
    /// Not gated on the render ceilings: `build_context` uses the map to name
    /// each file's owning root (requirement 6) even when neither the map nor the
    /// symbols item is rendered, and `load_or_build` reuses the stored map.
    fn load_map(&self, index: &RepositoryIndex) -> Option<RepositoryMap> {
        let source = &self.repository_map;
        let (map, _) = source
            .store
            .load_or_build(index, &source.command_policy)
            .ok()?;
        Some(map)
    }

    /// The symbols the prompt's identifiers name, as data rather than rendered
    /// text: the category share picks the ceiling, so the render happens in the
    /// budget pass (Correction C14). `None` when the ceiling is `0`, nothing
    /// matches, or the symbol index cannot be loaded or stored, which leaves
    /// the item out rather than failing the turn. It reads the stored index as
    /// it is, whatever an eager definition pass has merged so far, and never
    /// waits on a language server (`context.md` §8).
    fn symbols_block(
        &self,
        index: &RepositoryIndex,
        map: &RepositoryMap,
        prompt: &str,
    ) -> Option<(SymbolIndex, Vec<SymbolMatch>)> {
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
        Some((symbols, matches))
    }

    #[allow(clippy::too_many_arguments)]
    fn add_file(
        &self,
        repository_root: &Path,
        repository_id: &str,
        task_id: &str,
        path: &str,
        range: Option<LineRange>,
        provenance: Option<Provenance>,
        source: CollectionSource,
        kind: ContextKind,
        files: &mut Vec<String>,
        pending: &mut Vec<PendingItem>,
        excluded: &mut Vec<ExclusionRecord>,
        range_candidates: &mut Vec<FileCandidate>,
        roots: Option<&RepositoryMap>,
        allow_outside_root: bool,
        restriction: Option<&[String]>,
    ) {
        // Normalise before the dedup, the collection check and the candidate
        // list, so all three compare what the read path will actually open
        // (Task 5 Step 3a).
        let normalised = self.normalise_candidate(repository_root, path, allow_outside_root);
        let path = normalised.as_str();
        // Requirement 6: every included file names its owning root. Resolved
        // here, where the assembler still knows this is a file and not a
        // session block with a path-shaped label.
        let root_path = roots.and_then(|map| owning_root(map, path));
        // The session restriction narrows file-bearing collection only
        // (spec 27 §5.4, context.md C10). `AGENTS.md` is repository policy, not
        // one of the files the user is choosing between, so it is exempt; the
        // policy check below still applies to it. A restriction entry that names
        // a directory carries a trailing `/` from `build_context`, and selects
        // everything under it; without that, `src/` selected nothing at all.
        if kind != ContextKind::AgentInstruction
            && let Some(allowed) = restriction
            && !allowed
                .iter()
                .any(|selected| restriction_admits(path, selected))
        {
            push_exclusion(
                excluded,
                ExclusionRecord {
                    reason: ExclusionReason::PathNotSelected,
                    path: Some(path.to_string()),
                    range,
                    category: kind.category(),
                    detail: Some("the session's path restriction does not include it".to_string()),
                },
            );
            return;
        }
        // Whole files are deduplicated here, as they always were. A range for
        // a path already included whole is kept as a candidate so the merge can
        // record it as absorbed rather than silently dropped (§5.4).
        if range.is_none() && files.iter().any(|existing| existing == path) {
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
                    range,
                    category: kind.category(),
                    detail: None,
                },
            );
            return;
        }

        if let Some(range) = range {
            range_candidates.push(FileCandidate {
                path: path.to_string(),
                range: Some(range),
                kind,
                provenance,
                root_path,
                allow_outside_root,
            });
            return;
        }

        // `Whole`, not the tool's bounded window: assembly's category budget
        // decides what fits. Capping here would silently shrink what every task
        // sees.
        let file = match self.file_access.read_file(
            repository_root,
            path,
            Some(task_id),
            Some(repository_id),
            false,
            allow_outside_root,
            crate::file_access::ReadWindow::Whole,
        ) {
            Ok(file) => file,
            Err(error) => {
                // A path can still be refused at read time (a symlink swapped
                // between the check and the read, or the size cap and the
                // binary check, which only the read knows about). Record what
                // the error says rather than dropping it silently.
                if !record_read_failure(excluded, path, range, kind, &error)
                    && kind == ContextKind::Pinned
                {
                    // A pin is the user's standing request: a file that no
                    // longer reads is reported rather than dropped from the
                    // list (spec 27 §5.4).
                    push_exclusion(
                        excluded,
                        ExclusionRecord {
                            reason: ExclusionReason::Missing,
                            path: Some(path.to_string()),
                            range,
                            category: kind.category(),
                            detail: Some("the pinned file could not be read".to_string()),
                        },
                    );
                }
                return;
            }
        };
        let added = push_text(
            pending,
            add_text(
                &self.scanner,
                kind,
                Some(file.path.clone()),
                None,
                provenance,
                &file.content,
            )
            .map(|mut item| {
                item.root_path = root_path;
                item
            }),
        );
        if added {
            files.push(file.path);
        }
    }

    /// Read each merged range once, as one item (proposal §5.4).
    ///
    /// Ranges use [`crate::file_access::ReadWindow::ContextRange`]: no line
    /// cap, because assembly's own token budget is the bound and a merge cut at
    /// the tool's 400 lines would be silently wrong. Path policy, the byte cap
    /// and redaction are `read_file`'s and are not reimplemented here.
    #[allow(clippy::too_many_arguments)]
    fn flush_file_ranges(
        &self,
        repository_root: &Path,
        repository_id: &str,
        task_id: &str,
        range_candidates: &[FileCandidate],
        files: &mut Vec<String>,
        pending: &mut Vec<PendingItem>,
        excluded: &mut Vec<ExclusionRecord>,
    ) {
        if range_candidates.is_empty() {
            return;
        }
        let whole_files = files.clone();
        let (merged, absorbed) = merge_file_ranges(range_candidates, &whole_files);
        for record in absorbed {
            push_exclusion(excluded, record);
        }

        for file in merged {
            let window = match file.range {
                Some(range) => crate::file_access::ReadWindow::ContextRange(range),
                None => crate::file_access::ReadWindow::Whole,
            };
            let read = match self.file_access.read_file(
                repository_root,
                &file.path,
                Some(task_id),
                Some(repository_id),
                false,
                file.allow_outside_root,
                window,
            ) {
                Ok(read) => read,
                Err(error) => {
                    // The collection-time check already refused restricted
                    // paths; what is left is a read-time refusal the error can
                    // name (a restriction that appeared since, the size cap, a
                    // binary file) or an ordinary missing/unreadable file. Only
                    // the first becomes a record, with a fixed detail, except
                    // for a pinned range, which reports the loss.
                    if !record_read_failure(excluded, &file.path, file.range, file.kind, &error)
                        && file.kind == ContextKind::Pinned
                    {
                        push_exclusion(
                            excluded,
                            ExclusionRecord {
                                reason: ExclusionReason::Missing,
                                path: Some(file.path.clone()),
                                range: file.range,
                                category: file.kind.category(),
                                detail: Some("the pinned file could not be read".to_string()),
                            },
                        );
                    }
                    continue;
                }
            };
            // The header names what was actually read, so a range clamped to the
            // file's length cannot claim lines that do not exist.
            let added = push_text(
                pending,
                add_text(
                    &self.scanner,
                    file.kind,
                    Some(read.path.clone()),
                    file.range.map(|_| read.line_range),
                    file.provenance,
                    &read.content,
                )
                .map(|mut item| {
                    item.root_path = file.root_path;
                    item
                }),
            );
            if added && !files.iter().any(|existing| existing == &file.path) {
                files.push(read.path);
            }
        }
    }
}

/// Shape the record of one assembly.
///
/// Kept separate from `apply_budget` so the manifest can be tested against every
/// [`ExclusionReason`], including ones today's collection cannot yet produce
/// (no collection source yields a line range — Task 3's discovery).
#[allow(clippy::too_many_arguments)]
fn build_manifest(
    repository_id: &str,
    task_id: &str,
    limit: EffectiveLimit,
    items: &[ContextItem],
    categories: Vec<CategoryUsage>,
    excluded: Vec<ExclusionRecord>,
    conversation_summary: Option<ConversationSummaryUsage>,
    symbol_provenance: Option<SymbolProvenanceCounts>,
    memory: Vec<MemoryRecallRef>,
) -> ContextManifest {
    ContextManifest {
        repository_id: repository_id.to_string(),
        task_id: task_id.to_string(),
        assembled_at_ms: now_millis(),
        limit,
        total_tokens: items.iter().map(|item| item.tokens).sum(),
        categories,
        included: items.iter().map(IncludedRef::from_item).collect(),
        excluded,
        conversation_summary,
        symbol_provenance,
        memory,
    }
}

/// Whether a normalised path is admitted by one entry of the session's path
/// restriction (spec 27 §5.4).
///
/// `build_context` marks a directory entry with a trailing `/`; a file must
/// match exactly. The empty entry is the repository root and admits everything.
/// Exact equality for a directory would have made the most natural way to
/// restrict — `src/` — select nothing, which reads as "no file content" rather
/// than "everything under `src`". The directory entry itself is admitted too,
/// so `list_directory` can show the directory the user selected rather than
/// refusing the only listing that is entirely inside the restriction.
fn restriction_admits(path: &str, selected: &str) -> bool {
    selected.is_empty()
        || path == selected
        || (selected.ends_with('/')
            && (path == selected.trim_end_matches('/') || path.starts_with(selected)))
}

/// Record an exclusion once per `(reason, path, range)`. The same restricted
/// path can be reached by more than one source — a prompt mention is also a
/// keyword hit — and the manifest wants the fact, not a copy per route. The
/// range is part of the key because two different ranges for one path are two
/// facts (proposal §5.4 records every absorbed range). A path-less record has no
/// identity beyond its category, so two path-less items in different categories
/// (a block and a fetched page) are two facts rather than one.
fn push_exclusion(excluded: &mut Vec<ExclusionRecord>, record: ExclusionRecord) {
    let duplicate = excluded.iter().any(|existing| {
        existing.reason == record.reason
            && existing.path == record.path
            && existing.range == record.range
            && (record.path.is_some() || existing.category == record.category)
    });
    if !duplicate {
        excluded.push(record);
    }
}

/// Record a read the assembler attempted and could not use, when the read
/// path's error says why (Task 5 Step 3a).
///
/// `read_file` distinguishes three refusals by fixed messages: a policy
/// restriction, the whole-file size cap, and a binary file. Nothing else it
/// returns has a reason in proposal §5.5's list — a missing file or a path
/// outside the root is not a dropped item — so those stay unrecorded rather
/// than carrying a fabricated reason. The detail is never the error text: it
/// names the path and the manifest is not the place for a second copy of it.
/// Returns whether the error named a reason this manifest can record.
fn record_read_failure(
    excluded: &mut Vec<ExclusionRecord>,
    path: &str,
    range: Option<LineRange>,
    kind: ContextKind,
    error: &ClientError,
) -> bool {
    let ClientError::AccessDenied(message) = error else {
        return false;
    };
    let reason = if message.starts_with("Path is restricted by policy:") {
        ExclusionReason::RestrictedPath
    } else if message == "File exceeds configured size limit" {
        ExclusionReason::TooLarge
    } else if message == "Binary file reads are denied by default" {
        ExclusionReason::Binary
    } else {
        return false;
    };
    push_exclusion(
        excluded,
        ExclusionRecord {
            reason,
            path: Some(path.to_string()),
            range,
            category: kind.category(),
            detail: None,
        },
    );
    true
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

/// Redact, measure and shape one item. The only place item content is created
/// (proposal §5.6): redaction runs before the estimate, so nothing unredacted is
/// ever measured or stored. Category budgeting happens afterwards in
/// [`ContextManager::apply_budget`], which is why this no longer refuses an item
/// for budget — the `Required` item's refusal is a visible error there.
fn add_text(
    scanner: &SecretScanner,
    kind: ContextKind,
    path: Option<String>,
    range: Option<LineRange>,
    provenance: Option<Provenance>,
    content: &str,
) -> Option<ContextItem> {
    if content.is_empty() {
        return None;
    }
    let redaction = scanner.redact(content);
    let tokens = TokenEstimate::for_item(kind, path.as_deref()).tokens(&redaction.text);
    Some(ContextItem {
        category: kind.category(),
        kind,
        path,
        range,
        content: redaction.text,
        tokens,
        redaction_status: if redaction.findings.is_empty() {
            "clean".to_string()
        } else {
            "redacted".to_string()
        },
        provenance,
        root_path: None,
        origin: None,
    })
}

/// A fetched page as a context item (spec 51 §5.4, D7).
///
/// The text arrives converted, redacted and bounded, so this never re-runs
/// conversion. It is rendered through the same data frame as the fetching
/// turn's tool result: a later assembly must not turn a page into bare prompt
/// text, where `--- file: AGENTS.md ---` would read as a section boundary. It
/// still goes through `add_text`, which measures it with the same estimator
/// every other item uses and scans it once more; the status the fetch recorded
/// is kept, because the second scan sees the first scan's placeholders rather
/// than the credential they replaced.
fn external_reference_item(
    scanner: &SecretScanner,
    reference: &ExternalReference,
) -> Option<ContextItem> {
    add_text(
        scanner,
        ContextKind::ExternalReference,
        None,
        None,
        None,
        &reference.render_for_model(),
    )
    .map(|mut item| {
        item.origin = Some(reference.origin.clone());
        if reference.redaction_status == "redacted" {
            item.redaction_status = "redacted".to_string();
        }
        item
    })
}

/// A recalled memory as a context item (spec 30 §5.2).
///
/// The frame is descriptive, never imperative: a statement is a claim about the
/// world the model may act on or disbelieve, like a file's contents, not an
/// instruction. Provenance, date and staleness travel inside the text the model
/// reads, because a flag in the manifest satisfies requirement 4 for nobody.
/// Redaction runs in [`add_text`]; recall has already cleaned the statement it
/// returns, so `recalled.redacted` restores the status the second scan cannot
/// see past the replacement marker (context.md C13.2).
fn memory_item(scanner: &SecretScanner, recalled: &RecalledMemory) -> Option<ContextItem> {
    add_text(
        scanner,
        ContextKind::Memory,
        None,
        None,
        None,
        &render_memory(recalled),
    )
    .map(|mut item| {
        if recalled.redacted {
            item.redaction_status = "redacted".to_string();
        }
        item
    })
}

/// The proposal §5.2 frame, with the provenance and date inline.
///
/// `created_at_ms` is the entry's own date, not today's: the frame answers when
/// the statement was recorded. A check performed now is said as a check rather
/// than written onto the statement as a fact.
fn render_memory(recalled: &RecalledMemory) -> String {
    let entry = &recalled.entry;
    let date = utc_date(entry.created_at_ms);
    let frame = match (&entry.provenance, recalled.verification) {
        (MemoryProvenance::RepositoryEvidence { paths }, Verification::EvidenceStale) => {
            let changed = if paths.len() == 1 {
                "that file has changed since"
            } else {
                "those files have changed since"
            };
            format!(
                "Remembered about this project (derived from {}, recorded {date}, may be out of date — {changed}):",
                evidence_paths(paths)
            )
        }
        (MemoryProvenance::RepositoryEvidence { paths }, _) => format!(
            "Remembered about this project (derived from {}, recorded {date}, evidence unchanged):",
            evidence_paths(paths)
        ),
        _ => format!("Remembered about this project (user-confirmed, {date}):"),
    };
    format!("{frame}\n  {}\n", entry.statement)
}

fn evidence_paths(paths: &[crate::memory::EvidenceRef]) -> String {
    paths
        .iter()
        .map(|evidence| evidence.path.as_str())
        .collect::<Vec<_>>()
        .join(", ")
}

fn push_text(pending: &mut Vec<PendingItem>, item: Option<ContextItem>) -> bool {
    match item {
        Some(item) => {
            pending.push(PendingItem::Item(item));
            true
        }
        None => false,
    }
}

/// Record an item its category share could not hold (proposal §5.2). Never
/// content: the path, the range it would have carried, and the category.
fn budget_exclusion(
    excluded: &mut Vec<ExclusionRecord>,
    item: &PendingItem,
    category: ContextCategory,
) {
    let (path, range) = match item {
        PendingItem::Item(item) => (item.path.clone(), item.range),
        PendingItem::Block(_) => (None, None),
    };
    push_exclusion(
        excluded,
        ExclusionRecord {
            reason: ExclusionReason::CategoryBudgetExhausted,
            path,
            range,
            category,
            detail: Some("the category share is exhausted".to_string()),
        },
    );
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
        assert_eq!(
            serde_json::to_string(&ContextCategory::Memory).unwrap(),
            "\"memory\""
        );
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
            (ContextKind::ExternalReference, "external_reference"),
            (ContextKind::Memory, "memory"),
        ] {
            assert_eq!(kind.as_str(), spelling);
            assert_eq!(
                serde_json::to_string(&kind).unwrap(),
                format!("\"{spelling}\"")
            );
        }
    }

    #[test]
    fn every_exclusion_reason_reaches_the_manifest() {
        // `DeduplicatedIntoRange` has no producer through `build_context` yet
        // (no collection source yields a line range), so the manifest's
        // carry-through is pinned here for every variant rather than through a
        // fixture that does not exist.
        let reasons = [
            ExclusionReason::RestrictedPath,
            ExclusionReason::DeduplicatedIntoRange,
            ExclusionReason::CategoryBudgetExhausted,
            ExclusionReason::Truncated,
            ExclusionReason::TooLarge,
            ExclusionReason::Binary,
            ExclusionReason::UserRemoved,
            ExclusionReason::PathNotSelected,
            ExclusionReason::Missing,
        ];
        let excluded = reasons
            .iter()
            .map(|&reason| ExclusionRecord {
                reason,
                path: Some("src/lib.rs".to_string()),
                range: None,
                category: ContextCategory::FileRanges,
                detail: None,
            })
            .collect::<Vec<_>>();

        let manifest = build_manifest(
            "repo",
            "task",
            EffectiveLimit::declared(10),
            &[],
            Vec::new(),
            excluded.clone(),
            None,
            None,
            Vec::new(),
        );

        assert_eq!(manifest.excluded, excluded);
        let json = serde_json::to_string(&manifest).unwrap();
        for name in [
            "restricted_path",
            "deduplicated_into_range",
            "category_budget_exhausted",
            "truncated",
            "too_large",
            "binary",
            "user_removed",
            "path_not_selected",
            "missing",
        ] {
            assert!(json.contains(name), "{name} is missing from {json}");
        }
    }

    fn candidate(
        path: &str,
        range: Option<(usize, usize)>,
        provenance: Option<Provenance>,
    ) -> FileCandidate {
        FileCandidate {
            path: path.to_string(),
            range: range.map(|(start, end)| LineRange { start, end }),
            kind: ContextKind::RetrievedFile,
            provenance,
            root_path: None,
            allow_outside_root: false,
        }
    }

    #[test]
    fn two_search_hits_in_one_function_send_one_merged_range() {
        let merged = merge_file_ranges(
            &[
                candidate("src/lib.rs", Some((12, 20)), Some(Provenance::Heuristic)),
                candidate("src/lib.rs", Some((22, 30)), Some(Provenance::Heuristic)),
            ],
            &[],
        );

        assert_eq!(merged.0.len(), 1, "{:?}", merged.0);
        assert_eq!(merged.0[0].range, Some(LineRange { start: 12, end: 30 }));
        assert_eq!(
            merged
                .1
                .iter()
                .map(|record| record.range)
                .collect::<Vec<_>>(),
            vec![
                Some(LineRange { start: 12, end: 20 }),
                Some(LineRange { start: 22, end: 30 }),
            ]
        );
    }

    #[test]
    fn ranges_two_lines_apart_merge_and_ranges_eight_apart_do_not() {
        // Lines 21 and 22 sit between the first pair: `23 <= 20 + 1 + 2`.
        let (near, _) = merge_file_ranges(
            &[
                candidate("src/lib.rs", Some((10, 20)), None),
                candidate("src/lib.rs", Some((23, 30)), None),
            ],
            &[],
        );
        assert_eq!(near.len(), 1, "{near:?}");
        assert_eq!(near[0].range, Some(LineRange { start: 10, end: 30 }));

        // Eight lines between them: `29 > 20 + 1 + 2`, so two reads.
        let (far, records) = merge_file_ranges(
            &[
                candidate("src/lib.rs", Some((10, 20)), None),
                candidate("src/lib.rs", Some((29, 35)), None),
            ],
            &[],
        );
        assert_eq!(
            far.iter().map(|file| file.range).collect::<Vec<_>>(),
            vec![
                Some(LineRange { start: 10, end: 20 }),
                Some(LineRange { start: 29, end: 35 })
            ]
        );
        // Nothing was deduplicated: both ranges are read as they were asked for.
        assert!(records.is_empty(), "{records:?}");
    }

    #[test]
    fn a_whole_file_item_absorbs_every_range_for_its_path() {
        let (merged, records) = merge_file_ranges(
            &[
                candidate("src/lib.rs", None, None),
                candidate("src/lib.rs", Some((10, 20)), Some(Provenance::Heuristic)),
                candidate("src/lib.rs", Some((30, 40)), Some(Provenance::Heuristic)),
            ],
            &[],
        );

        assert_eq!(merged.len(), 1, "{merged:?}");
        assert_eq!(merged[0].range, None);
        assert_eq!(
            records
                .iter()
                .map(|record| (record.reason, record.range))
                .collect::<Vec<_>>(),
            vec![
                (
                    ExclusionReason::DeduplicatedIntoRange,
                    Some(LineRange { start: 10, end: 20 })
                ),
                (
                    ExclusionReason::DeduplicatedIntoRange,
                    Some(LineRange { start: 30, end: 40 })
                ),
            ]
        );

        // The production shape: the whole file was already included by an
        // earlier collection stage, so only the range is a candidate here.
        let (merged, records) = merge_file_ranges(
            &[candidate("src/lib.rs", Some((10, 20)), None)],
            &["src/lib.rs".to_string()],
        );
        assert!(merged.is_empty());
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].path.as_deref(), Some("src/lib.rs"));
        assert_eq!(records[0].range, Some(LineRange { start: 10, end: 20 }));
    }

    #[test]
    fn merging_keeps_the_language_server_provenance() {
        let (merged, _) = merge_file_ranges(
            &[
                candidate(
                    "src/lib.rs",
                    Some((10, 20)),
                    Some(Provenance::LanguageServer),
                ),
                candidate("src/lib.rs", Some((22, 30)), Some(Provenance::Heuristic)),
            ],
            &[],
        );

        assert_eq!(merged.len(), 1);
        // `LanguageServer` sorts before `Heuristic`, so the best of the parts
        // is the minimum, not the maximum the symbols block uses.
        assert_eq!(merged[0].provenance, Some(Provenance::LanguageServer));
    }

    #[test]
    fn every_absorbed_or_merged_range_is_recorded() {
        let (merged, records) = merge_file_ranges(
            &[
                candidate("src/lib.rs", Some((10, 20)), None),
                candidate("src/lib.rs", Some((15, 25)), None),
                candidate("src/lib.rs", Some((40, 50)), None),
            ],
            &[],
        );

        assert_eq!(
            merged.iter().map(|file| file.range).collect::<Vec<_>>(),
            vec![
                Some(LineRange { start: 10, end: 25 }),
                Some(LineRange { start: 40, end: 50 })
            ]
        );
        // The two parts of the first merge each produced a different range, so
        // both are recorded; the second was read exactly as asked and is not an
        // exclusion.
        assert_eq!(
            records
                .iter()
                .map(|record| record.range)
                .collect::<Vec<_>>(),
            vec![
                Some(LineRange { start: 10, end: 20 }),
                Some(LineRange { start: 15, end: 25 }),
            ]
        );
        assert!(
            records
                .iter()
                .all(|record| record.reason == ExclusionReason::DeduplicatedIntoRange)
        );
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
            origin: None,
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

    // -----------------------------------------------------------------
    // Memory delivery (spec 30 Task 3)
    // -----------------------------------------------------------------

    use crate::config::Config;
    use crate::memory::{EvidenceRef, MemoryCategory, MemoryEntry, MemoryScope, MemoryState};
    use std::fs;
    use std::sync::atomic::{AtomicU64, Ordering};

    static MEMORY_COUNTER: AtomicU64 = AtomicU64::new(1);

    /// 2026-03-14T00:00:00Z, the date the rendered frame must show.
    const MEMORY_DATE_MS: u128 = 1_773_446_400_000;
    const MEMORY_STATEMENT: &str = "Integration tests require Docker to be running.";
    const SECOND_STATEMENT: &str = "The Compose file names the web service.";

    fn memory_repo(
        name: &str,
        configure: impl FnOnce(&mut Config),
    ) -> (crate::workspace_engine::WorkspaceEngine, PathBuf) {
        let dir = std::env::temp_dir().join(format!(
            "damaian-memory-context-{name}-{}-{}",
            std::process::id(),
            MEMORY_COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("src")).unwrap();
        fs::write(dir.join("src/lib.rs"), "pub fn answer() -> usize { 42 }\n").unwrap();
        let mut config = Config {
            data_dir: dir.join(".damaian"),
            enable_index_watcher: false,
            ..Config::default()
        };
        configure(&mut config);
        (crate::workspace_engine::WorkspaceEngine::new(config), dir)
    }

    /// A recalled entry, built the way the store deserializes one. The store
    /// has no public constructor on purpose (spec 29's consent boundary); the
    /// delivery path only needs the shape recall returns.
    fn recall(
        id: &str,
        statement: &str,
        verification: Verification,
        provenance: MemoryProvenance,
    ) -> RecalledMemory {
        RecalledMemory {
            entry: MemoryEntry {
                id: id.to_string(),
                scope: MemoryScope::Project,
                project_key: Some("root_fixture".to_string()),
                session_id: None,
                category: MemoryCategory::Environment,
                statement: statement.to_string(),
                provenance,
                verification,
                sensitivity: crate::memory::Sensitivity::Ordinary,
                state: MemoryState::Active,
                created_at_ms: MEMORY_DATE_MS,
                updated_at_ms: MEMORY_DATE_MS,
                supersedes: None,
                expiry: None,
            },
            match_reason: MemoryMatchReason {
                matched_terms: vec!["docker".to_string()],
                category_boost: true,
            },
            verification,
            redacted: false,
        }
    }

    fn user_confirmed(id: &str, statement: &str) -> RecalledMemory {
        recall(
            id,
            statement,
            Verification::UserConfirmed,
            MemoryProvenance::UserRequest {
                session_id: "session_fixture".to_string(),
                message_id: "message_fixture".to_string(),
            },
        )
    }

    fn assemble(
        engine: &crate::workspace_engine::WorkspaceEngine,
        repo: &Path,
        memory: &[RecalledMemory],
        explicit_paths: &[String],
        limit: usize,
    ) -> ContextPlan {
        engine
            .context_manager
            .build_context(ContextRequest {
                repository_root: repo,
                repository_id: "repo_memory",
                task_id: "task_memory",
                prompt: "Review the Docker setup",
                index: None,
                explicit_paths,
                limit,
                hook_context: &[],
                plan: None,
                pinned: &[],
                restriction: None,
                conversation_summary: None,
                external_references: &[],
                memory,
            })
            .expect("the assembly should succeed")
    }

    /// The rendered repository-context string, which is what the prompt builder
    /// places before the user request.
    fn rendered(plan: &ContextPlan) -> String {
        plan.sections()
            .into_iter()
            .map(|section| section.text)
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn a_recalled_entry_reaches_the_prompt_as_its_own_memory_item() {
        let (engine, repo) = memory_repo("delivery", |_| {});
        let entry = user_confirmed("mem_docker", MEMORY_STATEMENT);
        let limit = engine.config.context_token_budget();
        let plan = assemble(&engine, &repo, std::slice::from_ref(&entry), &[], limit);

        let memory_items = plan
            .items
            .iter()
            .filter(|item| item.category == ContextCategory::Memory)
            .collect::<Vec<_>>();
        assert_eq!(memory_items.len(), 1, "{:?}", plan.items);
        let item = memory_items[0];
        assert_eq!(item.kind, ContextKind::Memory);
        assert!(item.path.is_none(), "memory has no file identity");
        assert_eq!(
            item.content,
            "Remembered about this project (user-confirmed, 2026-03-14):\n  \
             Integration tests require Docker to be running.\n"
        );
        assert_eq!(item.redaction_status, "clean");
        // Collected last, so the prompt puts memory after every other category.
        assert_eq!(
            plan.items.last().map(|item| item.kind),
            Some(ContextKind::Memory)
        );

        // The manifest carries the explanation, never the statement.
        assert_eq!(plan.manifest.memory.len(), 1);
        let reference = &plan.manifest.memory[0];
        assert_eq!(reference.entry_id, "mem_docker");
        assert_eq!(reference.verification, Verification::UserConfirmed);
        assert_eq!(reference.match_reason.matched_terms, vec!["docker"]);
        assert!(reference.match_reason.category_boost);
        let manifest_json = serde_json::to_string(&plan.manifest).unwrap();
        assert!(
            manifest_json.contains("\"entryId\":\"mem_docker\""),
            "{manifest_json}"
        );
        assert!(
            !manifest_json.contains("Integration tests require Docker"),
            "the manifest must stay content-free: {manifest_json}"
        );
        assert!(rendered(&plan).contains("Remembered about this project"));

        fs::remove_dir_all(repo).unwrap();
    }

    #[test]
    fn zero_memory_share_keeps_every_statement_out_of_the_prompt() {
        let (engine, repo) = memory_repo("share-zero", |config| {
            config.context_budget.set_share(ContextCategory::Memory, 0);
        });
        let entry = user_confirmed("mem_docker", MEMORY_STATEMENT);
        let limit = engine.config.context_token_budget();
        let plan = assemble(&engine, &repo, std::slice::from_ref(&entry), &[], limit);

        assert!(
            plan.items
                .iter()
                .all(|item| item.category != ContextCategory::Memory),
            "{:?}",
            plan.items
        );
        let prompt = rendered(&plan);
        assert!(
            !prompt.contains("Integration tests require Docker"),
            "{prompt}"
        );
        assert!(
            !prompt.contains("Remembered about this project"),
            "{prompt}"
        );
        assert!(plan.manifest.memory.is_empty());
        assert!(
            plan.manifest.excluded.iter().any(|record| {
                record.category == ContextCategory::Memory
                    && record.reason == ExclusionReason::CategoryBudgetExhausted
            }),
            "the drop must be recorded: {:?}",
            plan.manifest.excluded
        );

        fs::remove_dir_all(repo).unwrap();
    }

    #[test]
    fn memory_is_dropped_before_file_content_under_pressure() {
        // The acceptance criterion is the enum ordering, which Task 1 landed:
        // memory is the last budgeted category, so released carry reaches it
        // only after every other category has had its allocation.
        assert_eq!(
            ContextCategory::BUDGETED.last().copied(),
            Some(ContextCategory::Memory)
        );

        // A direct budget pass with every earlier category exactly at its
        // share, so nothing is released and the contest is real. The earlier
        // categories keep their items; memory cannot fit its own share and is
        // dropped. A whole engine fixture cannot express this as crisply,
        // because a file that under-uses its share releases the remainder.
        let (engine, repo) = memory_repo("pressure", |config| {
            for category in ContextCategory::BUDGETED {
                config.context_budget.set_share(category, 0);
            }
            config
                .context_budget
                .set_share(ContextCategory::Instructions, 1_000);
            config
                .context_budget
                .set_share(ContextCategory::FileRanges, 2_500);
            config
                .context_budget
                .set_share(ContextCategory::Memory, 300);
        });
        let item = |category, kind, tokens| {
            PendingItem::Item(ContextItem {
                category,
                kind,
                path: None,
                range: None,
                content: "x".repeat(tokens * 4),
                tokens,
                redaction_status: "clean".to_string(),
                provenance: None,
                root_path: None,
                origin: None,
            })
        };
        let plan = engine
            .context_manager
            .apply_budget(
                "repo_memory",
                "task_memory",
                vec![
                    item(
                        ContextCategory::Instructions,
                        ContextKind::AgentInstruction,
                        100,
                    ),
                    item(ContextCategory::FileRanges, ContextKind::ExplicitFile, 250),
                    item(ContextCategory::Memory, ContextKind::Memory, 40),
                ],
                EffectiveLimit::declared(1_000),
                Vec::new(),
                None,
                vec![(
                    2,
                    MemoryRecallRef {
                        entry_id: "mem_docker".to_string(),
                        match_reason: MemoryMatchReason {
                            matched_terms: vec!["docker".to_string()],
                            category_boost: false,
                        },
                        verification: Verification::UserConfirmed,
                    },
                )],
            )
            .expect("the budget pass should succeed");

        let categories = plan
            .items
            .iter()
            .map(|item| item.category)
            .collect::<Vec<_>>();
        assert!(
            categories.contains(&ContextCategory::Instructions),
            "{categories:?}"
        );
        assert!(
            categories.contains(&ContextCategory::FileRanges),
            "{categories:?}"
        );
        assert!(
            !categories.contains(&ContextCategory::Memory),
            "{categories:?}"
        );
        assert!(plan.manifest.memory.is_empty());
        let memory = plan
            .manifest
            .categories
            .iter()
            .find(|usage| usage.category == ContextCategory::Memory)
            .expect("the memory category is always reported");
        assert_eq!(memory.items, 0);
        assert!(memory.truncated);
        assert!(
            plan.manifest.excluded.iter().any(|record| {
                record.category == ContextCategory::Memory
                    && record.reason == ExclusionReason::CategoryBudgetExhausted
            }),
            "{:?}",
            plan.manifest.excluded
        );

        fs::remove_dir_all(repo).unwrap();
    }

    #[test]
    fn a_stale_entry_renders_the_out_of_date_marker_in_the_prompt() {
        let (engine, repo) = memory_repo("stale", |_| {});
        let provenance = || MemoryProvenance::RepositoryEvidence {
            paths: vec![EvidenceRef::new("docker-compose.yml", "sha256:abc")],
        };
        let stale = recall(
            "mem_stale",
            "The Postgres service listens on port 5433.",
            Verification::EvidenceStale,
            provenance(),
        );
        let current = recall(
            "mem_current",
            SECOND_STATEMENT,
            Verification::EvidenceCurrent,
            provenance(),
        );
        let limit = engine.config.context_token_budget();
        let plan = assemble(&engine, &repo, &[stale, current], &[], limit);

        let prompt = rendered(&plan);
        assert!(
            prompt.contains(
                "derived from docker-compose.yml, recorded 2026-03-14, \
                 may be out of date — that file has changed since"
            ),
            "{prompt}"
        );
        assert!(
            prompt.contains(
                "derived from docker-compose.yml, recorded 2026-03-14, evidence unchanged"
            ),
            "a current entry states why it is current: {prompt}"
        );
        let verifications = plan
            .manifest
            .memory
            .iter()
            .map(|reference| reference.verification)
            .collect::<Vec<_>>();
        assert_eq!(
            verifications,
            vec![Verification::EvidenceStale, Verification::EvidenceCurrent]
        );

        fs::remove_dir_all(repo).unwrap();
    }

    #[test]
    fn a_statement_with_a_credential_is_redacted_on_the_way_into_the_item() {
        let (engine, repo) = memory_repo("redaction", |_| {});
        let raw = user_confirmed("mem_raw", "Use AKIAIOSFODNN7EXAMPLE for the test bucket.");
        let limit = engine.config.context_token_budget();
        let plan = assemble(&engine, &repo, std::slice::from_ref(&raw), &[], limit);
        let item = plan
            .items
            .iter()
            .find(|item| item.category == ContextCategory::Memory)
            .expect("the entry should be included");
        assert_eq!(item.redaction_status, "redacted");
        assert!(
            !item.content.contains("AKIAIOSFODNN7EXAMPLE"),
            "the raw credential reached the prompt: {}",
            item.content
        );

        // Recall has already replaced the secret and reports the fact; the
        // second scan only sees the marker, so the flag is what keeps the item
        // from claiming to be clean (context.md C13.2).
        let mut cleaned =
            user_confirmed("mem_cleaned", "Use the redacted value for the test bucket.");
        cleaned.redacted = true;
        let plan = assemble(&engine, &repo, std::slice::from_ref(&cleaned), &[], limit);
        let item = plan
            .items
            .iter()
            .find(|item| item.category == ContextCategory::Memory)
            .expect("the entry should be included");
        assert_eq!(item.redaction_status, "redacted");

        fs::remove_dir_all(repo).unwrap();
    }

    #[test]
    fn a_memory_item_is_removable_like_any_optional_item() {
        let (engine, repo) = memory_repo("removal", |_| {});
        let first = user_confirmed("mem_first", MEMORY_STATEMENT);
        let second = user_confirmed("mem_second", SECOND_STATEMENT);
        let limit = engine.config.context_token_budget();
        let plan = assemble(&engine, &repo, &[first, second], &[], limit);
        let mut prepared = PreparedContext::from_plan(&plan);
        assert_eq!(prepared.manifest.memory.len(), 2);

        let index = prepared
            .items
            .iter()
            .position(|item| item.category == ContextCategory::Memory)
            .expect("two memory items were assembled");
        let record = prepared
            .remove_optional(index)
            .expect("memory is optional, like every non-Required category");
        assert_eq!(record.reason, ExclusionReason::UserRemoved);
        assert_eq!(record.category, ContextCategory::Memory);

        // The ref for the removed entry goes with it; the other entry keeps its
        // own row, so the inspector still names what was sent (C14).
        assert_eq!(prepared.manifest.memory.len(), 1);
        assert_eq!(prepared.manifest.memory[0].entry_id, "mem_second");
        assert_eq!(
            prepared
                .manifest
                .included
                .iter()
                .filter(|row| row.category == ContextCategory::Memory)
                .count(),
            1
        );
        let prompt = prepared
            .sections()
            .into_iter()
            .map(|section| section.text)
            .collect::<String>();
        assert!(
            !prompt.contains("Integration tests require Docker"),
            "{prompt}"
        );
        assert!(prompt.contains(SECOND_STATEMENT), "{prompt}");

        fs::remove_dir_all(repo).unwrap();
    }

    #[test]
    fn a_send_time_drop_removes_the_memory_item_and_its_ref() {
        let (engine, repo) = memory_repo("send-drop", |_| {});
        let first = user_confirmed("mem_first", MEMORY_STATEMENT);
        let second = user_confirmed("mem_second", SECOND_STATEMENT);
        let limit = engine.config.context_token_budget();
        let plan = assemble(&engine, &repo, &[first, second], &[], limit);
        let mut prepared = PreparedContext::from_plan(&plan);

        let record = prepared
            .remove_memory(
                "mem_first",
                "memory entry mem_first was deleted; it was not sent".to_string(),
            )
            .expect("the entry has a manifest row")
            .expect("the row names an item");
        assert_eq!(record.reason, ExclusionReason::MemoryNoLongerActive);
        assert_eq!(record.category, ContextCategory::Memory);
        assert!(record.path.is_none(), "memory has no file identity");

        // The ref goes with the item, and the row for the entry that stayed is
        // still the right one (C14).
        assert_eq!(prepared.manifest.memory.len(), 1);
        assert_eq!(prepared.manifest.memory[0].entry_id, "mem_second");
        let prompt = prepared
            .sections()
            .into_iter()
            .map(|section| section.text)
            .collect::<String>();
        assert!(!prompt.contains(MEMORY_STATEMENT), "{prompt}");
        assert!(prompt.contains(SECOND_STATEMENT), "{prompt}");
        assert!(
            prepared
                .manifest
                .included
                .iter()
                .filter(|row| row.category == ContextCategory::Memory)
                .count()
                == 1
        );

        // A second rejection, or one for an entry the budget never included,
        // has nothing to remove.
        assert!(
            prepared
                .remove_memory("mem_first", "again".to_string())
                .expect("a missing row is not an error")
                .is_none()
        );

        fs::remove_dir_all(repo).unwrap();
    }
}
