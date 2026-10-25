# Context Assembly — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Implements:** [`proposal.md`](proposal.md), with the corrections below and the
current-code map in [`context.md`](context.md)
**Started:** 2026-10-24
**Status:** In progress. Planned 2026-10-24 against `main`; Task 1 landed 2026-10-25.

## Open questions for the owner

Each one is answered by a stated assumption in the plan so that no task is
blocked. If the answer is different, the named task changes in one place.

1. **Is `context_token_budget` a packing budget or the provider's window?** The
   config docs call it a packing budget (`config.rs:1920-1924`), and the built-in
   grants 64k to a model with a 1M-token window on purpose. Requirement 7 and the
   acceptance criterion say the declared path subtracts "the documented safety
   margin". Subtracting from a packing budget would shrink every existing user's
   context by 10% for no safety gain. **Assumption:** the declared path uses the
   config value unchanged (`safety_margin_bps = 0`), and the margin applies only
   on the observed path, where a real window is known. Task 4's
   `EffectiveLimit` is the only place this decides anything (Correction C6).
2. **Should the `Findings` category collect automatically?** Spec 22 ships
   findings, but the agent sees only the subset the user selects for a repair
   request, and that flow already works. **Assumption:** it stays reserved in
   #26 (proposal §5.11); the decision belongs with a follow-up, not with a
   pipeline rewrite. Task 4 only has to keep the slot and redistribute its share.
3. **May the `Instructions` category borrow from later categories?** A 10% share
   can truncate a large `AGENTS.md` hierarchy, and the instructions change the
   model's behaviour toward the user's own rules. **Assumption:** keep the
   proposal's rule (a category over its share truncates within itself), with
   nesting priority *inside* the category so the most specific file survives, and
   measure the eval fixture in Task 4. Borrowing would contradict an acceptance
   criterion; if the owner wants it, it is a change to that criterion first.
4. **Who owns the prompt-section ordering, #26 or #49?** `docs/specs/README.md`
   says #26, #55 and #49's reuse slice all rewrite `build_model_prompt` and that
   "#26 decides the ordering"; `docs/PLAN/OBSERVATIONS.md` entry 5 says #49 or
   #26. **Assumption:** #26 orders the sections and exposes them by name; #49's
   reuse slice places breakpoints. #26 emits none.

## Progress

| Task | State | Notes |
|---|---|---|
| 1 · Categories, ranges and one renderer | Done 2026-10-25 | **Landed:** `ContextCategory` (all 11 variants, derived `Ord` so declaration order *is* pipeline priority) and `ContextKind` (10 variants, `as_str()` returning today's exact prompt spellings, and `category()` as the single kind→category mapping). `ContextItem` gained `category`, `kind: ContextKind`, `range: Option<LineRange>` (serialised `lineRange`), `provenance: Option<Provenance>` and `root_path`; `LineRange` gained `Serialize`/`Deserialize`; `add_text`/`add_file` take a `ContextKind` and derive the category, so no caller can set them inconsistently. `ContextPlan::render_item` is the one header format (a range appends `path:start-end`), `sections()` returns the plan-owned blocks stable→volatile (`PromptSection`/`PromptSectionName`, each with `heading()`), and `included_paths()` is the Task 5 seam. `build_model_prompt` now takes `&ContextPlan` + `&SecretScanner` and `build_edit_prompt` takes `&ContextPlan`; neither takes the raw prompt. Chat order is `Repository context:` → `Recent conversation:` → `User request:`, the request is the redacted `Required` item's content, and each prior message is redacted *before* truncation. The edit flow keeps `User edit request:` first (a single call has no cache prefix to order) but renders through the same sections. Existing `item.kind` readers moved to `ContextKind` in `tests/repository_map.rs`, `tests/symbol_index.rs` and `tests/hooks.rs`; `tests/foundation.rs`'s assertions were prompt strings and stayed unchanged; chat's `approval_prompt_wording_tests` now builds a plan by hand. **Tests:** 10 new in `tests/context_assembly.rs` (range-less byte-identical, ranged header, section order, request once, chat and edit prompt redaction, replayed-turn redaction, oversized-prompt survival added by the review fix, chat/edit items identical, category order) and 3 serde unit tests in `context_manager.rs`. `cargo nextest run -p workspace-engine --test context_assembly --test foundation --test repository_map --test symbol_index --test hooks --test prompt_cache`: 404 passed, 3 skipped. `--lib -E 'test(context_manager)'`: 3 passed. `--lib -E 'test(chat::)'`: 70 passed. Scoped `cargo clippy -p workspace-engine --all-targets --locked -- -D warnings` clean; `scripts/quality-gate.sh fmt node lint typos deny` clean (deny included, no lock issue); `cargo run -p eval-harness -- run --tier deterministic`: all 23 scenarios pass, completion 1.000, check pass 0.724, 0 restricted-path/secret violations. **Falsified (all reverted, all caught):** (a) rendering `Required` in the context block as well as the request → `the_user_request_appears_once` (2 occurrences); (b) emitting the conversation block before the context → the order test; (c) changing the range-less header ` ---` to ` ----` → the byte-identical test; (d) overriding the request with a reintroduced raw `prompt` argument → the chat prompt-secret test printed the raw `AKIA…` key, and the same mutation in `build_edit_prompt` → the edit prompt-secret test; (e) using `message.content.clone()` instead of `scanner.redact` → the replayed-secret test printed the raw key. **Deviations:** (1) The request section is the `Required` item's content, not its `--- user_prompt ---` header — the heading already names it; proposal §5.10 records this. (2) The edit flow keeps request-first order; only chat moves to stable→volatile. (3) `sections()` returns only the two plan-owned sections; the conversation block is the caller's, because the plan holds no messages. (4) `ContextItem::range` keeps the proposal's field name and is serialised as `lineRange` (Task 1 Step 1's required serde name). **Discoveries for later tasks:** (1) `RetrievedFile` maps to `FileRanges`, so keyword/semantic hits currently land in `FileRanges`, not `SearchResults`; Task 2/4 must either add a `ContextKind` for them or let a collection source set the category explicitly — `add_text` deliberately has no category parameter today. (2) The legacy `command_context` block still renders after the request with its own `--- command_result ---` header; its only production call passes `None`, so it was left alone. (3) `tests/prompt_cache.rs` passed unchanged, so the new section order is still deterministic. **Review fix 2026-10-25:** the first cut rendered the request only from the `Required` item but left it subject to `add_text`'s budget check, so a prompt larger than `context_token_budget` produced an empty `User request:` — a regression against the old raw-prompt path. `add_text` now exempts `ContextCategory::Required` from the over-budget refusal while still redacting it and charging its tokens to `token_estimate`; it stays one code path, and a comment records that Task 4 replaces the exemption with the visible "Required item does not fit" error. New test `a_prompt_larger_than_the_budget_still_reaches_the_model_redacted` (16-token budget, prompt with a distinctive marker and an AWS key) asserts the marker and the redaction marker arrive and the raw key does not. **Falsified (reverted):** restoring the unconditional budget check makes that test fail on the empty `User request:`. Rerun: `--test context_assembly --test foundation --test prompt_cache` 179 passed; the six-file set 404 passed, 3 skipped; scoped clippy clean; `scripts/quality-gate.sh fmt typos` clean. |
| 2 · Collection sources and exclusion at collection time | Not started | |
| 3 · Range deduplication and ranged reads | Not started | |
| 4 · Per-category budgets and the effective limit | Not started | |
| 5 · The manifest, its audit record, and the callers | Not started | |
| 6 · A content-aware estimate | Not started | |
| 7 · Calibration, docs and close-out | Not started | |

## Global Constraints

Every task's requirements implicitly include this section.

- **Read [`context.md`](context.md) in full before Task 1**, then §2, §3 and §5
  of [`proposal.md`](proposal.md) and the Corrections table below. Where a
  correction contradicts the proposal's prose, the correction wins, and the task
  that acts on it updates the proposal in the same change (they were folded in
  while planning; check §2, §5 and §6 agree after each task).
- **Secret redaction runs before any item is measured or stored.** `add_text`
  (`context_manager.rs:410-440`) is the only place an item's content is created,
  and `scanner.redact` stays ahead of the token estimate and the budget test in
  it. No new path may produce item content: a collection source yields paths and
  ranges, and the one reader redacts. Nothing bypasses `SecretScanner`, and
  `SecretScanner` itself is not changed.
- **Nothing unredacted reaches the provider.** The user's prompt and the
  prior-conversation block are not items, and today neither is redacted (C17):
  the request renders from the redacted `Required` item, never from the `prompt`
  argument, and each prior message passes through the orchestrator's
  `SecretScanner` before the conversation block is written. A test seeds a token a
  `SecretScanner` detects into the prompt and into an earlier turn and asserts the
  raw value is absent from the built request.
- **The manifest never contains content.** `ContextManifest` and everything it
  holds — included refs, exclusion records, category usage — carry paths,
  ranges, counts, reasons and enum names only. A test asserts that a seeded
  secret and a content marker appear in no manifest field.
- **`path_policy` is applied at collection time in every source and again at
  read time** (proposal §5.6). Every file-bearing source has its own test
  asserting that a restricted path becomes a `RestrictedPath` exclusion record
  and never an included item. The source list is an exhaustive enum (no wildcard
  arm), so a new source that skips the check fails to compile *and* fails
  `every_collection_source_refuses_a_restricted_path`.
- **A new config field must be classified explicitly.** The `context_budget.*`
  fractions and `context_budget.safety_margin` join `ConfigOverlay`,
  `classify_overlay_fields!` (exhaustive, `config.rs:2787-2795`),
  `apply_overlay_scoped`, both `to_policy_text`s and `split_profile_keys`
  (exhaustive, `profile.rs:322`). **Decision: `Capability`, restrict-only at
  repository scope.** A share steers model traffic toward or away from a
  repository's own content, and spec 34's rule is that a clone may tighten a
  bound but never redirect or widen one; so repository scope may lower a share
  and raise the safety margin, never raise a share or lower the margin. Profile
  scope keeps only the restrictive direction the other numeric caps take.
  `tests/permission_profiles.rs` gains the weakening cases; a repository-scope
  raise must be refused as `RestrictOnly`, never silently clipped.
- **No command-policy change.** `permission_profiles`, `command_policy`'s unit
  tests and `repository_config_trust` pass unchanged after every task — the last
  two are never edited. If a task finds itself wanting to change *which*
  commands run or ask, stop and say so.
- **No new dependency.** serde is already present in `workspace-engine`; basis
  points need nothing.
- **Tests that build a temp repository set `enable_index_watcher: false`** and
  their own `data_dir` inside the fixture (reuse `foundation.rs:55`'s
  `test_config`). Never write into the user's real Damaian data directory.
- **The prompt stays deterministic.** `tests/prompt_cache.rs`'s
  `two_assemblies_of_an_unchanged_repository_are_identical` must pass after every
  task, unchanged: no timestamp, hash iteration order, filesystem order, or float
  rounding may reach the rendered prompt or decide which item is included.
  `ContextManifest.assembledAtMs` is the one time-dependent field; the manifest
  is only written to the audit record and is never compared.
- **`ContextPlan.files` keeps meaning "unique included paths, in inclusion
  order".** The eval harness reads it as `context_files` and asserts
  `context_contains`, `context_excludes` and `context_ranks_within` against it.
- **A private Cargo target directory per worktree.** Prefix every `cargo`
  command with `CARGO_TARGET_DIR="$PWD/target"` from the worktree root.
- **Scoped checks per task; never workspace-wide.** Run
  `scripts/quality-gate.sh fmt node lint typos deny`,
  `cargo clippy -p workspace-engine --all-targets --locked -- -D warnings` (plus
  `-p desktop-shell` or `-p damaian-cli` in a task that touches them), and the
  named test files. **Never run `cargo clippy --workspace` or
  `cargo nextest run --workspace` in any task** (AGENTS.md, since 2026-10-15);
  the full seven-check gate runs on CI.
- **Falsify every load-bearing test.** Break what it guards, confirm the named
  test fails, revert, and record the mutation in the Progress row.
- **Never `git commit`, `git push`, stash or switch branches unasked.** Each
  task ends by showing the change and the scoped check results. One-line commit
  subject, no body.

## Corrections to the proposal found while planning

Each one names the task that owns it. All are folded into
[`proposal.md`](proposal.md) already; this table is why.

| # | Proposal said | Code says | Resolution |
|---|---|---|---|
| C1 | `build_context` at `context_manager.rs:66`, `add_text` at `:248`, the estimate at `:261`, redaction at `:260`, `ContextItem` at `:22-29`, `ContextPlan` at `:31-38`, `agent_instruction_paths` at `:280-309`, `context_token_budget` at `config.rs:115`, `estimate_tokens` at `model.rs:209` | `:96`, `:410`, `:423`, `:422`, `:25-32`, `:34-41`, `:442-470`, `config.rs:1925-1934`, `model.rs:354` | §2 and every section that cites a line corrected. The estimate is `len.div_ceil(4)`, not `len / 4`. **Task 1** |
| C2 | "Nothing is deduplicated" (§1, §2) | `add_file` skips a path already in `files` (`context_manager.rs:378`), so whole files are deduplicated today; ranges do not exist, so nothing *can* overlap | §1 corrected: the problem is that content is whole-file, plus the whole-file-absorbs-range case that ranges will introduce. §5.4 restated. **Task 3** |
| C3 | `kind` becomes the `ContextCategory` enum (§5.1) | `kind` is the model-facing source label: the prompt header prints it, and `foundation.rs:4354`, `repository_map.rs:1855-1858`, `symbol_index.rs:3377-3379` and `hooks.rs:3019` assert the strings `agent_instruction`, `retrieved_file`, `explicit_file`, `project_rule`, `hook_context`, `user_prompt` | Two fields: `category: ContextCategory` for budget and priority, `kind: ContextKind` for the header, with today's spellings. §5.1 corrected. **Task 1** |
| C4 | A new `LineRange { start: u32, end: u32 }` (§5.1) | `crate::file_access::LineRange` already exists (`file_access.rs:16`), is `usize`, 1-based inclusive, re-exported at `lib.rs:103`, and is what `ReadWindow::Range` takes | Reuse it. §5.1 corrected. **Task 1** |
| C5 | "Re-read the merged range once" (§5.4) | `ReadWindow::Range` is clamped to `max_read_lines` (400) at `file_access.rs:184-189`, deliberately; `Whole` opts out but returns redacted text whose line structure redaction may have changed | Add `ReadWindow::ContextRange(LineRange)`: no line cap, byte cap, path policy and redaction unchanged. Slicing a `Whole` read is rejected for the redaction reason. §5.4 corrected. **Task 3** |
| C6 | The declared limit is `context_token_budget` minus a 10% margin (§5.7, acceptance) | `context_token_budget` is documented as a *packing* budget (`config.rs:1920-1924`) and defaults to 64k against a 1M window (`:3843-3848`); a margin on it shrinks real context for no safety gain | Declared path uses the value unchanged (`safety_margin_bps = 0`); the margin applies to an observed window only. Acceptance criterion restated; the alternative is open question 1. **Task 4** |
| C7 | `safety_margin: f32` and decimal fractions (§5.2, §5.7) | `ContextItem` and `ContextPlan` derive `Eq` and tests compare plans; a float costs `Eq` and makes the last token's allocation depend on rounding | Fractions and the margin are parsed as decimals and stored as integer basis points (`u16`), sum `<= 10_000`, oversubscription is a config error. §5.2, §5.7 corrected. **Task 4** |
| C8 | Most category slots have a source and are simply budgeted (§5.2) | `Pinned` needs #27's storage, `ConversationSummary` needs #55, and `Findings` reaches the model only as the user-selected repair subset (spec 22). Only `PlanState` (on a resumed turn) and `HookContext` (with a hook configured) have real sources today | `PlanState` and `HookContext` are wired in #26; `Pinned`, `Findings` and `ConversationSummary` stay reserved (proposal §5.11), appear with zero use, and their shares redistribute. **Tasks 2, 4** |
| C9 | The manifest is "recorded through `AuditLog::record`" (§5.5) | `ContextManager` holds no `AuditLog` (`context_manager.rs:43-51`); both orchestrators already have one and already record `model_request_prepared` there (`chat.rs:2927`) | The manifest travels inside `ContextPlan`; the caller records one `context_assembled` event. Adding an audit log to the manager would be a second audit path for one event. **Task 5** |
| C10 | §5.6 applies path policy at collection time "in every source" | `ContextManager` holds no `PathPolicy` either; the policy lives in the private `FileAccessController` field and in `SymbolSource` | `ContextManager::new` gains the engine's `PathPolicy` (`workspace_engine.rs:88-117`). Same instance, not a second rule. **Task 2** |
| C11 | §5.4's dedup and §5.5's exclusions are all the manifest needs | Acceptance says every *dropped or truncated* item is recorded, and a block trimmed to fit is neither dropped nor `TooLarge` | `ExclusionReason::Truncated` added. §5.5 corrected. **Task 5** |
| C12 | Eight positional arguments to `build_context` | Plan state and findings come from `SessionStore`, not the index, and there are already `too_many_arguments` allowances at `context_manager.rs:95` | One `ContextRequest<'_>` struct; `build_context_with_hooks` is removed and its two production callers plus its one test are updated. **Task 2** |
| C13 | §5.3 orders `Symbols` by provenance, then match quality | `SymbolIndex::find_for_prompt` already sorts exactly that way (`symbol_index.rs:519-535`), and the symbols item is one rendered block with the hedge inside its text (`:545-591`) | #26 does not re-sort. The item's `provenance` is the lowest provenance it contains. §5.3 corrected. **Task 2** |
| C14 | A category over its share drops items (§5.2) | The map and symbols blocks have their own ceilings (800, 1,600) and `add_text` drops a block that does not fit in one go, so a smaller category share would drop the whole block | A block item over its share is re-rendered at the smaller ceiling (`render_for_model` already takes one); only a block that cannot render one line is excluded. §5.2 corrected. **Task 4** |
| C15 | `build_model_prompt` produces the prompt; the edit flow is separate (§5.9 mentions only docs) | `build_edit_prompt` (`edit.rs:1086-1105`) is a second copy of the item renderer, so the header format can drift | One renderer on `ContextPlan`; the two callers keep their own preambles. §5.10 added. **Task 1** |
| C16 | The prompt's shape is out of scope (never stated) | The sections are ordered conversation → request → context, the reverse of a cacheable prefix, and the user prompt is sent **twice** (`context_manager.rs:139-147` and `chat.rs:6126-6127`). #55 §2 and #49 `context.md` §3.2 both assign the ordering to #26 or #49; `OBSERVATIONS.md` entry 5 says the same | §5.10 added: stable-to-volatile order, `Required` rendered under the final `User request:` heading so the request appears once, sections named for #49. No breakpoints emitted. **Task 1** |
| C17 | §5.10 said the request moves out of the renderer but never said what it is rendered *from*; nothing claimed the prompt was redacted | Nothing redacts the user's prompt on any path into the engine (shell `lib.rs:2270`, CLI `main.rs:516/532`, harness `runner.rs:368`): `build_model_prompt` writes the raw `prompt` (`chat.rs:6126-6127`), `build_edit_prompt` the same (`edit.rs:1088-1089`), and the `Recent conversation:` block writes each stored message raw (`chat.rs:6117-6124`) with the transcript stored verbatim by spec 16. The duplicate `user_prompt` item *is* redacted (`context_manager.rs:422`), so one copy of the prompt is clean and the other is not | The request renders from the redacted `Required` item and every prior message passes through `SecretScanner` before it is written. No fallback to the raw argument. §5.10 and §6 corrected. Evidence in `context.md` §2. **Task 1** |

## File Structure

| File | Change |
|---|---|
| `crates/workspace-engine/src/context_manager.rs` | `ContextCategory`, `ContextKind`, `ContextItem` fields, `ContextPlan` manifest and renderer (Task 1); `ContextRequest`, collection sources, collection-time policy (Task 2); dedup and merge (Task 3); `ContextBudget`, allocation, truncation (Task 4); manifest filling (Task 5); estimate (Task 6) |
| `crates/workspace-engine/src/file_access.rs` | `ReadWindow::ContextRange` only (Task 3). Path policy, byte cap and redaction untouched |
| `crates/workspace-engine/src/config.rs` | `context_budget.*` parse, defaults, overlay field, restrict-only merge, policy text (Task 4) |
| `crates/workspace-engine/src/profile.rs` | `split_profile_keys` carries the two restrictive directions (Task 4) |
| `crates/workspace-engine/src/effective_policy.rs` | `rule_label` for the new keys (Task 4) |
| `crates/workspace-engine/src/workspace_engine.rs` | Pass the engine's `PathPolicy` and the budget to `ContextManager::new` (Tasks 2, 4) |
| `crates/workspace-engine/src/chat.rs` | `build_model_prompt` uses the shared renderer and the new section order (Task 1); `build_context` call site, `context_assembled` record (Tasks 2, 5); plan handed to assembly (Task 2) |
| `crates/workspace-engine/src/edit.rs` | `build_edit_prompt` uses the shared renderer (Task 1); call site, manifest record (Tasks 2, 5) |
| `crates/workspace-engine/src/lib.rs` | Re-export the new context types (Tasks 1, 4, 5) |
| `crates/workspace-engine/tests/context_assembly.rs` | New. Every task adds to it |
| `crates/workspace-engine/tests/{foundation,repository_map,symbol_index,hooks}.rs` | Only where a `kind` string or an item position moved; no assertion weakened (Task 1) |
| `crates/workspace-engine/tests/permission_profiles.rs` | Two weakening cases for the new keys (Task 4) |
| `crates/workspace-engine/tests/agent_tools.rs` | The new `ReadWindow` variant exhausts a match here (Task 3) |
| `docs/USER_GUIDE.md`, `docs/TROUBLESHOOTING.md` | Task 7 |
| `docs/specs/README.md`, `CHANGELOG.md`, specs naming #26 | Task 7 (close-out) |

## Interfaces

Task 1 lands the types; later tasks consume them. Names are a contract between
tasks; fields may grow, but a rename is a rebase across tasks.

```rust
// context_manager.rs
pub enum ContextCategory { Required, Instructions, Pinned, PlanState,
    RepositoryMap, Symbols, SearchResults, FileRanges, Findings,
    ConversationSummary }

pub enum ContextKind { UserPrompt, ExplicitFile, AgentInstruction, ProjectRule,
    HookContext, RepositoryMap, Symbols, RetrievedFile, PlanState, Pinned }

pub struct ContextItem { category, kind, path: Option<String>,
    range: Option<LineRange>, content: String, tokens, redaction_status,
    provenance: Option<Provenance>, root_path: Option<String> }

pub struct ContextRequest<'a> { repository_root: &'a Path, repository_id: &'a str,
    task_id: &'a str, prompt: &'a str, index: Option<&'a RepositoryIndex>,
    explicit_paths: &'a [String], limit: usize, hook_context: &'a [(String, String)],
    plan: Option<&'a Plan> }

pub struct ContextPlan { /* today's fields */ manifest: ContextManifest }

impl ContextPlan {
    pub fn sections(&self) -> Vec<PromptSection>;   // stable → volatile
    pub fn render_item(item: &ContextItem) -> String; // the one header format
    pub fn included_paths(&self) -> &[String];      // unique included paths
}

pub enum LimitSource { Observed, Declared }
pub struct EffectiveLimit { tokens, source, safety_margin_bps: u16,
    window: Option<usize> }
/// One share per budgeted category; `Required` has none.
pub struct ContextBudget { shares: [u16; 10], safety_margin_bps: u16 }
/// `estimated` is always true for assembly and exists so a figure can never be
/// read as measured (spec 19 requirement 4); a post-send view may set it false.
pub struct CategoryUsage { category, allocated, used, items, truncated,
    estimated: bool }
pub struct IncludedRef { category, kind, path: Option<String>,
    range: Option<LineRange>, tokens, root_path, provenance }
pub struct ExclusionRecord { reason: ExclusionReason, path: Option<String>,
    range: Option<LineRange>, category, detail: Option<String> }
pub struct ContextManifest { repository_id, task_id, assembled_at_ms, limit,
    total_tokens, categories, included, excluded }

pub enum CollectionSource { Explicit, Mention, KeywordHit, SemanticHit,
    Instruction, ProjectRule, Map, Symbols, HookContext, PlanState }
```

`CollectionSource` exists so §5.6's per-source check is exhaustive: the assembler
matches on it with no wildcard arm, and one test walks every variant.

`ContextRequest` is a struct rather than eight parameters for two reasons: the
argument list is already at the `too_many_arguments` limit, and spec 27 adds a
`pinned` field to it without touching what either caller passes for everything
else. #26 populates none.

## Order and parallelism

```
1 ──> 2 ──> 3 ──> 4 ──> 5 ──> 6 ──> 7
```

Sequential; Tasks 1–5 all edit `context_manager.rs`, and Tasks 1–2 and 5 also
edit `chat.rs`. **No task may run beside #55, #49's reuse slice, #65 or #66**:
all of them edit `build_model_prompt`, and #65/#66 also edit the turn loop
(`proposal.md` §5.10; `docs/specs/README.md`, "Parallel work"). Nothing else in
flight edits `context_manager.rs`, `file_access.rs` or the four config files.

---

## Task 1: Categories, ranges and one renderer

**Requirements:** 4 (the shape provenances and ranges travel in); proposal §5.1,
§5.10. **Corrections:** C1, C3, C4, C15, C16, C17.
**Files:** `context_manager.rs`, `chat.rs`, `edit.rs`, create
`tests/context_assembly.rs`, update the four test files that read `item.kind`.

No budgeting and no collection change yet: this task changes the shape of an
item, makes the range expressible, makes one renderer the only place a header is
written, and closes the two places where unredacted text reaches the provider
(C17).

- [x] **Step 1: Write the failing tests** in
      `crates/workspace-engine/tests/context_assembly.rs`, with `temp_dir`,
      `write_fixture` and `test_config` copied from `foundation.rs:20-61`
      (`enable_index_watcher: false`, `data_dir` inside the fixture).
  - `a_range_less_item_renders_exactly_as_it_does_today` — a `RetrievedFile`
    with path `src/lib.rs` renders `\n--- retrieved_file: src/lib.rs ---\n…`
    byte for byte, and an item with no path renders `\n--- hook_context: hook:x ---\n…`;
  - `a_ranged_item_renders_its_line_range` —
    `--- retrieved_file: src/lib.rs:12-40 ---`, 1-based inclusive;
  - `the_prompt_orders_context_before_conversation_before_the_request` — the
    three section headings appear in that order exactly once each;
  - `the_user_request_appears_once` — a distinctive prompt string occurs exactly
    once in the built prompt (it is currently twice);
  - `a_secret_in_the_user_prompt_is_redacted_in_the_model_request` — seed a token
    the `SecretScanner` detects (the `AKIAIOSFODNN7EXAMPLE` style the eval
    fixtures use) into the prompt, build a turn with `MockModelAdapter`, and
    assert the request's user message contains the scanner's marker and not the
    raw token. **It fails today**, because nothing redacts the prompt on any path
    into the engine (`context.md` §2, "The user's prompt is not redacted on any
    path") — so it is a fix, not only a guard. Assert it for the edit flow too
    (`build_edit_prompt`), which has the same hole;
  - `a_secret_in_an_earlier_turn_is_redacted_in_the_recent_conversation_block` —
    turn one's prompt holds the token; turn two of the same session renders it
    with the marker, not the token, because stored messages are verbatim
    (spec 16) and the renderer must not replay them raw;
  - `the_chat_and_edit_prompts_render_items_identically` — `ContextPlan::render_item`
    is what both call, asserted by rendering the same plan through both builders;
  - `a_category_orders_before_the_categories_after_it` — `Ord` over the eleven
    variants is the pipeline order (one test, exhaustive array, no wildcard);
  - unit tests in `context_manager.rs` for the serde names (`snake_case` for
    categories and kinds, `camelCase` for item fields, `lineRange`).
  In the four existing files, replace `item.kind == "retrieved_file"` with
  `item.kind == ContextKind::RetrievedFile` etc.; the prompt-string assertions
  stay as they are.
- [x] **Step 2: Run them and confirm they fail** on the missing types:
      `cargo nextest run -p workspace-engine --test context_assembly`.
- [x] **Step 3: Implement.** Add the two enums (each with `as_str`, `ContextKind`'s
      returning today's exact spellings), the three new fields (each `None` until a
      later task fills it), `PromptSection`, `render_item` and `sections()`; make
      `build_model_prompt` and `build_edit_prompt` use it.
      Order the sections `Repository context:` → `Recent conversation:` →
      `User request:`, and render `Required` items under the final heading. The
      request comes **only** from the `Required` item, which `add_text` has
      already redacted; the `prompt` argument is not rendered at all, and there is
      no fallback to it. Pass each prior message's content through the orchestrator's
      `SecretScanner` before writing the conversation block (C17). Keep the item
      text and the trailing-newline behaviour identical.
- [x] **Step 4: Falsify.** (a) Render the request as a section *and* as a
      `Required` item → `the_user_request_appears_once` fails; (b) restore the
      conversation-first order → the order test fails; (c) change the range-less
      header at all → the byte-identical test fails; (d) render the raw `prompt`
      argument instead of the `Required` items → the prompt-secret test fails and
      the raw token appears in the request; (e) write the conversation block
      without the scanner → the replayed-secret test fails. Revert each.
- [x] **Step 5: Update the proposal** for C1, C3, C4, C15, C16 and C17 (already
      folded in; confirm §2, §5.1, §5.10 and §6 agree).
- [x] **Step 6: Scoped checks.** `cargo nextest run -p workspace-engine --test
      context_assembly --test foundation --test repository_map --test symbol_index
      --test hooks --test prompt_cache`, `cargo nextest run -p workspace-engine
      --lib -E 'test(context_manager)'`, scoped clippy for `workspace-engine`,
      and the fast checks. Then `cargo run -p eval-harness -- run --tier
      deterministic` — the prompt order moved, so this is the first baseline.

## Task 2: Collection sources and exclusion at collection time

**Requirements:** 5; proposal §5.3 (separation), §5.6, §5.11 (PlanState).
**Corrections:** C8, C10, C12, C13.
**Files:** `context_manager.rs`, `workspace_engine.rs`, `chat.rs`, `edit.rs`,
`tests/context_assembly.rs`, `tests/hooks.rs` (call-site signature only).

- [ ] **Step 1: Write the failing tests.**
  - `a_restricted_explicit_path_is_excluded_at_collection`,
    `a_restricted_prompt_mention_is_excluded_at_collection`,
    `a_restricted_keyword_hit_is_excluded_at_collection`,
    `a_restricted_semantic_hit_is_excluded_at_collection`,
    `a_restricted_instruction_path_is_excluded_at_collection` (fixture with
    `secrets/AGENTS.md` and a prompt mentioning `secrets/x.rs`),
    `a_restricted_project_rule_is_excluded_at_collection` (a fixture whose
    `restricted_patterns` names `README.md`), and
    `a_restricted_path_is_never_an_included_item` — each asserts a
    `RestrictedPath` record and no included item;
  - `every_collection_source_refuses_a_restricted_path` — walks every
    `CollectionSource` variant, no `_ =>` arm, and fails to compile when a
    variant is added without a case;
  - `keyword_hits_are_ordered_before_semantic_hits` and
    `a_keyword_hit_and_a_semantic_hit_for_one_path_become_one_item`;
  - `a_carried_plan_becomes_a_plan_state_item` (a resumed turn's plan renders
    its steps, and `PlanState` shares the item);
  - `the_symbols_item_keeps_the_lowest_provenance_it_contains`.
- [ ] **Step 2: Run them and confirm they fail.**
- [ ] **Step 3: Implement.** `ContextRequest`, `CollectionSource`, the
      collection-time `is_restricted` check per source, keyword/semantic kept
      apart, `PlanState` rendering from `SessionStore::read_task_plan`
      (`session.rs:1554`) read once in `chat.rs` before assembly. The turn's own
      mutable copy is still read inside `run_agentic_turn`
      (`chat.rs:2727-2738`) and is what the plan tools write through; the two
      cannot disagree because nothing writes a plan between them. Remove
      `build_context_with_hooks` and update its callers.
      `ContextManager::new` takes the `PathPolicy`.
- [ ] **Step 4: Falsify.** Remove the check from one source → its test fails;
      merge the two search lists → the ordering test fails; drop a
      `CollectionSource` arm → the exhaustive test fails to compile.
- [ ] **Step 5: Update the proposal** for C8, C10, C12, C13 (confirm §5.3, §5.6,
      §5.11 agree).
- [ ] **Step 6: Scoped checks.** `cargo nextest run -p workspace-engine --test
      context_assembly --test hooks --test foundation --test symbol_index
      --test repository_map`, scoped clippy for `workspace-engine`, fast checks.

## Task 3: Range deduplication and ranged reads

**Requirements:** 2; proposal §5.3 (FileRanges order), §5.4.
**Corrections:** C2, C5.
**Files:** `context_manager.rs`, `file_access.rs`, `tests/context_assembly.rs`,
`tests/agent_tools.rs`.

- [ ] **Step 1: Write the failing tests.**
  - `two_search_hits_in_one_function_send_one_merged_range` — one item, one
    range covering both, one read;
  - `ranges_two_lines_apart_merge_and_ranges_eight_apart_do_not` (the gutter is
    2; §7 records the reasoning);
  - `a_whole_file_item_absorbs_every_range_for_its_path`, and the absorbed ranges
    appear as `DeduplicatedIntoRange`;
  - `merging_keeps_the_language_server_provenance` (a `LanguageServer` range and
    a `Heuristic` one merge to `LanguageServer`);
  - `a_merged_range_longer_than_the_tool_read_cap_is_read_in_full` (a 600-line
    fixture range, against `max_read_lines` 400);
  - `a_context_range_read_still_refuses_a_restricted_path` and
    `a_context_range_read_still_redacts` (through `read_file`, not around it);
  - `every_absorbed_or_merged_range_is_recorded`.
- [ ] **Step 2: Run them and confirm they fail.**
- [ ] **Step 3: Implement.** Group by path, sort and merge with the gutter,
      whole-file absorption, provenance = the best of the parts, one
      `ReadWindow::ContextRange` read per merged range. Add the variant to
      `file_access.rs` and fix the exhaustive matches (`chat.rs:5333`,
      `damaian-cli/src/main.rs:169`, `tests/agent_tools.rs`).
- [ ] **Step 4: Falsify.** Gutter 0 → the merge test fails; gutter ∞ → the
      eight-lines-apart test fails; drop whole-file absorption → that test fails;
      use `ReadWindow::Range` → the 600-line test fails and truncates at 400.
- [ ] **Step 5: Update the proposal** for C2 and C5 (confirm §5.4's gutter and
      read paragraph).
- [ ] **Step 6: Scoped checks.** `cargo nextest run -p workspace-engine --test
      context_assembly --test agent_tools`, scoped clippy for `workspace-engine`
      and `damaian-cli`, fast checks.

## Task 4: Per-category budgets and the effective limit

**Requirements:** 1, 3, 7; proposal §5.2, §5.3, §5.7.
**Corrections:** C6, C7, C8 (the reserved categories), C14.
**Files:** `config.rs`, `profile.rs`, `effective_policy.rs`, `context_manager.rs`,
`workspace_engine.rs`, `chat.rs`, `edit.rs`, `tests/context_assembly.rs`,
`tests/permission_profiles.rs`.

- [ ] **Step 1: Write the failing tests.**
  - `each_category_stops_at_its_configured_share` — with a small limit, no
    category's `used` exceeds `allocated + what it borrowed`;
  - `an_unused_share_moves_to_later_categories` — with no plan and no findings,
    the file ranges get more than their nominal share and the manifest says so;
  - `a_category_over_its_share_drops_its_lowest_priority_members` — the
    `FileRanges` case: explicit, then mentioned, then implied; the dropped ones
    carry `CategoryBudgetExhausted`;
  - `semantic_hits_are_dropped_before_exact_matches`;
  - `the_most_specific_agents_md_survives_truncation` while the root one is
    dropped with `CategoryBudgetExhausted`, and the prompt still renders
    root-before-nested for whatever survived;
  - `a_block_item_too_large_for_its_share_is_rerendered_smaller` (a map that fits
    at 800 but not in its 5% share at a small limit appears, shortened, with a
    `Truncated` record);
  - `a_required_item_that_does_not_fit_is_an_error` — the assembly returns an
    error naming the limit, and the turn fails visibly rather than proceeding;
  - `shares_are_stored_in_basis_points_and_an_oversubscribed_set_is_a_config_error`;
  - `with_no_observed_profile_the_limit_is_the_config_budget_labelled_declared`
    — `tokens == context_token_budget()`, `source == Declared`,
    `safety_margin_bps == 0`;
  - `a_repository_may_lower_a_share_and_raise_the_margin` and
    `a_repository_cannot_raise_a_share_or_lower_the_margin` (the latter as a
    `WeakeningCase` in `tests/permission_profiles.rs`; a profile may only take
    the restrictive direction too).
- [ ] **Step 2: Run them and confirm they fail.**
- [ ] **Step 3: Implement.** `ContextBudget` from `Config`; parse
      `context_budget.<category>` and `context_budget.safety_margin` as decimals
      into basis points; classify both `Capability`; add the restrict-only merge
      beside the other numeric caps (`config.rs:1600-1650`); carry them through
      `profile.rs`, both `to_policy_text`s and `effective_policy.rs`;
      allocate, redistribute, truncate, and build `EffectiveLimit`.
      `build_context` now returns `Result<ContextPlan>`: a `Required` item that
      does not fit is `ClientError::InvalidInput` naming the limit, and both
      callers propagate it so the turn fails visibly. Replace Task 1's interim
      exemption that lets `add_text` keep a `Required` item regardless of budget.
- [ ] **Step 4: Falsify.** Strict caps without redistribution → the
      redistribution test fails; over-share stealing from the next category →
      the truncation test fails; accept a repository raise → the weakening case
      fails; a 10% margin on the declared path → the declared test fails.
- [ ] **Step 5: Update the proposal** for C6, C7, C8, C14 (confirm §5.2, §5.7,
      §5.11 and the restated acceptance criterion).
- [ ] **Step 6: Scoped checks.** `cargo nextest run -p workspace-engine --test
      context_assembly --test permission_profiles --test repository_config_trust
      --test foundation`, `cargo nextest run -p workspace-engine --lib -E
      'test(config)'`, scoped clippy for `workspace-engine` and `desktop-shell`,
      fast checks, and `npm run specs:check` (no spec status moved yet).

## Task 5: The manifest, its audit record, and the callers

**Requirements:** 6; proposal §5.5.
**Corrections:** C9, C11.
**Files:** `context_manager.rs`, `chat.rs`, `edit.rs`, `tests/context_assembly.rs`,
`tests/foundation.rs`.

- [ ] **Step 1: Write the failing tests.**
  - `the_manifest_lists_every_included_item_by_path_range_and_root`;
  - `the_manifest_names_the_reason_for_every_exclusion` — one fixture per reason
    that can be produced today (`RestrictedPath`, `CategoryBudgetExhausted`,
    `Truncated`, `DeduplicatedIntoRange`);
  - `no_manifest_field_contains_item_content` — seed a distinctive secret and a
    content marker into a fixture file, serialise the whole manifest to JSON, and
    assert neither appears; also assert the seeded secret is absent from the
    audit file;
  - `the_manifest_is_recorded_once_per_turn_as_context_assembled` — one event,
    `repositoryId`, `taskId`, `tokenEstimate`, the manifest JSON, and no content;
  - `the_edit_flow_records_its_manifest_too`;
  - `context_files_are_the_unique_included_paths_in_inclusion_order`.
- [ ] **Step 2: Run them and confirm they fail.**
- [ ] **Step 3: Implement.** Fill `ContextManifest` while assembling; put it in
      `ContextPlan`; record it from both callers next to `model_request_prepared`;
      keep `files` as the unique included paths.
- [ ] **Step 4: Falsify.** Put the item content in `IncludedRef` → the content
      test fails; stop recording → the audit test fails; drop exclusion records →
      the reason test fails.
- [ ] **Step 5: Update the proposal** for C9 and C11 (confirm §5.5).
- [ ] **Step 6: Scoped checks.** `cargo nextest run -p workspace-engine --test
      context_assembly --test foundation --test hooks`, scoped clippy for
      `workspace-engine`, fast checks, and `cargo run -p eval-harness -- run
      --tier deterministic` (the manifest and `files` are what the harness reads).

## Task 6: A content-aware estimate

**Requirements:** 8; proposal §5.8.
**Corrections:** none.
**Files:** `context_manager.rs`, `tests/context_assembly.rs`,
`tests/token_accounting.rs` (unchanged assertions, rerun only).

- [ ] **Step 1: Write the failing tests.**
  - `prose_code_and_json_get_different_estimates_for_the_same_length` — same
    character count, three content types, three different estimates;
  - `an_identifier_heavy_snippet_is_estimated_higher_than_prose_of_the_same_length`;
  - `the_estimate_is_monotone_in_length` and `never_zero_for_non_empty_content`;
  - `each_category_reports_whether_its_tokens_are_measured_or_estimated`
    (assembly is always `estimated`, per §5.8);
  - `#[ignore] calibration_compares_estimates_with_recorded_usage` with a doc
    comment giving the command (`DAMAIAN_DATA_DIR=… cargo nextest run -p
    workspace-engine --test context_assembly --run-ignored ignored-only -E
    'test(calibration_compares)'`):
    it reads `model_request_prepared.tokenEstimate` and `task_usage_recorded` for
    paired rounds in the data directory it is given and prints the ratio. It reads
    no key and writes nothing. Ignored because it inspects a real data directory.
  - `#[ignore] calibration_measures_bytes_per_token` — **the live half.** This
    task only *writes* it; Task 7 runs it. It sends three fixed documents —
    a prose paragraph, a Rust function, a JSON object, each ~1–2 kB so the reply
    is one short line — one per `ModelAdapter::stream_response` call to the
    configured provider, reads `ModelRun.usage.input_tokens` (which must be
    `Some(measured)`), and prints `chars / tokens` per document plus the ratio
    `len / 4` would have used. It asserts nothing about the values except that
    each reported usage is measured and each ratio is within a sane band, because
    an assertion on a measured ratio would fail on a different provider.
    - **Doc comment, per `AGENTS.md` "Tests with real side effects":** it calls a
      real model provider over the network, it spends the user's money, it is
      ignored by default, and it is run manually. Give the exact command, with a
      **separate** `DAMAIAN_DATA_DIR` so the run cannot touch the user's real
      state:
      `DAMAIAN_DATA_DIR=/tmp/damaian-calibration cargo nextest run -p
      workspace-engine --test context_assembly --run-ignored ignored-only -E
      'test(calibration_measures_bytes_per_token)'`.
    - **Key handling.** It resolves the key the way a normal non-desktop turn
      does — `env::var(&config.model_api_key_env)` from the config it loaded,
      exactly as `damaian-cli`'s `live_model_api_key` does
      (`damaian-cli/src/main.rs:694-703`) — and passes it to
      `CurlModelTransport`. It never prints, logs, stores or asserts on the key,
      adds no new variable, and does not read the Keychain (that module belongs to
      `desktop-shell`). The doc comment says so: a `keychain:` reference cannot be
      resolved from this crate at all, so that setup runs the test against a
      `DAMAIAN_DATA_DIR` whose user config names an environment variable, or
      measures the same three documents through the app instead.
    - **The three documents are fixtures in the test**, not files on disk: no
      repository, no index, and no write to the data directory beyond what
      `Config`/`ProcessRegistry` already do.
- [ ] **Step 2: Run them and confirm they fail.** (`cargo nextest run -p
      workspace-engine --test context_assembly` — the ignored calibration tests
      are reported as skipped, which is the confirmation they compile.)
- [ ] **Step 3: Implement.** A `TokenEstimate` with a ratio per content type
      chosen from `kind` + file extension, replacing the `div_ceil(4)` inside
      `add_text` only. **Do not change `ModelAdapter::estimate_tokens`
      (`model.rs:354`)**: that one accounts for a provider that reports no usage
      (spec 19), it measures the whole request body, and changing it moves a
      different spec's numbers.
- [ ] **Step 4: Falsify.** One ratio for every type → the three-type test fails;
      an estimate that ignores length → monotonicity fails; a zero estimate → the
      non-empty test fails. The two calibration tests are not falsified here —
      they are measurements, not guards, and Task 7 records what they reported.
- [ ] **Step 5: Update the proposal** §5.8 with the provisional constants and say
      plainly that Task 7 replaces them with measured values.
- [ ] **Step 6: Scoped checks.** `cargo nextest run -p workspace-engine --test
      context_assembly --test token_accounting --test foundation` (which shows the
      two ignored calibration tests as skipped), scoped clippy for
      `workspace-engine`, fast checks.

## Task 7: Calibration, docs and close-out

**Requirements:** 8's calibration, the docs half of acceptance, and
`AGENTS.md`'s "When a spec becomes Done".
**Proposal:** §5.8, §5.9, §7.
**Files:** `docs/USER_GUIDE.md`, `docs/TROUBLESHOOTING.md`, `proposal.md` §7,
`docs/specs/README.md`, `CHANGELOG.md`, this file, every spec whose `Depends on:`
names #26.

- [ ] **Step 1: Offline calibration (no key, run by the agent against a data
      directory the human names).** Run
      `calibration_compares_estimates_with_recorded_usage` from Task 6 over a
      `DAMAIAN_DATA_DIR` the human points at. Record the ratio distribution, and
      whether the new estimate is closer than `len / 4` (spec 19 §7's 2.5–6.3%
      high is the comparison). If the directory has no usable pairs, say so and
      do not invent a number.
- [ ] **Step 2: Live calibration — the human runs it, or explicitly approves the
      agent running it.** This is the only step that needs the provider, and it is
      separate because it spends the user's money and uses their key. Either
      route is acceptable:
  - **Human-run:** ask the human to run
    `DAMAIAN_DATA_DIR=/tmp/damaian-calibration cargo nextest run -p workspace-engine --test context_assembly --run-ignored ignored-only -E 'test(calibration_measures_bytes_per_token)'`
    (Task 6's test, with its own doc comment).
  - **Agent-run on explicit approval:** ask in chat first and run it only after a
    clear yes for *this* live run. The key is resolved by the engine from
    `config.model_api_key_env` exactly as for a turn — the agent never reads,
    echoes, logs or stores it, and never passes it on a command line or in an
    environment assignment it types. If the approval is not given, take the
    human-run route or record the half as outstanding.
  - Either way the test sends three fixed fixture documents — a prose paragraph,
    a Rust function, a JSON blob — one per call, reads the provider's reported
    input tokens, and prints `chars / tokens` for each;
  - record the three ratios, the provider and model, and how far `len / 4` was in
    §7; replace Task 6's provisional constants with them;
  - if the human declines both routes, record in §7 that the per-content-type half
    is outstanding, keep the provisional constants with their derivation, and say
    so in the Progress row. Do not claim the calibration acceptance in full.
- [ ] **Step 3: Docs** (§5.9). `USER_GUIDE.md`'s **Output and Context Limits**:
      the pipeline order, what each category is, the `context_budget.*` keys with
      their defaults and "lower only from repository config", and what the
      conversation/request order now is. `TROUBLESHOOTING.md`: a new section
      "Reading a turn's context manifest" — where the `context_assembled` event
      is, what each `ExclusionReason` means, observed vs declared limits, and what
      to do when content is `CategoryBudgetExhausted` (raise the share, name the
      file in the prompt, or lower a competing category). Add a row to
      TROUBLESHOOTING's symptom index.
- [ ] **Step 4: Acceptance run.** `cargo run -p eval-harness -- run --tier
      deterministic`: all scenarios pass, and every metric is compared by hand
      against `evals/baseline.json` (spec 18 requirement 7). A metric that moved
      is explained in §7 or the run is a regression; a moved completion rate is
      the acceptance criterion failing, not a note. Record the result and the
      date in §7.
- [ ] **Step 5: Close the spec** per `AGENTS.md`, in one change:
      proposal `Status:` → Done with the date and the acceptance evidence;
      README's implementation-order row and **Current state** bullets; every
      `Depends on:` line naming #26 (search
      `grep -rn "26_context_assembly" docs/specs/*.md docs/specs/*/*.md`) → built;
      "What to build next" re-derived, which promotes #27, #54, #55, #33, #51 and
      #49's reuse slice; remove the `Unreleased` item ("Allocate context by
      category with a record of what was excluded and why") from `CHANGELOG.md`;
      this file's Progress rows and header.
- [ ] **Step 6: Checks.** `scripts/quality-gate.sh fmt node lint typos deny`,
      `npm run specs:check -- --strict`, scoped clippy and the named test files.
      The full seven-check gate runs on CI; record its result here when it is
      green, and do not call the work complete before that.
