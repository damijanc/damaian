# Feature Spec: Symbol and Relationship Index

Status: Done 2026-10-23 (started 2026-10-13). Tasks 1–11 of 11 done; see
[`tasks.md`](tasks.md). §7 records the acceptance evidence, the measurements,
the three `chat.rs` pieces that remain deferred, and where the eager definition
pass starts (closed by the Task 11 follow-up). Split into a folder and planned on 2026-10-09, re-checked
against `main` on 2026-10-12. Design unchanged from the original flat spec. Corrections where it no
longer matches the code are in [`context.md`](context.md), not inlined here,
the way specs 22, 24 and 31 kept theirs. The most important ones:
- the process registry this spec needs is spec 46's, and it is built; the MCP
  stdio transport is the model for spawning and teardown but not for framing,
  because LSP frames with `Content-Length` and MCP with newlines;
- heuristic symbols are already extracted in the index pass, so persistence
  pays off for language-server results, not for re-extraction;
- sessions have no close event, so a language server lives for the Damaian
  process (or an idle timeout) rather than "until the last session closes";
- `chat.rs` is out of bounds for this spec, so the model reaches symbols
  through a context item, not a new tool, and two pieces that need the chat
  loop are deferred and named (`context.md` §12).

Read `context.md` before starting any task in [`tasks.md`](tasks.md).
Order: 25 of 27
Plan: `docs/PLAN/03_phase_3_code_understanding.md`, Phase 3, Work
Package 3 (Must). That directory is local-only and not committed, so the
reference is a name rather than a link; this spec is self-contained.
Depends on: [#24](../24_repository_map_and_monorepo_boundaries/proposal.md) (the repository
map) — built (Done 2026-10-09). Everything else named below is a cross-reference, not a
prerequisite.
Related spec sections: `ai_coding_assistant_specification.md` section 7.2
(project indexer), section 19 (recommended technology direction). Related
implementation specs: [`02_semantic_search.md`](../02_semantic_search.md),
[`05_clickable_file_references.md`](../05_clickable_file_references.md)
(navigation), [`22_findings_model_and_panel/proposal.md`](../22_findings_model_and_panel/proposal.md)
(LSP diagnostics land as an additional `Finding` source here — this is where
Phase 2's deferred LSP dependency is satisfied),
[`24_repository_map_and_monorepo_boundaries/`](../24_repository_map_and_monorepo_boundaries/proposal.md)
(supplies the roots symbols are namespaced by).

## 1. Motivation

**Correction to the roadmap first, because it changes the shape of this work
package.** The roadmap states that `language.rs` "identifies languages; it does
not parse them" and that there is "no symbol index". That is not accurate. A
heuristic symbol layer exists and is populated on every index:

- `language.rs::extract_symbols` (`crates/workspace-engine/src/language.rs:29`)
  extracts declaration names for JavaScript, TypeScript, Python, Go, Rust, Java,
  Kotlin, and PHP by matching line prefixes.
- `language.rs::extract_imports` (`language.rs:69`) extracts import targets for
  the same languages.
- `FileRecord` carries `symbols: Vec<String>` and `imports: Vec<String>`
  (`crates/workspace-engine/src/indexer.rs:29-30`), populated at
  `indexer.rs:357-358`.

So this is a **rescope**, like Phase 3 WP1 was: extend an existing heuristic
layer rather than build a symbol index from nothing.

What the existing layer cannot do is the actual requirement. `symbols` is a
`Vec<String>` of bare names:

- **No location.** WP3's first acceptance criterion is "searching for a symbol
  returns its definition with an exact file and line". A name with no line
  cannot satisfy it. The file is known; the line was discarded.
- **No kind.** `extract_symbols` matches `"pub fn "`, `"struct "`, `"enum "`,
  `"class "`, `"interface "` — it knows the kind at the moment it matches and
  throws it away, returning only the name.
- **No relationships.** Imports are raw strings (`"./upload"`, `"serde"`),
  never resolved to files, so there is no dependency edge. No references, no
  implementations, no inheritance, no source-to-test association.
- **No confidence.** Everything is a heuristic guess, and nothing says so.
- **Accuracy limits the naive matching cannot escape.** Prefix matching is
  line-based, so it matches inside comments and string literals, misses
  multi-line declarations, and for Rust misses every method — `impl` blocks are
  not matched, so `fn` inside one is either missed or recorded without its
  type.

The last point is why language servers are in the requirements at all: the
existing heuristics are a reasonable floor and a poor ceiling.

## 2. Current State

- **Heuristic symbol and import extraction exists**, as above:
  `language.rs:29-100`, called from `indexer.rs:357-358`, stored on
  `FileRecord` (`indexer.rs:22-33`).
- **`detect_language`** (`language.rs:3`) maps path to language name.
- **Search is keyword overlap plus embeddings.** `RepositoryIndex::keyword_search`
  (`indexer.rs:61`) scores over `FileRecord.terms`; semantic search is
  [spec 02](../02_semantic_search.md)'s embedding index.
- **`FileRecord.symbols` is searchable only as text**, through the same term
  scoring as file content. There is no symbol lookup by name.
- **No language server exists.** No LSP client, no server process management, no
  diagnostics ingestion.
- **Per-repository persistence has a working pattern.**
  `vector_index.rs` writes `<data_dir>/vector-index/<repository_id>.bin`
  (`vector_index.rs:149-152`) with `load` and `save` (`vector_index.rs:22-40`),
  and reuses cached vectors keyed by `(path, ordinal)`. It has **no schema
  version**, which this spec does not copy.
- **The keyword index is in-memory only** and rebuilt per launch; persistence is
  Phase 3 WP1, which is Should-tier and **not in this phase's minimum slice**.
- **Process tracking for spawned children is specified but not built.**
  [Spec 17](../17_durable_task_state_and_crash_recovery/proposal.md) §5.7 defines the
  session-scoped PID registry, driven by MCP stdio servers and the `curl` model
  child. Language servers are a third client of it.
- **`Finding`** ([spec 22](../22_findings_model_and_panel/proposal.md)) declares
  `FindingSource::LanguageServer` already, unused until this work package.

## 3. Requirements

1. Use language servers where available; fall back to syntax parsing or
   conservative heuristics otherwise.
2. Capture definitions and declarations; symbol kind and location; imports and
   exports; references where reliable; implementations and inheritance where
   available; file and package dependencies; and source-to-test associations.
3. **Record the source and confidence of every relationship**, so a heuristic
   guess and a language-server answer are distinguishable downstream.
4. Heuristic relationships are never presented as authoritative, in the UI or in
   model context.
5. Unavailable, slow, or crashing language servers degrade to heuristics and
   never block a task.
6. Symbol data is local and versioned.
7. Symbol results integrate with search and with the clickable file references
   from [spec 05](../05_clickable_file_references.md).
8. LSP diagnostics feed [spec 22](../22_findings_model_and_panel/proposal.md)'s `Finding`
   model as an additional source.
9. Language server processes are tracked by PID and terminated by PID, per
   `AGENTS.md`.

## 4. Non-goals

- Writing a language server, or a full parser for any language. Where a language
  server is unavailable, the fallback stays heuristic — improved, but still
  heuristic and still labelled as such.
- Bundling or installing language servers. Damaian uses one if it is on the
  user's machine and configured; it never downloads a binary.
- Supporting every LSP capability. §5.4 names the small set used, and everything
  else in the protocol is ignored.
- Whole-repository reference indexing via LSP. §5.5 explains why references are
  resolved on demand rather than crawled.
- Cross-language relationship resolution (a TypeScript file calling a Rust
  binary).
- Replacing keyword or semantic search. Symbols supplement both.
- Persisting the keyword index — Phase 3 WP1, Should-tier, not in this slice.
- Tree-sitter or any new parser dependency. §5.3 improves the existing
  prefix heuristics within their own approach rather than adding a grammar
  toolchain, which is its own work package if it is ever justified.

## 5. Design

### 5.1 The symbol record

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SymbolKind {
    Function, Method, Struct, Enum, Class, Interface, Trait,
    Type, Constant, Variable, Module, Unknown,
}

/// How a symbol or relationship was derived. Requirement 3.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Provenance {
    /// Reported by a language server. Authoritative.
    LanguageServer,
    /// Derived from source text by Damaian's own matching. A guess.
    Heuristic,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Symbol {
    pub name: String,
    pub kind: SymbolKind,
    /// Repository-relative path, plus the line the declaration starts on.
    pub path: String,
    pub line: u32,
    pub column: Option<u32>,
    /// Enclosing symbol, where known: the type a method belongs to.
    pub container: Option<String>,
    /// Nearest enclosing project root, from spec 24.
    pub root_path: String,
    pub provenance: Provenance,
}
```

`Provenance` is an enum of exactly two values rather than a numeric confidence
score. A score invites arithmetic that has no meaning — a 0.7 heuristic is not
comparable to a 0.9 heuristic in any calibrated way — and requirement 4's real
demand is a binary distinction: may this be presented as fact, or must it be
hedged. Two values answer that; a float does not.

`Ord` is derived so `LanguageServer` sorts before `Heuristic`, which is how
§5.6 merges the two sources.

### 5.2 Relationships

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RelationKind {
    Imports,        // file → file or file → external package
    References,     // symbol → symbol
    Implements,     // type → trait/interface
    Extends,        // type → supertype
    TestOf,         // test file/symbol → subject
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Relation {
    pub kind: RelationKind,
    pub from: SymbolRef,
    pub to: RelationTarget,
    pub provenance: Provenance,
}

/// An import may not resolve to a file in this repository.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum RelationTarget {
    /// Resolved to a path inside the repository.
    File { path: String },
    Symbol(SymbolRef),
    /// An external dependency, kept as the raw specifier.
    External { specifier: String },
}
```

`RelationTarget::External` is the honest representation of most imports.
`extract_imports` returns `"serde"` and `"./upload"` alike (`language.rs:69-100`);
only the second can become a file edge. Collapsing both into a path would
invent edges to files that do not exist.

**Import resolution** turns a specifier into a `File` target where it can:
relative specifiers (`./`, `../`) resolve against the importing file's directory,
trying the language's conventional extensions and index files. Anything else —
a bare package name, an alias from a `tsconfig` path mapping, a Rust crate name
— stays `External`. Resolving alias mappings would mean parsing build
configuration, which §4 rules out; an unresolved specifier is recorded, not
guessed at.

**`TestOf`** is derived from the test paths in
[spec 24](../24_repository_map_and_monorepo_boundaries/proposal.md)'s map plus naming
convention (`upload.rs` ↔ `tests/upload.rs`, `foo.ts` ↔ `foo.test.ts`). It is
always `Heuristic`, because a naming convention is a convention.

### 5.3 The heuristic tier, improved

Requirement 1's fallback is the existing `extract_symbols` approach, kept and
improved within itself — no new parser dependency:

- **Record the line number.** `extract_symbols` already iterates
  `content.lines()` (`language.rs:31`); the index is available and discarded.
  This one change is what makes the acceptance criterion reachable in the
  fallback tier.
- **Record the kind.** Each `collect_after_prefix` call knows which prefix it
  matched (`language.rs:36-63`); map each to a `SymbolKind` instead of pushing a
  bare name.
- **Skip comments and strings.** Track whether a line is inside a line comment,
  a block comment, or a raw string, and skip declaration matching there. This
  removes the largest class of false positives — a commented-out function is
  currently indexed as a definition.
- **Handle Rust `impl` blocks.** Track the current `impl` target so `fn` inside
  one is recorded as `Method` with `container` set, instead of being missed or
  recorded as a free function. Rust is this repository's own primary language,
  and methods being invisible is the most consequential current gap.

These stay `Provenance::Heuristic`. Improving a heuristic does not make it
authoritative, and the label is what requirement 4 rests on.

### 5.4 The language-server tier

An optional, per-language, user-configured LSP client:

```text
language_server.rust=rust-analyzer
language_server.typescript=typescript-language-server --stdio
```

Unconfigured means unused: with no configuration, Damaian behaves exactly as the
heuristic tier, so this work package ships useful without any user setup.

Only these LSP requests are used, and nothing else in the protocol is
implemented:

| Request | Supplies |
|---|---|
| `textDocument/documentSymbol` | Definitions with kind, location, container |
| `textDocument/definition` | Symbol → definition, on demand |
| `textDocument/references` | References, on demand (§5.5) |
| `textDocument/implementation` | `Implements` relations |
| `textDocument/publishDiagnostics` (notification) | Findings, per requirement 8 |

Transport is stdio over a spawned child process, which is the same shape as the
MCP stdio transport (`crates/workspace-engine/src/mcp.rs:288-345`) — that code
is the model to follow for spawning, framing, and reader-thread teardown rather
than a second independent implementation.

### 5.5 References are on demand, not crawled

Requirement 2 asks for "references where reliable". A whole-repository reference
index would mean a `textDocument/references` request per symbol, which on a
large repository is thousands of round trips against a server that is often the
slowest component in the system, for data that is stale as soon as a file
changes.

References are therefore resolved **on demand** — when the user or the agent
asks about a specific symbol — and cached with the file's content hash. A cached
reference set whose file hash no longer matches is discarded rather than
returned.

Definitions, by contrast, come from `documentSymbol` per file and are indexed
eagerly, because that is one request per file and is what search needs.

### 5.6 Merging the two tiers

Both tiers can produce a symbol for the same declaration. The rule:

- A language-server symbol supersedes a heuristic symbol with the same name,
  path, and approximate line. `Provenance`'s `Ord` makes this a sort-and-dedupe
  rather than special-case logic.
- A heuristic symbol with no language-server counterpart is kept, labelled
  `Heuristic`. Losing it would mean a configured language server makes some
  symbols *disappear* — an LSP that indexes only `src/` would hide everything
  outside it.
- A language server that is unconfigured, absent, slow, or crashed contributes
  nothing, and the heuristic tier stands alone. This is requirement 5: the
  degradation path is the normal path with one input missing, not an error state.

### 5.7 Presenting provenance

Requirement 4 applies in two places, and both need to be explicit or it will be
satisfied in neither:

- **UI**: a heuristic symbol or relationship is marked — an icon or a "likely"
  qualifier — and hovering explains that it was derived from source text rather
  than a language server. Clickable references
  ([spec 05](../05_clickable_file_references.md)) work for both, since a heuristic
  line number is still a line number.
- **Model context**: heuristic items are labelled in the rendered context, e.g.
  `upload_retry (function, src/upload.rs:42, heuristic — may be inaccurate)`.
  This is the part most likely to be skipped, and it is the part that matters:
  an agent told a relationship is certain will act on it, and a wrong `Implements`
  edge sends it to rewrite the wrong type. [Spec 26](../26_context_assembly/proposal.md)
  carries the label through as provenance on the context item.

### 5.8 Process lifetime

Language servers are long-lived children, so requirement 9 uses the registry
from [spec 17](../17_durable_task_state_and_crash_recovery/proposal.md) §5.7: PID and start
time recorded at spawn, killed by PID, with the start-time check before killing
so a recycled PID is never someone else's process.

One server per language per repository, shut down when the last session using
that repository closes. A crashed server is not restarted automatically more
than once per session — a server that crashes repeatedly on a particular file
would otherwise be restarted in a loop, and the heuristic tier already covers
its absence.

`AGENTS.md`'s rule against matching processes by name applies with particular
force here: `rust-analyzer` is very likely to be running as the user's own
editor process, and killing it by name would kill their editor.

### 5.9 Diagnostics as findings

`publishDiagnostics` notifications map to `Finding`
([spec 22](../22_findings_model_and_panel/proposal.md)) with
`source: FindingSource::LanguageServer`, severity from the LSP severity, and
`range` from the LSP range. This satisfies requirement 8 and Phase 2's deferred
LSP dependency.

Diagnostics arrive asynchronously and unsolicited, unlike every other finding
source, which are produced by a command Damaian ran. Two rules follow:

- Diagnostics are attributed to the task active when they arrive, or to no task
  when none is. A diagnostic with no task is still shown in the panel.
- They do not, on their own, block a plan step
  ([spec 21](../21_task_plan_progress_and_budget/proposal.md)). A step is blocked by a check
  that ran and failed; a language server reporting an error mid-edit is normal
  and transient, and treating it as a blocking failure would stall the loop on
  every intermediate state.

### 5.10 Persistence

```text
<data_dir>/symbol-index/<repository_id>.json
```

With `schema_version` and per-file content hashes, following the vector-index
pattern (`vector_index.rs:149-152`) and adding the schema version that file
lacks. A version mismatch or corrupt file rebuilds.

Symbols are keyed by file and content hash, so an unchanged file's symbols are
reused and only changed files are re-extracted — the same incremental approach
`vector_index.rs` uses for embeddings.

Because the keyword index is not persisted (Phase 3 WP1, not in this slice), a
launch still pays a full content scan; symbols persisting independently means
that scan does not also pay for re-extraction. This spec does not depend on WP1
shipping.

### 5.11 Search integration

A symbol lookup path: exact name match, then prefix, then fuzzy, returning
`Symbol` records ordered by provenance then match quality. Requirement 7's
acceptance criterion — a symbol search returns a definition with an exact file
and line — is satisfied by this path, in both tiers.

Symbol hits also feed [spec 26](../26_context_assembly/proposal.md) as a distinct context
category with its own budget, so symbol results do not crowd out file content.

## 6. Acceptance Criteria

- Searching for a symbol returns its definition with an exact file and line, in
  both the language-server tier and the heuristic-only tier.
- Symbol kind is recorded, and a Rust method inside an `impl` block is recorded
  as `Method` with its container.
- A declaration inside a comment or a string literal is not indexed as a symbol
  — asserted by fixture.
- Every symbol and relationship carries `Provenance`, and a heuristic-derived
  item is labelled as such in the UI **and** in rendered model context —
  asserted by test on the rendered context string.
- An unresolvable import is recorded as `External` with its raw specifier, never
  as a file edge.
- A relative import resolves to a repository file where one exists.
- Killing a language server mid-task degrades to heuristics without failing the
  task, and the task completes.
- With no language server configured, symbol extraction works and every item is
  `Heuristic`.
- A language server that indexes only part of the repository does not cause
  heuristic symbols elsewhere to disappear.
- A repeatedly crashing language server is not restarted in a loop.
- No language server process outlives the session that started it, and a
  recorded PID whose start time no longer matches is not killed — asserted by
  test.
- LSP diagnostics appear as `Finding`s with `source: language_server` and a
  range, and do not block a plan step on their own.
- References are resolved on demand and a cached set is discarded when the
  file's content hash changes.
- A corrupt or version-mismatched symbol index rebuilds cleanly and reports it.
- Symbol accuracy fixtures pass for each supported language.
- Every quality-gate command from `AGENTS.md` passes, and the
  [spec 18](../18_local_evaluation_harness/proposal.md) baseline shows no
  regression.

## 7. Implementation Notes

Written in Task 11 (2026-10-23). The per-task record is
[`tasks.md`](tasks.md)'s progress table; the decisions that replaced parts of
§5 are in [`context.md`](context.md).

### 7.1 Acceptance criteria and the tests that prove them

Engine tests are in `crates/workspace-engine/tests/symbol_index.rs` unless
another file is named. The pure LSP protocol tests are in `lsp.rs`'s
`#[cfg(test)]` module; shell tests are in the `tests` module of
`crates/desktop-shell/src/lib.rs`.

| §6 criterion | Proved by | State |
|---|---|---|
| Searching for a symbol returns its definition with an exact file and line, in both tiers | Heuristic: `a_lookup_returns_the_definition_at_its_exact_path_and_line`, `the_engine_finds_symbols_from_the_stored_index`, shell `symbols_endpoint_serves_matches_with_provenance_and_their_root`. Language server: `a_server_that_covers_one_file_supersedes_it_and_heuristics_elsewhere_survive`, and the app path `a_symbol_search_starts_the_pass_without_waiting_and_the_next_read_sees_it` | Met. `/api/symbols` calls `WorkspaceEngine::symbol_search`, which reads the current snapshot and starts the eager pass; the first response is heuristic and the next read sees the server's definition (§7.4) |
| Symbol kind is recorded, and a Rust method inside an `impl` block is `Method` with its container | `rust_declarations_match_the_fixture` (the fixture's `impl` methods are expected as `Method` with their type), `typescript_declarations_match_the_fixture` … `php_declarations_match_the_fixture`, `heuristic_accuracy_per_language` | Met |
| A declaration inside a comment or a string literal is not indexed — asserted by fixture | All eight `*_declarations_match_the_fixture` tests; each fixture contains comment, block-comment and string declarations that its expected table omits | Met |
| Every symbol and relationship carries `Provenance`, and a heuristic item is labelled in the UI **and** in rendered model context | `with_no_language_server_every_symbol_and_relation_is_heuristic` (types and every built item); `a_prompt_naming_a_symbol_gets_one_hedged_symbols_item_after_the_map` (the rendered context string contains `heuristic — may be inaccurate`); `a_language_server_symbol_is_rendered_without_the_hedge`. The UI's `likely` marker and its hover text were verified by hand against the running shell in Task 10 | Met. The UI half has **no automated test**: the web UI has no JS test harness, the same limit spec 24 recorded for its chips |
| An unresolvable import is `External` with its raw specifier, never a file edge | `relative_imports_resolve_to_indexed_files_and_the_rest_stay_external` | Met |
| A relative import resolves to a repository file where one exists | `relative_imports_resolve_to_indexed_files_and_the_rest_stay_external` | Met |
| Killing a language server mid-task degrades to heuristics without failing the task, and the task completes | `a_server_that_exits_halfway_stops_the_pass_and_nothing_else` (a mock-model turn started during the pass completes), `a_server_that_crashes_twice_is_restarted_once_and_then_left_down`, `a_server_that_never_answers_costs_one_timeout_per_file_and_never_delays_a_reader` | Met |
| With no language server configured, extraction works and every item is `Heuristic` | `with_no_language_server_every_symbol_and_relation_is_heuristic`; `with_no_language_server_configured_nothing_is_launched_or_recorded` | Met |
| A server that indexes part of the repository does not make heuristic symbols elsewhere disappear | `a_server_that_covers_one_file_supersedes_it_and_heuristics_elsewhere_survive`, `the_same_name_in_another_file_is_not_superseded` | Met |
| A repeatedly crashing server is not restarted in a loop | `a_server_that_crashes_twice_is_restarted_once_and_then_left_down` | Met |
| No server process outlives the session that started it, and a PID whose start time no longer matches is not killed | `a_running_server_has_one_registry_entry_until_shutdown_all`, `an_idle_server_is_stopped_and_its_entry_removed`, `a_language_server_entry_whose_start_time_does_not_match_is_not_killed` | Met with `context.md` §3's restatement: sessions have no close event, so a server lives for the Damaian process or ten idle minutes, and the shell calls `shutdown_all` on exit. The Tauri app's own exit path does not call it; EOF on stdin usually ends a server, and the next launch's sweep covers the rest (§7.4) |
| LSP diagnostics appear as `Finding`s with `source: language_server` and a range, and do not block a plan step | `diagnostics_are_recorded_once_with_one_based_ranges_and_a_clearing_set_removes_nothing`, `a_diagnostic_on_a_restricted_file_or_outside_the_repository_is_never_recorded`, `a_language_server_error_leaves_the_active_plan_step_unchanged`, shell `get_findings_returns_a_language_server_diagnostic`, and the app path `a_symbol_search_lets_a_configured_server_publish_a_finding` | Met. The pass sends `didOpen` before `documentSymbol`, the hub routes the diagnostic, and `GET /api/findings` records it through `record_language_server_findings` with no task (§7.4) |
| References are resolved on demand and a cached set is discarded when a file's hash changes | `references_are_language_server_relations`, `a_second_reference_call_is_served_from_the_cache`, `editing_a_referencing_file_discards_the_cache_and_asks_again`, `implementations_are_language_server_relations`, `without_a_language_server_references_are_unavailable_and_not_guessed` | Met |
| A corrupt or version-mismatched index rebuilds cleanly and reports it | `a_garbage_index_file_is_rebuilt_as_corrupt_and_audited`, `an_index_from_another_schema_version_is_rebuilt_as_a_mismatch`, `an_index_of_another_repository_is_rebuilt_as_corrupt` | Met |
| Symbol accuracy fixtures pass for each supported language | `rust_declarations_match_the_fixture`, `typescript_declarations_match_the_fixture`, `javascript_declarations_match_the_fixture`, `python_declarations_match_the_fixture`, `go_declarations_match_the_fixture`, `java_declarations_match_the_fixture`, `kotlin_declarations_match_the_fixture`, `php_declarations_match_the_fixture` | Met |
| Every quality-gate command passes, and the spec 18 baseline shows no regression | The Task 11 scoped runs recorded in `tasks.md`; the full seven-command gate is CI's job under `AGENTS.md` | **Pending CI.** Scoped checks and the deterministic eval tier pass locally: 19/19 scenarios, 268,808 estimated tokens against Task 8's 268,839 and the 101,963 baseline of 2026-09-18, whose staleness Task 8 documented |

### 7.2 Recording questions

**Language servers tested: none.** Checked again on 2026-10-23:
`~/.cargo/bin/rust-analyzer` is still the rustup proxy and answers "Unknown
binary 'rust-analyzer' in official toolchain"; `typescript-language-server`,
`pyright-langserver` and `gopls` are not on `PATH`. Installing one is the user's
decision (`context.md` §13), so **`documentSymbol` latency is not measured**, and
neither is the full pass's wall time or a warm launch against a cold one. The
engine is tested against the re-executed fake server only. Whether eager
definition indexing is viable on a medium repository is therefore **unresolved**
by this spec.

**Heuristic accuracy, by name** (progress row 1). Precision is 1.00 in every
language after Task 1 because comment and string skipping removed the false
positives; extra names are 0 everywhere except Rust's one pinned
`macro_rules!` body. Rust recall moved materially, 0.50 → 1.00, through
visibility and qualifier stripping, the added keywords and `impl` tracking. The
other languages' recall did not move; their misses are prefix gaps (modifiers,
`async def`, TS `type`/`enum`, Go receivers, class methods), not parsing
problems.

| Language | Found before | Extra before | P / R before | Found after | Extra after | P / R after |
|---|---|---|---|---|---|---|
| Rust | 11/22 | 5 | 0.69 / 0.50 | 22/22 | 1 | 0.96 / 1.00 |
| TypeScript | 8/12 | 5 | 0.62 / 0.67 | 8/12 | 0 | 1.00 / 0.67 |
| JavaScript | 8/10 | 5 | 0.62 / 0.80 | 8/10 | 0 | 1.00 / 0.80 |
| Python | 4/6 | 3 | 0.57 / 0.67 | 4/6 | 0 | 1.00 / 0.67 |
| Go | 4/7 | 2 | 0.67 / 0.57 | 4/7 | 0 | 1.00 / 0.57 |
| Java | 1/4 | 2 | 0.33 / 0.25 | 1/4 | 0 | 1.00 / 0.25 |
| Kotlin | 3/5 | 2 | 0.60 / 0.60 | 3/5 | 0 | 1.00 / 0.60 |
| PHP | 3/4 | 2 | 0.60 / 0.75 | 3/4 | 0 | 1.00 / 0.75 |

**Symbol index size.** Task 3 measured this repository on 2026-10-15 in a
release build: 309 indexed files, 4,743 symbols, 851 relations; the stored JSON
is **1,632,054 bytes**, `Built` takes 3.3–4.2 ms and `Reused` 7.9–8.1 ms. Reuse
is slower than a build because parsing 1.6 MB costs more than re-deriving the
heuristics, which confirms `context.md` §4.3: persistence earns its keep for
language-server results, not for heuristic ones. **The vector index's size for
the same repository is not measured**: semantic search is off by default and
nothing builds a vector index unless it runs, so no comparable file exists in
this checkout's isolated data directory.

### 7.3 The three `chat.rs` pieces, not met for the agent

The three `chat.rs` pieces named in `context.md` §12 are **not met for the
agent**, because #25 must not edit `chat.rs` or `build_model_prompt`:

1. **An agent-facing symbol tool.** The user can search symbols and ask for
   references through the shell and the CLI; the agent can see definitions in
   the `symbols` context item but cannot request references or implementations.
   Requirement 2's references are therefore met for the user, not for the agent.
2. **Diagnostics attributed to the active task.** They are recorded with no task
   id, which §5.9 allows when no task is active, but they are never tied to one.
3. **Symbols in `search_codebase`'s result text.** Not needed by any acceptance
   criterion; `find_symbols` satisfies the search criterion.

One follow-up `chat.rs` task adds items 1–3, sequenced against whichever
`chat.rs` spec is in flight. It is not started.

### 7.4 Where the eager pass starts

Closed on 2026-10-23 by the Task 11 follow-up. `WorkspaceEngine::symbol_search`
is the production entry point: it reads `find_symbols`'s snapshot,
then calls `start_definition_pass_with` with the process-wide hub and the
`symbol-search` session id. `handle_symbols` uses it, so the **first
`GET /api/symbols` for a repository starts the pass**. §8 of `context.md`
requires the pass to run on the hub's thread and never be awaited by a reader;
Task 6's note named repository open or the first symbol request. The first
symbol request is the least surprising of those:

- opening a repository, or running a turn, does not spawn a language server;
  the user asks for symbol data first;
- the request answers from the snapshot it already read, so the first search
  returns at once (heuristic until the pass lands) and the next read sees the
  server's definitions;
- `PassClaim` still allows one pass per repository at a time, and a second
  search while one runs returns its snapshot without starting another;
- no `language_server.<language>` means `start_definition_pass_with` returns
  `None` before claiming or spawning anything;
- the caller is reached only after `symbol_repository` has canonicalised the
  root and checked `allowed_roots`, and the pass keeps its own restrictions:
  restricted files are skipped, repository config cannot name a server, and a
  failure to start is swallowed because it only costs the next read its
  improvement.

The pass sends `didOpen` before `documentSymbol`, which is where a server
publishes diagnostics. `GET /api/findings` then records them through
`record_language_server_findings`, so the findings path needs no second
change.

**Tests.** Four engine tests use the production entry point, not
`start_definition_pass`:
`a_symbol_search_starts_the_pass_without_waiting_and_the_next_read_sees_it`
asserts the first snapshot is heuristic, returns quickly, and the next read is
`LanguageServer`;
`a_symbol_search_lets_a_configured_server_publish_a_finding` asserts the
diagnostic reaches the session as a `language_server` finding with a range and
no task;
`a_second_symbol_search_while_a_pass_runs_starts_no_second_pass` asserts the
claim holds through the caller; and
`a_symbol_search_with_no_server_configured_starts_no_pass` asserts no process
or registry entry. They are engine-level because the production caller
is in the engine; `handle_symbols` is the shell's one-line use of it.
**Falsified:** removing the `start_definition_pass_with` call makes both
app-path tests time out after 20 s waiting for a covered file, then passes again
when restored.

The Tauri app's exit path still does not call `shutdown_all`; that remains
recorded in Task 5's row and is out of scope here.
