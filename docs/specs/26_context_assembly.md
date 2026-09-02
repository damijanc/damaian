# Feature Spec: Context Assembly

Status: Not started
Order: 26 of 27
Roadmap: `docs/ROADMAP/03_phase_3_code_understanding.md`, Phase 3, Work
Package 4 (Must). That directory is local-only and not committed, so the
reference is a name rather than a link; this spec is self-contained.
Related spec sections: `ai_coding_assistant_specification.md` section 7.3 (path
and secret policy), section 7.5 (model adapter), section 7.9 (context assembly).
Related implementation specs:
[`02_semantic_search.md`](02_semantic_search.md),
[`11_agents_md_support.md`](11_agents_md_support.md) (instruction resolution and
precedence), [`21_task_plan_progress_and_budget.md`](21_task_plan_progress_and_budget.md)
(plan state as a context category),
[`22_findings_model_and_panel.md`](22_findings_model_and_panel.md) (findings as a
category), [`24_repository_map_and_monorepo_boundaries.md`](24_repository_map_and_monorepo_boundaries.md),
[`25_symbol_and_relationship_index.md`](25_symbol_and_relationship_index.md),
[`27_context_inspector.md`](27_context_inspector.md) (renders the manifest this
spec produces).

## 1. Motivation

Context assembly is a single budget consumed first-come, first-served.

`ContextManager::build_context` (`crates/workspace-engine/src/context_manager.rs:66`)
takes one `token_budget` and fills it by calling `add_text` in a fixed sequence.
`add_text` (`context_manager.rs:248`) estimates a chunk at `len / 4`
(`context_manager.rs:261`), and if it would exceed the budget it **returns false
and the content is dropped**. Ordering is therefore priority, implicitly and
invisibly: whatever is added first wins, and whatever runs into the ceiling
vanishes with no record that it existed.

Three consequences:

- **Any one category can starve every later one.** A prompt mentioning a large
  file can consume the budget on that file, leaving nothing for instructions,
  search results, or symbols. Nothing bounds a category's share.
- **Nothing is deduplicated.** Two search hits in the same function send that
  function twice, paid for twice.
- **Nothing is recorded.** `ContextPlan` (`context_manager.rs:31-38`) carries
  items and a token estimate, but no provenance and no account of what was
  excluded or why. The user cannot see what was sent, and neither can a
  developer debugging a bad answer. That is the gap
  [spec 27](27_context_inspector.md) exists to close, and it needs this spec to
  produce the data.

There is also a structural gap that blocks two of the roadmap's requirements:
`ContextItem` has a `path` but **no line range** (`context_manager.rs:22-29`), so
content is whole-file. "Deduplicate overlapping file ranges" and "include path
and line provenance with every included range" are not adjustments to the
current model — they require ranges to exist first.

## 2. Current State

- **One flat budget.** `build_context(…, token_budget: usize)`
  (`context_manager.rs:66-75`) with no per-category allocation. `add_text` and
  `add_file` both take the same single `token_budget`.
- **Drop-on-overflow, silently.** `add_text` (`context_manager.rs:248-262`)
  returns `false` when `*token_estimate + tokens > token_budget`; the caller
  does not record the omission.
- **`ContextItem` is whole-file and range-less**:
  `{ kind: String, path: Option<String>, content: String, tokens: usize,
  redaction_status: String }` (`context_manager.rs:22-29`). `kind` is a free
  string, not an enum.
- **`ContextPlan`** is `{ repository_id, task_id, token_estimate, items, files }`
  (`context_manager.rs:31-38`). No manifest, no exclusion record, no per-category
  totals.
- **Redaction happens per item, at the right place.** `add_text` runs
  `scanner.redact(content)` before measuring or storing
  (`context_manager.rs:260`), and `redaction_status` is recorded per item. This
  is the pattern to preserve.
- **The estimate is `len / 4`** (`context_manager.rs:261`), also used by
  `ModelAdapter::estimate_tokens` (`crates/workspace-engine/src/model.rs:209`).
- **Instruction resolution exists.** `agent_instruction_paths`
  (`context_manager.rs:280-309`) walks ancestors for `AGENTS.md`, per
  [spec 11](11_agents_md_support.md).
- **`context_token_budget` is declared per provider**
  (`crates/workspace-engine/src/config.rs:115`), documented as bounding context
  and unrelated to `max_output_tokens`.
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
  ([spec 02](02_semantic_search.md)) and symbols
  ([spec 25](25_symbol_and_relationship_index.md)); this spec allocates budget
  among what they return and records what it did.
- Conversation compaction — Phase 3 WP6, Should-tier. The pipeline reserves a
  category for a conversation summary and tolerates its absence.
- Model routing — Phase 3 WP7.
- A tokenizer per model family. §5.8 improves the estimate without adding a
  tokenizer dependency, and is explicit that the result is still an estimate.
- Persisting assembled context beyond the manifest.
- The inspector UI — [spec 27](27_context_inspector.md).
- Changing `SecretScanner` or `path_policy.rs`.

## 5. Design

### 5.1 Ranges first

Requirements 2 and 4 need a range, so `ContextItem` gains one and `kind` becomes
an enum:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContextCategory {
    /// Never dropped. Safety instructions and the user's own message.
    Required,
    Instructions,   // AGENTS.md hierarchy, per spec 11
    Pinned,         // user-selected, per spec 27
    PlanState,      // spec 21
    RepositoryMap,  // spec 24
    Symbols,        // spec 25
    SearchResults,
    FileRanges,
    Findings,       // spec 22
    ConversationSummary,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LineRange {
    /// 1-based, inclusive.
    pub start: u32,
    pub end: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContextItem {
    pub category: ContextCategory,
    pub path: Option<String>,
    /// None means the whole file, or content with no file identity.
    pub range: Option<LineRange>,
    pub content: String,
    pub tokens: usize,
    pub redaction_status: String,
    /// Where this came from, and how much to trust it. Carried from spec 25.
    pub provenance: Option<Provenance>,
    /// Root that owns the path, from spec 24. Requirement 6 of spec 24.
    pub root_path: Option<String>,
}
```

The declaration order of `ContextCategory` is the pipeline order from the
roadmap, and `Ord` is derived so priority is the enum's own ordering rather than
a separate table that could disagree with it.

`Required` is first and is the category [spec 27](27_context_inspector.md)
forbids the user from removing. Naming it in the type is what makes that
guarantee checkable rather than a UI convention.

### 5.2 Per-category budgets

```text
context_budget.instructions=0.10
context_budget.pinned=0.20
context_budget.plan_state=0.05
context_budget.repository_map=0.05
context_budget.symbols=0.10
context_budget.search_results=0.15
context_budget.file_ranges=0.25
context_budget.findings=0.05
context_budget.conversation_summary=0.05
```

Fractions of the effective limit (§5.7) rather than absolute counts, so one set
of settings works across a 32k and a 200k context window. `Required` has no
budget: it is subtracted from the total before categories are allocated, because
a required item that does not fit is not a budgeting problem but an error the
user must see.

Two rules make the allocation useful rather than merely tidy:

- **Unused allocation is redistributed.** A category that uses less than its
  share releases the remainder to later categories in pipeline order. Without
  this, a repository with no findings and no plan wastes 10% of the window on
  nothing — strict per-category caps would make the common case worse than
  today's flat budget.
- **A category over its share is truncated within itself**, dropping its
  lowest-priority members (§5.3) rather than stealing from another category.

Overflow is recorded, never silent. This is the specific defect in
`add_text`'s `return false` (`context_manager.rs:262`): every dropped or
truncated item produces an exclusion record (§5.5).

### 5.3 Ordering within a category, and exact over semantic

Requirement 3 is a rule about `SearchResults` versus everything else, and about
ordering inside `SearchResults`:

| Within | Order |
|---|---|
| `Symbols` | Language-server provenance before heuristic (spec 25's `Provenance` ordering), then match quality |
| `SearchResults` | Exact keyword matches, then symbol-name matches, then semantic-similarity matches |
| `FileRanges` | Explicitly requested paths, then paths mentioned in the prompt, then ranges implied by findings |
| `Findings` | Severity, then newest |

"Prefer exact evidence over semantic similarity" is therefore not a global
tie-break but an ordering inside the contested category. Semantic hits are the
first thing dropped when `SearchResults` is over budget, which is the correct
trade: an exact match is evidence, a similarity score is a suggestion.

### 5.4 Range deduplication

Requirement 2, applied after collection and before budgeting:

1. Group items by `path`.
2. Sort ranges by `start`.
3. Merge ranges that overlap or are separated by fewer than a small gutter of
   lines — two hits three lines apart are better sent as one range than as two
   with a duplicated header.
4. Re-read the merged range once, as one item.

A whole-file item (`range: None`) absorbs every range for that path: if the whole
file is included, a range within it is redundant. This is the case most likely to
be missed, because the two come from different collection paths — a prompt
mention producing the whole file and a search hit producing a range.

Merging preserves the highest-priority `provenance` among the merged items, so a
range that came partly from a language-server symbol is not downgraded by being
merged with a heuristic one.

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
    /// Per category: allocated, used, item count.
    pub categories: Vec<CategoryUsage>,
    /// Every item, by path and range — never content.
    pub included: Vec<IncludedRef>,
    /// Everything left out, with a reason. Requirement 6's real value.
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
    TooLarge,
    UserRemoved,
    Binary,
}
```

The manifest holds **paths, ranges, counts, and reasons — never content**. That
is what makes it safe to write to the audit log and safe to show in a
diagnostics view, and it satisfies requirement 6's "sanitized" by construction
rather than by a redaction pass. Content lives in the assembled context, which
is sent to the provider and not persisted.

`excluded` is the part worth insisting on. A user asking "why didn't it look at
`upload.rs`?" currently has no answer; with this, the answer is
`RestrictedPath` or `CategoryBudgetExhausted` or `TooLarge`.

The manifest is recorded through `AuditLog::record`
(`crates/workspace-engine/src/audit.rs:42`), which redacts field values on the
way in — a second safety net over a structure that should contain nothing to
redact.

### 5.6 Exclusion at every stage

Requirement 5 says restricted and secret-bearing content is excluded at every
stage, not only at the end. Concretely, `path_policy.rs` is applied at
**collection** time in each source — search results, symbol hits, prompt
mentions, findings ranges, pinned items — and again at read time, and
`SecretScanner` runs before an item is measured or stored, as `add_text` already
does (`context_manager.rs:260`).

Checking twice is deliberate. A single check at the end is one edit away from
being bypassed by a new collection path, and the acceptance criterion is written
to assert exclusion **at every stage** so a new source cannot be added without a
test forcing it through the check.

A restricted path never enters the manifest's `included` list, and appears in
`excluded` as `RestrictedPath` — which is itself the assertion that it was
considered and refused, rather than never noticed.

### 5.7 The effective limit

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EffectiveLimit {
    pub tokens: usize,
    pub source: LimitSource,   // Observed | Declared | Default
    pub safety_margin: f32,
}
```

Resolution order:

1. **Observed** — a capability profile from Phase 1 WP3, if one exists.
2. **Declared** — `context_token_budget` for the provider
   (`config.rs:115`), with a documented safety margin (default 10%) subtracted.
3. **Default** — a conservative built-in when neither exists.

Phase 1 WP3 is Should-tier and may not have shipped, so the declared path is the
realistic one and is treated as the primary case rather than a fallback. The
margin exists because the estimate is an estimate (§5.8): submitting exactly the
declared limit and being refused wastes the whole assembly, and 10% of a context
window is cheap insurance.

`source` is recorded in the manifest and shown in the inspector, so a user
seeing conservative context can tell whether the limit was measured or assumed.

### 5.8 A better estimate, still an estimate

Requirement 8. `len / 4` (`context_manager.rs:261`) is a fixed
bytes-per-token ratio that is wrong in opposite directions for prose and for
code: dense code with many short tokens runs well over four bytes per token,
and long identifiers under.

Without adding a tokenizer dependency (§4), the improvement is a
content-aware ratio: estimate per item using a ratio selected by the item's
language and category — one for prose and Markdown, one for code, one for
structured data like JSON — calibrated once against measured provider usage from
[spec 19](19_token_and_cost_accounting.md) and recorded in §7.

The calibration is the point. [Spec 19](19_token_and_cost_accounting.md) records
measured input tokens per call; comparing those against the estimate for the
same call gives a real ratio per content type instead of a guessed one, and
turns `len / 4` from folklore into a number with a derivation.

It remains an estimate, is labelled as such in the manifest, and the safety
margin in §5.7 is what absorbs the residual error. Where a provider reports
usage, the measured figure supersedes the estimate after the fact for accounting
— but assembly must decide before the call, so it decides on the estimate.

### 5.9 Documentation

`docs/USER_GUIDE.md`: what goes into context, in what order, and how the
per-category budgets work. `docs/TROUBLESHOOTING.md`: how to read a manifest,
what each exclusion reason means, how to tell an observed limit from a declared
one, and what to do when content is being excluded as
`CategoryBudgetExhausted`.

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
- The manifest records the effective limit and whether it was observed,
  declared, or default.
- With no observed capability profile, assembly uses `context_token_budget`
  minus the documented margin and labels the limit `Declared`.
- A `Required` item that does not fit produces a visible error rather than being
  dropped.
- The improved estimate is calibrated against measured usage from
  [spec 19](19_token_and_cost_accounting.md), and the calibration is recorded in
  §7.
- The manifest is written through `AuditLog::record`.
- The five quality-gate commands from `AGENTS.md` pass, and the
  [spec 18](18_local_evaluation_harness.md) baseline shows no regression in task
  completion rate.

## 7. Implementation Notes

To be completed during implementation. Record:

- The default per-category fractions, and how often redistribution triggered in
  real use. If most categories routinely release their share, the defaults are
  wrong rather than the mechanism.
- The measured bytes-per-token ratio per content type from the §5.8 calibration,
  against which provider, and how far `len / 4` was off. This is the number that
  justifies the safety margin.
- The gutter size chosen for range merging in §5.4.
- Whether any collection source proved awkward to check at collection time, per
  §5.6 — that is where a future bypass would appear.
