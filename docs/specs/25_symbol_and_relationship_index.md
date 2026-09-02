# Feature Spec: Symbol and Relationship Index

Status: Not started
Order: 25 of 27
Roadmap: `docs/ROADMAP/03_phase_3_code_understanding.md`, Phase 3, Work
Package 3 (Must). That directory is local-only and not committed, so the
reference is a name rather than a link; this spec is self-contained.
Related spec sections: `ai_coding_assistant_specification.md` section 7.2
(project indexer), section 19 (recommended technology direction). Related
implementation specs: [`02_semantic_search.md`](02_semantic_search.md),
[`05_clickable_file_references.md`](05_clickable_file_references.md)
(navigation), [`22_findings_model_and_panel.md`](22_findings_model_and_panel.md)
(LSP diagnostics land as an additional `Finding` source here — this is where
Phase 2's deferred LSP dependency is satisfied),
[`24_repository_map_and_monorepo_boundaries.md`](24_repository_map_and_monorepo_boundaries.md)
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
  [spec 02](02_semantic_search.md)'s embedding index.
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
  [Spec 17](17_durable_task_state_and_crash_recovery.md) §5.7 defines the
  session-scoped PID registry, driven by MCP stdio servers and the `curl` model
  child. Language servers are a third client of it.
- **`Finding`** ([spec 22](22_findings_model_and_panel.md)) declares
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
   from [spec 05](05_clickable_file_references.md).
8. LSP diagnostics feed [spec 22](22_findings_model_and_panel.md)'s `Finding`
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
[spec 24](24_repository_map_and_monorepo_boundaries.md)'s map plus naming
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
  ([spec 05](05_clickable_file_references.md)) work for both, since a heuristic
  line number is still a line number.
- **Model context**: heuristic items are labelled in the rendered context, e.g.
  `upload_retry (function, src/upload.rs:42, heuristic — may be inaccurate)`.
  This is the part most likely to be skipped, and it is the part that matters:
  an agent told a relationship is certain will act on it, and a wrong `Implements`
  edge sends it to rewrite the wrong type. [Spec 26](26_context_assembly.md)
  carries the label through as provenance on the context item.

### 5.8 Process lifetime

Language servers are long-lived children, so requirement 9 uses the registry
from [spec 17](17_durable_task_state_and_crash_recovery.md) §5.7: PID and start
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
([spec 22](22_findings_model_and_panel.md)) with
`source: FindingSource::LanguageServer`, severity from the LSP severity, and
`range` from the LSP range. This satisfies requirement 8 and Phase 2's deferred
LSP dependency.

Diagnostics arrive asynchronously and unsolicited, unlike every other finding
source, which are produced by a command Damaian ran. Two rules follow:

- Diagnostics are attributed to the task active when they arrive, or to no task
  when none is. A diagnostic with no task is still shown in the panel.
- They do not, on their own, block a plan step
  ([spec 21](21_task_plan_progress_and_budget.md)). A step is blocked by a check
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

Symbol hits also feed [spec 26](26_context_assembly.md) as a distinct context
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
- The five quality-gate commands from `AGENTS.md` pass, and the
  [spec 18](18_local_evaluation_harness.md) baseline shows no regression.

## 7. Implementation Notes

To be completed during implementation. Record:

- Which language servers were tested, and the measured `documentSymbol` latency
  per file on a medium repository — this is the number that decides whether
  eager definition indexing is viable as specified.
- Heuristic symbol accuracy per language against the fixtures, before and after
  the §5.3 improvements. If comment-skipping and `impl` handling do not move
  Rust accuracy materially, say so — it changes whether a real parser is worth a
  later work package.
- Symbol index size on disk relative to the vector index, for the same
  repository.
