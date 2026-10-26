# Feature Spec: Context Assembly

Status: Done 2026-10-25. Split into a folder and planned on 2026-10-24, checked
against `main` that day, and built in seven tasks on 2026-10-24–25.
**Acceptance:** the deterministic tier passes all 23 scenarios with completion
1.000, check pass 0.7241 (21/29 scenario check commands), 0 approval-policy
violations, 0 restricted-path/secret violations, 0 unrelated files changed and
recovery 1.000. Compared by hand against `evals/baseline.json` (§7), the moved
aggregates are explained by the tier growing from 16 to 23 scenarios rather than
by a behaviour change; against the pre-implementation tree the same tier leaves
every count and rate identical and the whole-request estimate falls from 399391
to 397245 tokens, lower in every scenario. Scoped tests and clippy passed
locally, and the full seven-check gate passed in CI on 2026-10-25 (Quality run
40204226481, on `main` at `8758f62`). The estimate is calibrated against
provider-reported measured usage from one live run against
`deepseek`/`deepseek-v4-flash` (§7); the offline half over recorded traffic was
not run. Known open points — no collection source yields line ranges, so range
deduplication is implemented but inert in production, and no Observed
capability profile exists, so every limit is Declared — are recorded in §7 and
do not block acceptance. The design holds; its file and line references did not,
and three of its statements about the current code were wrong (there is already
path-level deduplication, `LineRange` already exists, and the user prompt is sent
twice). Planning also found a security hole the spec now closes: the user's
prompt is not redacted on any path to the provider, while the duplicate copy of
it is. [`context.md`](context.md) records the pre-implementation code, that
finding, and what other specs already rely on here; [`tasks.md`](tasks.md)
carries the corrections table, the task breakdown, and the questions this
planning round could not answer from the code alone.
Order: 26 of 27
Plan: `docs/PLAN/03_phase_3_code_understanding.md`, Phase 3, Work
Package 4 (Must). That directory is local-only and not committed, so the
reference is a name rather than a link; this spec is self-contained.
Depends on: [#21](../21_task_plan_progress_and_budget/proposal.md) (plan state) —
built; [#22](../22_findings_model_and_panel/proposal.md) (findings) — built;
[#24](../24_repository_map_and_monorepo_boundaries/proposal.md) (the repository map) —
built (Done 2026-10-09); [#25](../25_symbol_and_relationship_index/proposal.md) (the symbol index) —
built (Done 2026-10-23). Everything else named below is a cross-reference, not a
prerequisite.
Related spec sections: `ai_coding_assistant_specification.md` section 7.3 (path
and secret policy), section 7.5 (model adapter), section 7.9 (context assembly).
Related implementation specs:
[`../02_semantic_search.md`](../02_semantic_search.md),
[`../11_agents_md_support.md`](../11_agents_md_support.md) (instruction resolution and
precedence), [`../21_task_plan_progress_and_budget/proposal.md`](../21_task_plan_progress_and_budget/proposal.md)
(plan state as a context category),
[`../22_findings_model_and_panel/proposal.md`](../22_findings_model_and_panel/proposal.md) (findings as a
category), [`../24_repository_map_and_monorepo_boundaries/`](../24_repository_map_and_monorepo_boundaries/proposal.md),
[`../25_symbol_and_relationship_index/`](../25_symbol_and_relationship_index/proposal.md),
[`../27_context_inspector.md`](../27_context_inspector/proposal.md) (renders the manifest this
spec produces), [`../49_prompt_cache_accounting_and_reuse/proposal.md`](../49_prompt_cache_accounting_and_reuse/proposal.md)
(the section order this spec fixes is what its reuse slice needs),
[`../55_conversation_compaction/proposal.md`](../55_conversation_compaction/proposal.md) (the
conversation-summary category it will fill).

## 1. Motivation

Context assembly is a single budget consumed first-come, first-served.

`ContextManager::build_context` (`crates/workspace-engine/src/context_manager.rs:96`)
takes one `token_budget` and fills it by calling `add_text` in a fixed sequence.
`add_text` (`context_manager.rs:410`) estimates a chunk at
`len.div_ceil(4)` (`context_manager.rs:423`), and if it would exceed the budget it
**returns false and the content is dropped**. Ordering is therefore priority,
implicitly and invisibly: whatever is added first wins, and whatever runs into
the ceiling vanishes with no record that it existed.

Three consequences:

- **Any one category can starve every later one.** A prompt mentioning a large
  file can consume the budget on that file, leaving nothing for instructions,
  search results, or symbols. Nothing bounds a category's share.
- **Nothing is deduplicated at the level that matters.** Whole files are already
  deduplicated — `add_file` skips a path it has already added
  (`context_manager.rs:378`) — but a file can only ever be included whole, so
  "the two functions two search hits name" cannot be sent as their own ranges,
  and a range cannot absorb or be absorbed by a whole file. Deduplication only
  becomes the interesting problem once ranges exist.
- **Nothing is recorded.** `ContextPlan` (`context_manager.rs:34-41`) carries
  items and a token estimate, but no provenance and no account of what was
  excluded or why. The user cannot see what was sent, and neither can a
  developer debugging a bad answer. That is the gap
  [spec 27](../27_context_inspector/proposal.md) exists to close, and it needs this spec to
  produce the data.

There are also three structural gaps that block the roadmap's requirements and
two of the later specs:

- `ContextItem` has a `path` but **no line range** (`context_manager.rs:25-32`),
  so content is whole-file. "Deduplicate overlapping file ranges" and "include
  path and line provenance with every included range" require ranges to exist
  first.
- `kind` is a free string (`context_manager.rs:27`) that conflates two different
  questions — which budget the item belongs to, and what source produced it —
  so neither can be changed without changing the other.
- The prompt is assembled into one string with its sections in
  volatile-to-stable order, and the user's request appears in it **twice**:
  once as the `user_prompt` item (`context_manager.rs:139-147`) and again as
  the `User request:` section (`chat.rs:6126-6127`). §5.10 fixes both.

## 2. Current State

Checked against the tree on 2026-10-24. `context.md` §3 has the full file:line
map, including the call sites this section only names.

- **One flat budget.** `build_context` (`context_manager.rs:96-116`) delegates to
  `build_context_with_hooks` (`:124-312`) with one `token_budget`. Every
  `add_text` and `add_file` call takes that same number.
- **Drop-on-overflow, silently.** `add_text` (`:410-440`) returns `false` when
  `*token_estimate + tokens > token_budget` (`:424-426`); the caller does not
  record the omission, and `add_file` (`:364-407`) only learns `added == false`.
- **`ContextItem` is whole-file and range-less**:
  `{ kind: String, path: Option<String>, content: String, tokens: usize,
  redaction_status: String }` (`:25-32`).
- **`ContextPlan`** is `{ repository_id, task_id, token_estimate, items, files }`
  (`:34-41`). No manifest, no exclusion record, no per-category totals. It is
  consumed only inside the workspace engine — `chat.rs:1039-1059` and
  `edit.rs:509-543` — and `files` is also read by the eval harness through
  `ChatTurnResult.context_files`.
- **Redaction happens per item, at the right place.** `add_text` runs
  `scanner.redact(content)` before measuring or storing (`:422`), and
  `redaction_status` is recorded per item. This is the pattern to preserve.
- **The estimate is `len.div_ceil(4)`** (`:423`), also used by
  `ModelAdapter::estimate_tokens` (`crates/workspace-engine/src/model.rs:354`).
  Spec 19 measured it 2.5–6.3% high against DeepSeek
  ([spec 19](../19_token_and_cost_accounting/proposal.md) §7).
- **Instruction resolution exists.** `agent_instruction_paths`
  (`context_manager.rs:442-470`) walks ancestors for `AGENTS.md`, root first,
  per [spec 11](../11_agents_md_support.md). Items enter context in the order
  explicit → mentioned → instructions → project rules → map → symbols → hook
  context → search results (`:184-303`).
- **`context_token_budget` is resolved per provider and model**
  (`crates/workspace-engine/src/config.rs:1925-1934`), documented as **a packing
  budget, not the provider's window** (`:1920-1924`; defaults at `:3843-3848`,
  where a model with a 1M-token window is granted 64k because context is re-sent
  and re-billed every turn). §5.7 corrects the margin accordingly.
- **`LineRange` already exists** — `crate::file_access::LineRange`
  (`file_access.rs:16`, `usize` start/end, 1-based inclusive), re-exported at
  `lib.rs:103` and taken by `ReadWindow::Range`. §5.1 reuses it rather than
  defining a second range type.
- **No collection source can name a line range yet** (checked 2026-10-25 for
  Task 3). `indexer::SearchResult` (`indexer.rs:49-56`) carries the path and a
  snippet but drops the chunk offsets it was derived from, and
  `symbol_index::Symbol` (`symbol_index.rs:85-99`) carries one starting line and
  is collected as one rendered block, not as file ranges. Every `add_file` call
  therefore asks for the whole file. §5.4's merge is implemented ahead of its
  first producer and is unit-tested; the note in §5.4 records the seam.
- **`ContextManager` holds no `PathPolicy` and no `AuditLog`**
  (`context_manager.rs:43-51`). The policy is inside `FileAccessController`
  (private) and in `SymbolSource`; every read currently goes through
  `read_file`, which applies path policy and redaction at **read** time
  (`file_access.rs:125-129`, `:219`). Nothing checks at **collection** time, so
  a restricted path is discovered only after it has been selected by search or
  named in the prompt.
- **The index does not exclude restricted files.** `RepositoryIndex::files`
  (`indexer.rs:59-65`) is built without a path policy, and
  `keyword_search`/`semantic_search` (`:68`, `:99`) return them. The map and the
  symbol index *do* filter (`repository_map.rs:1046-1047`,
  `symbol_index.rs:295`).
- **Two prompt renderers.** `build_model_prompt` (`chat.rs:6101-6150`) and
  `build_edit_prompt` (`edit.rs:1086-1105`) each format `ContextItem`s into a
  `--- kind: path ---` header independently. Only the chat one is called in
  production with prior messages; `build_model_prompt`'s `command_context`
  argument is `None` at its only production call (`chat.rs:1040`) and is
  exercised only by a unit test.
- **No observed capability profile exists.** Phase 1 WP3 is Should-tier and not
  in Phase 1's minimum slice, so the observed-limit path this spec's requirement
  7 references may not exist. §5.7 handles that.

## 3. Requirements

1. A configurable token budget is allocated **per category**, not globally.
2. Overlapping file ranges are deduplicated. Two search hits in the same
   function are not sent twice.
3. Exact evidence is preferred over semantic similarity when both are available
   and the budget is contested.
4. Path and line provenance accompanies every included range.
5. Restricted and secret-bearing content is excluded at every stage, not only at
   the end.
6. A sanitized context manifest is recorded for debugging and audit.
7. Assembly adapts to the provider's real limits using an observed capability
   profile when one exists; otherwise it falls back to `context_token_budget`
   plus a documented safety margin, labelled declared rather than observed.
8. The token estimate is improved. `len / 4` is adequate for a rough budget and
   inadequate for deciding what to drop at the boundary.

## 4. Non-goals

- Choosing *what* is relevant. Retrieval quality is search
  ([spec 02](../02_semantic_search.md)) and symbols
  ([spec 25](../25_symbol_and_relationship_index/proposal.md)); this spec
  allocates budget among what they return and records what it did.
- Conversation compaction — Phase 3 WP6, Should-tier
  ([spec 55](../55_conversation_compaction/proposal.md)). The pipeline reserves a
  category for a conversation summary and tolerates its absence.
- Model routing — Phase 3 WP7.
- A tokenizer per model family. §5.8 improves the estimate without adding a
  tokenizer dependency, and is explicit that the result is still an estimate.
- Persisting assembled context beyond the manifest.
- The inspector UI — [spec 27](../27_context_inspector/proposal.md).
- Prompt-cache breakpoints. This spec fixes the section order that
  [spec 49](../49_prompt_cache_accounting_and_reuse/proposal.md)'s reuse slice
  needs and makes the sections separately addressable; it emits no breakpoints
  and does not change `emit_cache_breakpoints`.
- Changing `SecretScanner` or `path_policy.rs`. Both are used exactly as they
  are; `file_access.rs` gains one `ReadWindow` variant (§5.4).
- Changing the command policy, the tool list, or the round budgets. Every task
  must leave `cargo nextest run -p workspace-engine --test permission_profiles`
  and the command-policy unit tests green without editing them.
- **Per-root project rules.** `PROJECT_RULES` names repository-root files only
  (`context_manager.rs:13-23`), so a monorepo package's own manifest and README
  never enter context. The local planning notes assign that gap to this spec, but
  it is a retrieval change rather than a budget change: #26 gives the existing
  root-level rules the `Instructions` budget and leaves `PROJECT_RULES` as it is.
  A root-aware rule set is a follow-up with its own evidence.

## 5. Design

### 5.1 Categories, ranges, and two fields where there was one

Requirements 2 and 4 need a range, and per-category budgets need a category, so
`ContextItem` gains both. `kind` **stays** as a separate, small enum: it is the
source label the model already reads in the prompt header
(`--- retrieved_file: src/lib.rs ---`), and four test files assert those exact
strings (`foundation.rs:4354`, `repository_map.rs:1855-1858`,
`symbol_index.rs:3377-3379`, `hooks.rs:3019`). Merging the source label into the
budget bucket would change the prompt for every turn and lose the distinction
between "you asked for this file" and "search found it".

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContextCategory {
    /// Never dropped. The user's own message.
    Required,
    Instructions,   // AGENTS.md hierarchy and root project rules, per spec 11
    HookContext,    // spec 32: a rule or note the user installed
    Pinned,         // user-selected, per spec 27 (reserved; see §5.11)
    PlanState,      // spec 21
    RepositoryMap,  // spec 24
    Symbols,        // spec 25
    SearchResults,
    FileRanges,
    Findings,       // spec 22 (reserved; see §5.11)
    ConversationSummary, // spec 55 (reserved)
}

/// The source label the prompt header shows. The spelling is today's, so a
/// range-less item renders byte-identically to the current prompt.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContextItem {
    pub category: ContextCategory,
    pub kind: ContextKind,
    pub path: Option<String>,
    /// `None` means the whole file, or content with no file identity.
    /// Serialised as `lineRange`.
    #[serde(rename = "lineRange")]
    pub range: Option<LineRange>,
    pub content: String,
    pub tokens: usize,
    pub redaction_status: String,
    /// The lowest provenance among what this item contains. Carried from
    /// spec 25; `None` for content that has no symbol provenance at all.
    pub provenance: Option<Provenance>,
    /// Root that owns the path, from spec 24. Requirement 6 of spec 24.
    pub root_path: Option<String>,
}
```

`LineRange` is `crate::file_access::LineRange` (`file_access.rs:16`), already
1-based inclusive and already re-exported; it gains `Serialize`/`Deserialize` for
the manifest, which it has no derives for today. Every `ContextKind` maps to
exactly one category, and the mapping is the one place that decision lives:

| `ContextKind` | `ContextCategory` |
|---|---|
| `UserPrompt` | `Required` |
| `AgentInstruction`, `ProjectRule` | `Instructions` |
| `HookContext` | `HookContext` |
| `Pinned` | `Pinned` |
| `PlanState` | `PlanState` |
| `RepositoryMap` | `RepositoryMap` |
| `Symbols` | `Symbols` |
| `RetrievedFile` | `SearchResults` |
| `ExplicitFile` | `FileRanges` |

A search hit is `SearchResults`, not `FileRanges` (C18): §5.3 orders exact keyword
hits before semantic ones and drops semantic hits first when that category is
over budget, which `FileRanges` — the explicitly requested and mentioned paths —
does not describe. The header still prints `retrieved_file` for a search hit and
`explicit_file` for an explicit one, so splitting the category changes no prompt.

`HookContext` is its own category rather than sharing `Pinned`: spec 32's Task 10
requires `hook_context` to survive as a category, and giving it a share is what
replaces today's implicit "after symbols, before retrieved files" placement with
an explicit priority. It sits ahead of the map and symbols because a rule the
user installed must not be crowded out by a large repository layout; nothing in
the tree pins the old position (only the item's presence is asserted). The
declaration order of
`ContextCategory` is the pipeline order from the roadmap, and `Ord` is derived so
priority is the enum's own ordering rather than a separate table that could
disagree with it.

`Required` is first and is the category [spec 27](../27_context_inspector/proposal.md)
forbids the user from removing. Naming it in the type is what makes that
guarantee checkable rather than a UI convention. Today it holds exactly one
thing: the user's message.

### 5.2 Per-category budgets

```text
context_budget.instructions=0.10
context_budget.hook_context=0.05
context_budget.pinned=0.15
context_budget.plan_state=0.05
context_budget.repository_map=0.05
context_budget.symbols=0.10
context_budget.search_results=0.15
context_budget.file_ranges=0.25
context_budget.findings=0.05
context_budget.conversation_summary=0.05
context_budget.safety_margin=0.10
```

Fractions of the effective limit (§5.7) rather than absolute counts, so one set
of settings works across a 64k and a 200k budget. They are parsed as decimals and
**stored and allocated as integer basis points** (`u16`, `0..=10_000`): a float
would make the last token's allocation depend on rounding, and it would cost
`ContextItem`/`ContextPlan` their `Eq`, which tests rely on. The sum must be
`<= 10_000`; a set that oversubscribes is a config error naming the key, not a
silent scale-down.

`Required` has no budget: it is subtracted from the total before categories are
allocated, because a required item that does not fit is not a budgeting problem
but an error the user must see.

Two rules make the allocation useful rather than merely tidy:

- **Unused allocation is redistributed.** A category that uses less than its
  share releases the remainder to later categories in pipeline order. Without
  this, a repository with no findings, no pinned context and no conversation
  summary wastes up to 30% of the window on nothing — strict per-category caps
  would make the common case worse than today's flat budget.
- **A category over its share is truncated within itself**, dropping its
  lowest-priority members (§5.3) rather than stealing from another category. A
  block item (map, symbols, plan state) is first re-rendered at the smaller
  ceiling, because `render_for_model` already takes one; only a block that cannot
  render a single line is excluded.

Overflow is recorded, never silent. This is the specific defect in
`add_text`'s `return false` (`context_manager.rs:424-426`): every dropped or
truncated item produces an exclusion record (§5.5).

The config keys are a new `context_budget.*` family. Their scope classification,
and why repository config may not raise a share, is in
[`tasks.md`](tasks.md)'s Global Constraints and Task 4.

### 5.3 Ordering within a category, and exact over semantic

Requirement 3 is a rule about `SearchResults` versus everything else, and about
ordering inside `SearchResults`:

| Within | Order |
|---|---|
| `Required` | As given; one item |
| `Instructions` | Precedence order for the prompt: root `AGENTS.md` first, then each nested file, as today, then the root project rules. **Truncation priority is the reverse** for the instruction files — the most specific file survives, because nested instructions override broader ones (the system prompt says so). |
| `HookContext` | Event order: `session_start` before `before_context` |
| `Pinned` | As the user pinned them |
| `PlanState` | Step order; the current step first |
| `RepositoryMap` / `Symbols` | As their renderers produce them (spec 25 already sorts symbols by provenance then match quality, `symbol_index.rs:519-535`) |
| `SearchResults` | Exact keyword matches, then symbol-name matches, then semantic-similarity matches |
| `FileRanges` | Explicitly requested paths, then paths mentioned in the prompt, then ranges implied by findings |
| `Findings` | Severity, then newest |
| `ConversationSummary` | A single item |

"Prefer exact evidence over semantic similarity" is therefore not a global
tie-break but an ordering inside the contested category. Semantic hits are the
first thing dropped when `SearchResults` is over budget, which is the correct
trade: an exact match is evidence, a similarity score is a suggestion. Today the
two lists are concatenated before any of this is visible — `keyword_search`
results are extended with `semantic_search` and both become `retrieved_file`
paths (`context_manager.rs:164-182`) — so the source has to be carried per
result before this rule can exist.

Assembly therefore keeps the two searches as separate lists, adds keyword hits
first, and deduplicates by path so a path found by both becomes one item. Each
result carries its `CollectionSource` (`KeywordHit` or `SemanticHit`) through the
collection-time check, so a restricted path is refused on either route and
recorded once. Both use the `retrieved_file` header and the `SearchResults`
category; only the order differs. The eight former parameters travel in one
`ContextRequest` struct (C12), which also removes `build_context_with_hooks`:
hook context is one more field rather than a second entry point.

### 5.4 Range deduplication

Requirement 2, applied after collection and before budgeting:

1. Group items by `path`.
2. Sort ranges by `start`.
3. Merge ranges that overlap or are separated by at most a small gutter of
   lines — two hits a couple of lines apart are better sent as one range than as
   two with a duplicated header. The gutter is `GUTTER = 2`, and the rule is
   `next.start <= current.end + 1 + GUTTER`: a two-line gap merges, an eight-line
   gap does not. §7 records whether one value covered both search-hit clusters and
   instruction ranges.
4. Re-read the merged range once, as one item.

A whole-file item (`range: None`) absorbs every range for that path: if the whole
file is included, a range within it is redundant. This is the case most likely to
be missed, because the two come from different collection paths — a prompt
mention producing the whole file and a search hit producing a range.

Merging preserves the highest-priority `provenance` among the merged items
(`Provenance::LanguageServer` sorts before `Heuristic`), so a range that came
partly from a language-server symbol is not downgraded by being merged with a
heuristic one.

**Producers today: none** (checked 2026-10-25). No collection source yields a
`LineRange`: `indexer::SearchResult` has a path and a snippet, and
`symbol_index::Symbol` has one starting line and is collected as a block (§2).
Task 3 therefore implements the rules as `merge_file_ranges` plus
`flush_file_ranges`, with every `add_file` caller passing `range: None`; the
whole-file absorption has no producer either. The merge is covered by unit tests
of the function, and `no_collection_source_yields_a_line_range_yet`
(`tests/context_assembly.rs`) fails the day a source starts producing ranges, at
which point the merge must be covered through `build_context` as well.

**The re-read is a new `ReadWindow` variant.** `ReadWindow::Range` clamps to
`max_read_lines` (400 today) at `file_access.rs:184-189`, deliberately, so a
model cannot step around a read cap by asking for a huge range. A merged range
must not be silently cut at 400 lines. So `file_access.rs` gains
`ReadWindow::ContextRange(LineRange)`: an explicit range with **no line cap**,
because the assembler applies its own token budget, while the byte cap, the path
policy and the redaction still apply exactly as for `Range`. `Whole` already
opts out of the line cap for the same reason, and its doc comment says so.

Slicing a `Whole` read in memory instead was rejected: `FileRead.content` is
already redacted, and redaction can change line structure (a multi-line secret
collapses), so line numbers computed from the raw file would drift.

### 5.5 The manifest

```rust
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContextManifest {
    pub repository_id: String,
    pub task_id: String,
    pub assembled_at_ms: u128,
    /// Effective limit and where it came from. Requirement 7.
    pub limit: EffectiveLimit,
    pub total_tokens: usize,
    /// Per category: allocated (after redistribution), used, item count, and
    /// whether it truncated or borrowed.
    pub categories: Vec<CategoryUsage>,
    /// Every item, by path and range — never content.
    pub included: Vec<IncludedRef>,
    /// Everything left out, or included only in part, with a reason.
    /// Requirement 6's real value.
    pub excluded: Vec<ExclusionRecord>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExclusionReason {
    RestrictedPath,
    SecretBearing,
    CategoryBudgetExhausted,
    TotalBudgetExhausted,
    DeduplicatedIntoRange,
    Truncated,
    TooLarge,
    UserRemoved,
    Binary,
}
```

`Truncated` is an addition to the original list: acceptance requires every
*dropped or truncated* item to produce a record, and "included, but only its
first 40 lines" is not `TooLarge`.

The manifest holds **paths, ranges, counts, and reasons — never content**. That
is what makes it safe to write to the audit log and safe to show in a
diagnostics view, and it satisfies requirement 6's "sanitized" by construction
rather than by a redaction pass. Content lives in the assembled context, which
is sent to the provider and not persisted.

`excluded` is the part worth insisting on. A user asking "why didn't it look at
`upload.rs`?" currently has no answer; with this, the answer is
`RestrictedPath` or `CategoryBudgetExhausted` or `Truncated`.

The manifest is returned inside `ContextPlan` and recorded **by the caller**
through `AuditLog::record` (`crates/workspace-engine/src/audit.rs:42`), which
redacts field values on the way in — a second safety net over a structure that
should contain nothing to redact. The caller records it because
`ContextManager` has no audit log (`context_manager.rs:43-51`) and the two
orchestrators already do; adding one to the manager would be a second audit path
for one event. One `context_assembled` event per assembly, with the manifest as
one JSON field beside `repositoryId`, `taskId` and `tokenEstimate`, next to the
existing `model_request_prepared` (`chat.rs:2927-2944`).

In #26 the enum carries the reasons the pipeline can actually produce:
`RestrictedPath`, `DeduplicatedIntoRange`, `CategoryBudgetExhausted`, `Truncated`,
`TooLarge` and `Binary`. `SecretBearing`, `TotalBudgetExhausted` and
`UserRemoved` arrive with the stages that produce them (the total-budget path,
spec 27's removal), so no record can claim a reason nothing generates.

### 5.6 Exclusion at every stage

Requirement 5 says restricted and secret-bearing content is excluded at every
stage, not only at the end. Concretely, `path_policy.rs` is applied at
**collection** time in each source — explicit paths, prompt mentions, keyword and
semantic hits, instruction paths, project rules, and the sources §5.11 reserves
for a later spec — and again at read time, and `SecretScanner` runs before an item
is measured or stored, as `add_text` already does (`context_manager.rs:422`).

Checking twice is deliberate. A single check at the end is one edit away from
being bypassed by a new collection path, and the acceptance criterion is written
to assert exclusion **at every stage** so a new source cannot be added without a
test forcing it through the check.

The check is `ContextManager::collection_exclusion`, and its input is one of the
ten `CollectionSource` variants. The match has **no wildcard arm**, so a new
source does not compile until it says whether the policy applies, and
`every_collection_source_refuses_a_restricted_path` walks every variant. The
explicit, mention, keyword-hit, semantic-hit, instruction and project-rule
sources all call `PathPolicy::is_restricted`; the map and symbol stores already
filter their own inputs, and hook context and plan state carry session content
with no path to judge. The refused path becomes an `ExclusionRecord` with
`RestrictedPath`, carrying its category and path but never its content.

`ContextManager` does not hold a `PathPolicy` today, so it gains one, wired from
`workspace_engine.rs:88-117`. That is a plain field, not a second policy
implementation: the same `PathPolicy` instance the read path uses, passed to
`ContextManager::new` beside the `FileAccessController` built from it.

A restricted path never enters the manifest's `included` list, and appears in
`excluded` as `RestrictedPath` — which is itself the assertion that it was
considered and refused, rather than never noticed.

**Normalisation.** The collection check compares strings, while the read path
resolves first (`PathPolicy::resolve_existing`): it strips `./`, collapses `..`
and turns an absolute in-repo path into a repository-relative one before
`assert_not_restricted`. The assembler therefore normalises each candidate with
that same resolver before the collection check, and records the path the policy
matched. A read that still refuses for a policy reason becomes a
`RestrictedPath` record rather than a silent omission. A read that fails for a
reason the enum can name — a whole-file size refusal (`TooLarge`) or a binary
file (`Binary`) — is recorded too. A missing or unreadable file has no reason in
§5.5's list, so it stays unrecorded rather than carrying a fabricated one.

### 5.7 The effective limit

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EffectiveLimit {
    pub tokens: usize,
    pub source: LimitSource,   // Observed | Declared
    pub safety_margin_bps: u16,
    /// The provider window the margin was applied to, when one is known.
    pub window: Option<usize>,
}
```

Resolution order:

1. **Observed** — a capability profile from Phase 1 WP3, if one exists. The
   margin (`context_budget.safety_margin`, default 10%) is subtracted from the
   window, and the result is what categories divide.
2. **Declared** — `context_token_budget` for the provider
   (`config.rs:1925-1934`), used **unchanged**, with `safety_margin_bps = 0`.

Why the declared path carries no margin: `context_token_budget` is already a
packing budget rather than the provider's window (`config.rs:1920-1924`). The
built-in gives a 1M-token model 64k precisely because that is the number the
product wants to pack, not the number the provider accepts. Subtracting another
10% would shrink every existing user's context, and the margin exists to absorb
estimate error against a hard limit that, on this path, is 15× away. When an
observed window does exist, the margin applies to the real window and is recorded
with it. `tasks.md`'s open questions records this as the owner's call, with the
alternative (treat the config value as a window) as a one-line change.

Phase 1 WP3 is Should-tier and may not have shipped, so the declared path is the
realistic one and is treated as the primary case rather than a fallback.

`source` is recorded in the manifest and shown in the inspector, so a user
seeing conservative context can tell whether the limit was measured or assumed.

### 5.8 A better estimate, still an estimate

Requirement 8. `len.div_ceil(4)` (`context_manager.rs:423`) is a fixed
bytes-per-token ratio that is wrong in opposite directions for prose and for
code: dense code with many short tokens runs well over four bytes per token, and
long identifiers under.

Without adding a tokenizer dependency (§4), the improvement is a content-aware
ratio: estimate per item using a ratio selected by the item's language and
category — one for prose and Markdown, one for code, one for structured data like
JSON — calibrated once against measured provider usage from
[spec 19](../19_token_and_cost_accounting/proposal.md) and recorded in §7.

The calibration is the point. [Spec 19](../19_token_and_cost_accounting/proposal.md)
records measured input tokens per call; comparing those against the estimate for
the same call gives a real ratio per content type instead of a guessed one, and
turns `len / 4` from folklore into a number with a derivation. Spec 19 §7
already measured the *whole-request* ratio on DeepSeek at 2.5–6.3% high, which is
the sanity bound the new estimator has to beat; per-content-type ratios need a
live measurement, because only the request's total is reported. §7 records how
that measurement runs and what it is allowed to read.

**The measured ratios (Task 7, 2026-10-25).** Task 6 landed the estimator with
three ratios from published rules of thumb, labelled provisional in the code and
here. The live calibration in §7 replaced two of them with measured values. The
ratios are:

| content type | selected by | bytes / token | measured | derivation |
|---|---|---|---|---|
| prose and Markdown | `.md`, `.markdown`, `.mdx`, `.txt`, `.rst`, `.adoc`; prose-like kinds | 4 (4/1) | 4.50 | `len / 4` over-estimates prose by about 13%, consistent with spec 19 §7's 2.5–6.3% high, so the ratio is left alone. Classification never estimates prose below `len / 4`, and the headroom stands in for the margin the declared path does not subtract |
| source code | `.rs`, `.go`, `.py`, `.js`, `.ts`, `.java`, `.kt`, `.c`, `.h`, `.cpp`, `.cs`, `.rb`, `.php`, `.swift`, `.sh`, `.sql`; also a file with no recognised extension | 3.6 (18/5) | 3.61 | short identifiers and punctuation-heavy lines tokenize denser than prose. Over-counting is the safe direction |
| structured data | `.json`, `.jsonc`, `.jsonl`, `.yaml`, `.yml`, `.toml`, `.xml`, `.csv`, `.tsv`, `.html`, `.css`, `.scss`, `.lock`, `.ipynb` | 2.4 (12/5) | 2.45 | quoted keys, braces, commas and short scalar values put a token boundary at nearly every mark |

The measurement is one call per document against `deepseek` /
`deepseek-v4-flash`, and the provider's input count includes about 15 tokens of
instruction and chat-template overhead. A document-only ratio is therefore a
little higher than the reported one, so these figures over-count tokens slightly,
which is the safe direction. It is one sample per type from one provider; §7
records the table.

Every ratio is at or below four bytes per token, so no classified content is
estimated lower than `len / 4` was; the estimate can grow but never shrink, and
§5.7's margin absorbs the error where a real limit makes it matter. The estimate
is integer arithmetic — `ceil(len × denominator / numerator)` — so no float
rounding can decide which item fits and `prompt_cache`'s determinism guard is
unaffected. Content type comes from `kind` and the file extension only, never
from scanning the text, so two assemblies of an unchanged repository measure
identically.

Two paths stay prose on purpose. `Symbols` and `RepositoryMap` items are
path-less blocks whose own renderer (`render_for_model`) fits them to a ceiling
counted with `len / 4`; measuring the same text with a denser ratio would report
more tokens than the ceiling it was rendered to. Their ceilings move only when
their renderers share this estimator, which is outside Task 6's "change only
inside `add_text`" boundary and is recorded as a discovery rather than done
here.

It remains an estimate, is labelled as such in the manifest
(`CategoryUsage::estimated`), and the safety margin in §5.7 is what absorbs the
residual error. Where a provider reports usage, the measured figure supersedes
the estimate after the fact for accounting — but assembly must decide before the
call, so it decides on the estimate.

### 5.9 Documentation

`docs/USER_GUIDE.md` (in **Output and Context Limits**, `:249`): what goes into
context, in what order, how the per-category budgets work, and which keys change
them. `docs/TROUBLESHOOTING.md`: how to read a manifest from the audit log, what
each exclusion reason means, how to tell an observed limit from a declared one,
and what to do when content is being excluded as `CategoryBudgetExhausted`.

### 5.10 Where the assembled context reaches the model

Today `build_model_prompt` (`chat.rs:6101-6150`) writes one string: recent
conversation, then `User request:`, then `Repository context:` with every item.
Three changes are in scope here, because this spec owns the assembly and no other
spec may run beside it — #49's reuse slice, #55, #65 and #66 all edit this
function:

1. **One renderer.** The item header format moves out of both `chat.rs` and
   `edit.rs` into one function on `ContextPlan`. `build_model_prompt` and
   `build_edit_prompt` keep their own preambles and system prompts.
2. **Stable-to-volatile order.** The sections become `Repository context:` →
   `Recent conversation:` → `User request:`. The current order is the reverse of
   what a cacheable prefix needs ([spec 49](../49_prompt_cache_accounting_and_reuse/proposal.md)
   `context.md` §3.2; [spec 55](../55_conversation_compaction/proposal.md) §2), and both
   specs assign the decision to this one or to #49.
3. **The request is rendered once, redacted.** `Required` items render under the
   final `User request:` heading, and the `Repository context:` block holds only
   the other categories. Because `Required` items are produced by `add_text`,
   they are already redacted (C17); the raw `prompt` argument is never written.
   The heading already names the request, so its `--- user_prompt ---` item
   header is not repeated inside it: the section is the item's content. The
   one-shot edit keeps its own `User edit request:` preamble and its
   request-first order — it makes a single call, so there is no cache prefix to
   order — but it renders the same section from the same item.

The redaction half is not cosmetic. Today `build_model_prompt` writes the raw
`prompt` under `User request:` (`chat.rs:6126-6127`), and nothing redacts that
value on any path into the engine — shell, CLI or harness (`context.md` §2 has
the per-path evidence). A secret pasted into the prompt therefore leaves the
machine in cleartext, while the duplicate `user_prompt` item beside it is
correctly redacted. The same is true of the `Recent conversation:` block
(`chat.rs:6117-6124`): stored messages are verbatim by spec 16's design, and the
renderer replays them raw, so an earlier turn's secret is re-sent on every later
turn. Rendering the request from its item closes the first hole; passing each
prior message through the scanner closes the second. Redacting clean text is a
no-op, so a prompt without a secret renders byte-identically to today.

This is also why the request must not be rendered from the function's `prompt`
argument even as a fallback: a fallback would reintroduce the leak the moment the
`Required` item was ever absent.

The sections are named (`PromptSection`), so spec 49's reuse slice can put a
breakpoint at a section boundary without re-parsing the string. This spec emits
no breakpoints.

`prior_messages` keeps today's shape — the session's last eight messages,
truncated to 2,000 characters each (`chat.rs:6108-6125`) — because the recency
window is [spec 55](../55_conversation_compaction/proposal.md)'s to replace. This spec
only decides where that block sits.

### 5.11 Reserved categories

Three categories in §5.1 have no collection source yet, and this spec does not
invent one:

- `Pinned` needs session-scoped storage, which is [spec 27](../27_context_inspector/proposal.md).
- `ConversationSummary` needs [spec 55](../55_conversation_compaction/proposal.md) to
  produce a summary at all.
- `Findings` exists in the model (spec 22), but the agent sees only the subset
  the user selects for a repair request. Automatically sending every open finding
  on every turn is a product decision, not a wiring detail, and it would compete
  with the repair flow that already works. It stays reserved until that decision
  is made (`tasks.md`, open questions).

A reserved category still appears in the manifest with its allocation and zero
use, and its share is redistributed by §5.2 like any unused share. `PlanState` is
**not** reserved: a plan already persists and is carried between turns
(`session.rs:1554`, `chat.rs:2727-2738`), so this spec renders it as an item. The
caller reads the plan for the current task once before assembly — the turn
carries it via `carry_plan_from_the_previous_turn`, so a new turn has none — and
passes it in `ContextRequest::plan`. The item lists each step in order with its
status and detail; evidence stays on the panel and is not a prompt input. A plan
with no steps produces no item, because there is nothing to carry.

## 6. Acceptance Criteria

- Each category's contribution is bounded by its configured share and reported
  in the manifest with allocated, used, and item count.
- Unused allocation is redistributed to later categories, and a category over
  its share truncates within itself rather than reducing another category.
- Overlapping ranges in the same file appear once, merged, and a whole-file item
  absorbs ranges within that file.
- A merged range keeps the highest-priority provenance among its parts.
- Every included range carries path and line provenance, and its owning root.
- Semantic-similarity results are dropped before exact keyword matches when
  `SearchResults` is over budget.
- A restricted path is excluded at collection time **and** at read time, and
  appears in the manifest's `excluded` as `RestrictedPath` — asserted at every
  collection source, so adding a source without the check fails a test.
- A secret-bearing file is excluded, and no manifest field contains file
  content.
- Every dropped or truncated item produces an exclusion record. Nothing is
  dropped silently.
- The manifest records the effective limit and whether it was observed or
  declared.
- With no observed capability profile, assembly uses `context_token_budget`
  unchanged and labels the limit `Declared`; with one, it subtracts the
  documented margin from the observed window.
- A `Required` item that does not fit produces a visible error rather than being
  dropped.
- The improved estimate is calibrated against measured usage from
  [spec 19](../19_token_and_cost_accounting/proposal.md), and the calibration is
  recorded in §7.
- The manifest is written through `AuditLog::record`.
- The prompt is assembled in stable-to-volatile order, and the user's request
  appears once.
- A secret typed into the user's prompt, and a secret in an earlier turn replayed
  as recent conversation, are redacted before the request leaves the machine; the
  built prompt contains the redaction marker and not the raw value.
- Every quality-gate command from `AGENTS.md` passes, and the
  [spec 18](../18_local_evaluation_harness/proposal.md) baseline shows no
  regression in task completion rate.

## 7. Implementation Notes

To be completed during implementation. Record:

- The default per-category fractions, and how often redistribution triggered in
  real use. If most categories routinely release their share, the defaults are
  wrong rather than the mechanism.
- The measured bytes-per-token ratio per content type from the §5.8 calibration,
  against which provider, and how far `len / 4` was off. This is the number that
  justifies the safety margin. The calibration has two halves, and both are
  recorded here — the live half was run on 2026-10-25, the offline half was not
  run and is recorded as such:
  1. **Offline, from recorded usage.** Every `model_request_prepared` audit event
     carries a whole-request estimate computed with `len / 4`
     (`chat.rs:2921-2926`), and every completed call records measured input
     tokens (`task_usage_recorded`, spec 19 §5.4). Joining them per session and
     round gives the estimate/measured ratio for real traffic on this machine,
     with no provider call and no key. The test (`calibration_compares_estimates_with_recorded_usage`,
     `#[ignore]`) takes the directory to read from `DAMAIAN_CALIBRATION_DATA_DIR`
     and writes nothing. This is the comparison that must show the
     new estimator is no worse than `len / 4` (spec 19 §7 measured 2.5–6.3%
     high).
  2. **Live, per content type.** Three small fixed fixture documents — prose, Rust
     code, JSON — sent to the configured provider once, one per call. Only the
     provider can split bytes-per-token by content type. The test is written in
     Task 6 (`#[ignore]`, with the command in its doc comment) and run in Task 7;
     the reviewing human either runs it or explicitly approves that live run in
     chat. **The agent never reads, prints or stores an API key**: the key is
     resolved from `model_api_key_env` exactly as a non-desktop turn resolves it,
     and the doc comment records that a `keychain:` reference is not readable
     from this crate, so that setup uses a config naming an environment variable
     or measures the same documents through the app instead. **The owner ran it on
     2026-10-25**; the constants and this section are updated from its output.
- The gutter size chosen for range merging in §5.4, and whether one gutter value
  covered both search-hit clusters and instruction ranges.
- Whether any collection source proved awkward to check at collection time, per
  §5.6 — that is where a future bypass would appear.
- How much of the deterministic eval tier's context each category used, so the
  defaults are read against a fixture rather than guessed.

**Range gutter: `GUTTER = 2` (Task 3, 2026-10-25).** The rule is
`next.start <= current.end + 1 + GUTTER`, so ranges 10–20 and 23–30 merge
(two lines, 21–22, between them) and ranges 10–20 and 29–35 do not (eight
lines between them). Two is the proposal's original value and was kept: hits in
one function are separated by a blank line, a closing brace or a single
statement, while an eight-line gap is a different region whose intervening lines
are mostly unrelated to the match and would be paid for in tokens. What this
value could not be calibrated against: no collection source produces a
`LineRange` today (§2, §5.4), search hits are whole-file, and instruction files
are collected whole, so there were no real search-hit clusters or instruction
ranges to measure. The boundary is therefore pinned by
`ranges_two_lines_apart_merge_and_ranges_eight_apart_do_not`, and the answer to
"did one gutter cover both clusters and instruction ranges" is **not yet
answerable**: the first real range producer has to re-check it. If ranges later
arrive from findings (§5.3's `FileRanges` third tier) or from pinned selections,
revisit the value with that producer's data rather than changing it silently.

### Calibration against a provider (Task 7, 2026-10-25)

**Step 2, live: run by the owner on 2026-10-25.** The agent did not run it and
read no key. `calibration_measures_bytes_per_token` (`tests/context_assembly.rs`,
`#[ignore]`) sent three fixed documents one call each to **provider
`deepseek`, model `deepseek-v4-flash`**, through the same
`OpenAICompatibleAdapter` + `CurlModelTransport` path a turn uses, and read the
provider's reported `input_tokens` (`UsageSource::Measured`, the figure spec 19
records as measured usage).

| type | bytes | reported input tokens | bytes / token | `len / 4` estimate | `len / 4` error |
|---|---|---|---|---|---|
| prose | 1130 | 251 | 4.50 | 283 | +13% (over) |
| rust | 1332 | 369 | 3.61 | 333 | −10% (under) |
| json | 1037 | 424 | 2.45 | 260 | −39% (under) |

The reported input count includes about 15 tokens of instruction and
chat-template overhead, so a document-only ratio is a little higher (more bytes
per token) than each row's figure; the chosen constants over-count tokens
slightly, which is the safe direction. It is **one sample per type from one
provider**, not a distribution, and the first attempt failed with
`Model provider returned no assistant content` because `max_tokens: 16` was
consumed by the reasoning model's hidden output: the test now asks for 1024, and
only input usage is calibrated. The test's printed ratio was also fixed: it
passed a hundredths-scale value to a percent formatter and printed `45.00`, and
now prints `4.50`.

**Chosen ratios (§5.8):** code **18/5 = 3.6** bytes per token (measured 3.61),
structured **12/5 = 2.4** (measured 2.45), prose **4/1 unchanged** (measured
4.50). Prose stays at 4 on purpose: using the measurement would lower prose
estimates, and Task 6's rule is that classification never estimates below
`len / 4`; the measured 13% over-estimate is headroom that stands in for the
safety margin the Declared path does not subtract. `Symbols` and `RepositoryMap`
stay on the prose ratio for the renderer-ceiling reason §5.8 records.

**How far `len / 4` was off:** +13% high for prose, 10% low for Rust code and
39% low for JSON. The two low figures are why the per-type ratios exist; spec
19 §7's whole-request measurement of 2.5–6.3% high on DeepSeek agreed with the
prose direction and could not show the code and structured errors, because only
the request total is reported.

**Acceptance.** The criterion "the improved estimate is calibrated against
measured usage from spec 19" is met by this live measurement of
provider-reported usage: the harness's measurement is spec 19's
`UsageSource::Measured` input-token figure, read through the adapter rather than
from a stored log. It is not met by an offline comparison over recorded
traffic, because that half was not run: see the next paragraph.

**Step 1, offline: not run.** `calibration_compares_estimates_with_recorded_usage`
was not run because the owner did not provide a data directory to read, and
inventing a corpus or a ratio would be worse than recording the gap. It reads
nothing by default and prints how to point it at a directory; the command is in
its doc comment. Consequences: there is no per-machine whole-request
estimate-vs-measured distribution, and no figure for how often redistribution
triggers in real use (the tier's manifests are not retained by the harness,
and no live session corpus was read). The deterministic tier's
`context_contains`/`context_excludes`/`context_ranks_within` results are
unchanged (see the next section), so nothing in the acceptance run depends on
this.

### Acceptance run against `evals/baseline.json` (2026-10-25)

Command, from the worktree root:

```sh
CARGO_TARGET_DIR="$PWD/target" cargo run -p eval-harness -- run --tier deterministic
```

All 23 scenarios pass and every assertion passes. (`check_pass_rate` below
counts *scenario check commands*: it includes the eight `ls no-such-directory`
failures that `failed_validation_retry` exists to exercise, so it is not a
measure of whether this tier is green.)

`evals/baseline.json` was recorded when the tier held **16** scenarios
(fixture version 4, committed 2026-09-18); the tier now holds **23**, so seven
scenarios have no like-for-like predecessor and the aggregate metrics shift with
the scenario set. Each moved metric is therefore explained below rather than read
as a change in behaviour. Per scenario, every `context_contains`,
`context_excludes` and `context_ranks_within` assertion reports exactly the
`actual` string the baseline recorded, where both have the scenario: the
included-file lists and the `src/checkout.rs` rank are unchanged.

| Metric | Baseline (2026-09-18) | 2026-10-25 | Explanation |
|---|---|---|---|
| Task completion rate | 1.000 | 1.000 | Unchanged. This is the acceptance criterion, and it holds. |
| Check pass rate | 0.1111 (1/9) | 0.7241 (21/29) | Rose because seven scenarios joined the tier and each contributes passing checks; the eight deliberate failures in `failed_validation_retry` are unchanged. The denominator changed, so this is not comparable as a quality figure across the two rows. |
| Approval-policy violations | 0 | 0 | Unchanged. |
| Restricted-path and secret violations | 0 | 0 | Unchanged. |
| Unrelated files changed | 0 | 0 | Unchanged. |
| Recovery success | 1.000 | 1.000 | Unchanged (the one scenario that injects a crash, `resume_interrupted_session`). |
| Tool and model error rate | 0.2727 (9/33) | 0.1190 (10/84) | The denominator grew from 33 tool calls to 84; the deliberate failures are the same eight retries plus the refused restricted read, and the tenth is `worktree_survey`'s planted-path `read_git_diff`, which that scenario expects to refuse. Not an improvement in behaviour. |
| Latency, median | 44 ms | 62 ms | The tier now includes the round-budget pause and long-discovery scenarios (`answer_now_ends_the_turn` ≈ 610 ms, `continue_keeps_state` ≈ 840 ms), which shift the distribution and dominate p90; four repeat runs on this date ranged 62–183 ms median and 572–896 ms p90, so both figures are also noisy at this scale. |
| Latency, p90 | 109 ms | 590 ms | Same cause as the median. |
| Model calls and tool rounds per task | 2.5 (40/16) | 4.043 (93/23) | Seven added scenarios include the longest turns in the tier (`long_discovery` 15 calls, `continue_keeps_state` 12, `read_loop` 10, `answer_now_ends_the_turn` 9). |
| Input and output tokens (estimated) | 101963 | 397245 | 237430 of the increase is the seven new scenarios. The 16 baseline scenarios went 101963 → 159815 because 55 commits after the baseline touched `chat.rs`, adding tool definitions and system-prompt text that are part of the request body this figure estimates; it is a whole-request estimate, not a context figure. §7's next paragraph separates this spec's own effect. |
| Provider cost | not applicable (live-tier) | same | Unchanged marker. |
| Manual repair rate / Patch acceptance rate | not recorded (human) | same | Unchanged markers. |
| Memory recall usefulness / Memory correction rate | not applicable (phase-3b) | same | Unchanged markers. |
| Plan steps planned / verified / completed unverified / blocked | 2 / 1 / 1 / 0 | 2 / 1 / 1 / 0 | Unchanged. |
| Cache hit rate, Findings from exploration commands, Approval prompts by first cause | no baseline row | `not applicable (no-cache-report)`, 0, `setting 1 · shell_control 1 · unknown 2` | Metrics added to the tier after the baseline was committed (specs 49, 59, 32); there is nothing to compare them against. |

**What this spec itself moved.** To separate its effect from the drift since
September, the same tier was run on the pre-implementation tree (the parent of
Task 1, exported with `git archive`) in a separate checkout with its own target
directory. All 23 scenarios run there too, with the same assertions. Every
count and rate is identical — completion 1.000, check pass 0.7241, 0
approval-policy violations, 0 restricted-path/secret violations, 0 unrelated
files changed, recovery 1.000, tool and model error rate 0.119, 4.043 model
calls per task, plan 2/1/1/0 — and **tokens fall from 399391 to 397245**
(−2146, −0.54%), lower in every one of the 23 scenarios and higher in none,
because the user request is no longer sent twice; the included-file lists are
identical, so no category share dropped an item in this tier. Median latency
moved 67 → 62 ms and p90 583 → 590 ms between the two runs, which is this
machine's run-to-run noise at these durations rather than a change. No acceptance
metric regressed; the completion rate the acceptance criterion names is
unchanged.

**Re-run after the measured ratios landed (2026-10-25).** The estimator's
constants changed code 3.5 → 3.6 and structured 3 → 2.4, which changes what
fits a category share. Re-running the tier produced the same figures —
completion 1.000, check pass 0.7241, 0 violations, error rate 0.1190, 4.043
calls per task, 397245 tokens — and every `context_contains`,
`context_excludes` and `context_ranks_within` actual string is unchanged from
the baseline's. Nothing moved because the tier's fixtures fit inside their
category shares either way, and the harness's token figure is the model
adapter's whole-request `len / 4` estimate, not the per-item estimator. Latency
read 62–183 ms median and 572–896 ms p90 across four runs of this date, which
is this machine's run-to-run variance rather than a change (the developer Mac's
process launches stall intermittently; the same variance is recorded in
`docs/DEVELOPMENT.md`).

### Known open points (2026-10-25)

None of these blocks acceptance; each is recorded so a later spec does not
rediscover it:

- **No collection source yields a `LineRange` yet.** Range deduplication,
  whole-file absorption and per-item line provenance are implemented and
  covered by unit tests, but inert in production: every source asks for the
  whole file, so `lineRange` is null on every included item and
  `deduplicated_into_range` can never be recorded. §5.4 records the seam;
  `no_collection_source_yields_a_line_range_yet` fails the day a producer
  appears, at which point the merge must also be covered through
  `build_context` and the `GUTTER = 2` value re-checked.
- **Every limit is `Declared`.** `EffectiveLimit::observed` exists and is
  unit-tested, but no capability profile calls it (Phase 1 WP3), so
  `context_budget.safety_margin` never changes a live limit and
  `limit.source` is always `declared` with `window: null`. The margin is
  therefore headroom that is configured but unspent; the prose ratio's 13%
  over-estimate (the calibration above) is the de facto margin today.
- **The offline calibration half was not run** (no data directory was
  provided), so there is no per-machine distribution of the whole-request
  estimate against measured usage. The live half is the evidence for the
  ratios, and it is one sample per type from one provider.
- **Redistribution frequency and per-category usage of the eval tier were not
  measured.** The harness does not retain its manifests, and no live session
  corpus was read. `categories[].allocated` in a `context_assembled` event is
  where to read it; if most categories routinely release their share, the
  defaults rather than the mechanism are wrong (§5.2).
- **The collection-time check has one awkward family: path spelling.**
  Explicit and mentioned paths can arrive as `./x`, `../x` or an absolute
  in-repo path, so the check has to normalise with the read path's own
  resolver before comparing (Task 5's Step 3a). That is the place a future
  source could bypass the check by comparing raw strings, which is why every
  source goes through `add_file`/`normalise_candidate` and the exhaustive
  `CollectionSource` walk exists.
- **CI passed.** The full seven-check gate passed in CI on 2026-10-25 (Quality
  run 40204226481, on `main` at `8758f62`). Scoped checks passed locally:
  `scripts/quality-gate.sh fmt node lint typos deny`, `npm run specs:check --
  --strict`, `cargo clippy -p workspace-engine --all-targets --locked -- -D
  warnings`, the named test files (232 passed, 2 skipped) and the deterministic
  eval tier (23/23). Workspace clippy and the workspace test suite were not run
  locally, by `AGENTS.md`'s policy; the CI run is their record.
