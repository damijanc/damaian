# Symbol and Relationship Index Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Implements:** [`proposal.md`](proposal.md), except the three `chat.rs` items
[`context.md`](context.md) §12 defers · corrections and the decisions it left
open in [`context.md`](context.md)
**Started:** not yet (planned 2026-10-09)

## Progress

| Task | State | Notes |
|---|---|---|
| 1 · Structured heuristic declarations | Not started | |
| 2 · `SymbolIndex::build`: symbols, relations, lookup | Not started | |
| 3 · Persistence, rebuild reporting, engine API, CLI | Not started | |
| 4 · LSP framing and protocol, without a process | Not started | |
| 5 · Configuration, the language-server hub and process lifetime | Not started | |
| 6 · Eager definitions and merging the tiers | Not started | |
| 7 · Diagnostics as findings | Not started | |
| 8 · The `symbols` context item | Not started | |
| 9 · References and implementations on demand | Not started | |
| 10 · Shell API and UI | Not started | |
| 11 · Docs, acceptance criteria, close the spec | Not started | |

**Goal:** Every symbol Damaian knows has a name, a kind, an exact file and
line, its spec 24 root, and a `Provenance` that says whether a language server
reported it or Damaian guessed it. Relationships (imports, `TestOf`,
references, implementations) carry the same label. Heuristic items are hedged
wherever they are shown, in the UI and in model context. A configured language
server improves the answers. A missing, slow or crashing one changes nothing
but the labels.

**Architecture:** The heuristic tier extends `language.rs` in place (Task 1)
and is extracted in the existing index pass (Task 2). `SymbolIndex::build` is a
pure function of the index, the repository map and the path policy (Task 2),
persisted by content hash (Task 3). The language-server tier is a framing and
JSON-RPC layer over `Read`/`Write` (Task 4). Under it are child processes
owned by a process-wide hub, registered with spec 46's registry (Task 5).
Definitions are fetched eagerly in the background and merged into the stored
index (Task 6). Diagnostics become findings with no task (Task 7). References
and implementations are fetched on request (Task 9). The model sees symbols as
one more `ContextItem` built in `context_manager.rs` (Task 8), so neither
`build_model_prompt` nor `chat.rs` changes.

**Tech Stack:** Rust 2024, `serde`/`serde_json` (already dependencies of
`workspace-engine`), `libc` (already used by the process registry), the
existing `ConfigOverlay` parser, vanilla JS in `app.js`. No new dependencies,
and no parser or grammar crate (proposal §4).

## Global Constraints

Every task's requirements implicitly include this section.

- **#25 must not edit `chat.rs` or `build_model_prompt`.** It runs in parallel
  with a `chat.rs` spec. The three things the proposal asks for that need
  `chat.rs` are deferred and named in `context.md` §12. If a task turns out to
  need `chat.rs` anyway, stop, record why in the progress row, and ask the
  user. Do not edit it "just for a call site".
- **Every cargo command runs with `CARGO_TARGET_DIR="$PWD/target"` from the
  worktree root**, because parallel worktrees must not share or thrash one
  target directory. Shell state does not persist between commands, so put it
  on every invocation:
  `CARGO_TARGET_DIR="$PWD/target" cargo clippy -p workspace-engine --all-targets --locked -- -D warnings`.
- **Read [`context.md`](context.md) in full before Task 1.** Several of its
  decisions override the proposal:
  - §1 and §3: spec 46's registry, a `LanguageServer` process kind, and a hub
    that lives for the process or an idle timeout, not for a session.
  - §2: LSP framing is new code over `Read`/`Write`; the MCP transport is the
    model for spawning only.
  - §4: declarations are extracted from the original content, in the index
    pass, and scanned afterwards.
  - §6: `language_server.<language>` is Forbidden at repository scope.
  - §7: restricted files never appear in any symbol output.
  - §8 and §9: definitions are fetched in the background and never awaited;
    the model sees a `symbols` context item.
  - §10: diagnostics are recorded per session with no task id.

  Do not re-derive any of these from `proposal.md` alone.
- **A heuristic is never presented as fact.** Every `Symbol` and `Relation`
  carries `Provenance`. Every surface that shows one (context item, shell
  JSON, UI, CLI) shows the label. No task may drop it to save space.
- **A language server never blocks anything.** No request thread, turn or
  shell handler waits on a server. Every reader takes the current snapshot.
  A server's absence is the heuristic tier with one input missing, not an
  error state (proposal §5.6).
- **Processes are tracked and killed by PID, never by name** (`AGENTS.md`,
  "Traps"). rust-analyzer is very likely the user's own editor process too.
  Every spawned server is registered before anything else touches it.
- **Tests never run a real language server, a real login shell, or a freshly
  written executable** (observation 25). The fake server is the test binary
  re-executed into an `#[ignore]`d helper (`context.md` §2). A test against a
  real server is `#[ignore]` with a doc comment giving the command.
- **Repository configuration stays untrusted.** Spec 34's
  `tests/repository_config_trust.rs` passes unchanged after every task, and no
  task edits it. Spec 31's `tests/permission_profiles.rs` is extended in Tasks
  5 and 8 only, as `context.md` §6 and §9 describe. No existing assertion is
  weakened.
- **Existing behaviour is unchanged where no symbol feature is used.**
  `extract_symbols` keeps returning names, so keyword and semantic scoring
  keep their shape (Task 1). With no language server configured, no process
  is spawned (Task 5).
- **New engine tests go in `crates/workspace-engine/tests/symbol_index.rs`**,
  which Task 1 creates, except the pure LSP protocol tests, which live in
  `lsp.rs`'s own `#[cfg(test)]` module. Fixtures that build an engine or an
  index set `enable_index_watcher: false`.
- **Falsify every load-bearing test.** Break what it guards, confirm it fails,
  revert, and record the mutation in the progress row. This repository has a
  history of tests that passed without testing anything (spec 47 §7.2,
  spec 21's error rate).
- **Scope per-task checks. Run the full seven-command gate from `AGENTS.md`
  once, in Task 11.** `cargo nextest run --workspace` takes about 5 minutes
  locally, and `cargo clippy --workspace --all-targets` up to 18 minutes cold.
- **Never `git commit` unasked.** Each task ends by showing the change and the
  scoped check results, then asking. When asked, write one subject line with no
  body and no trailer. Never push. Never switch branches.

## File Structure

| File | Change |
|---|---|
| `crates/workspace-engine/src/language.rs` | `SymbolKind`, `Declaration`, `extract_declarations`, comment and string skipping, Rust `impl` containers; `extract_symbols` derived from it (Task 1) |
| `crates/workspace-engine/src/symbol_index.rs` | New (Task 2): `Symbol`, `Provenance`, `Relation`, `RelationKind`, `RelationTarget`, `SymbolIndex::build`, import resolution, `TestOf`, `find`. `SymbolIndexStore` and `SymbolLoad` (Task 3). Merge (Task 6). Rendering for the model (Task 8). Reference cache (Task 9) |
| `crates/workspace-engine/src/indexer.rs` | `FileRecord.declarations`, extracted from the original content (Task 2) |
| `crates/workspace-engine/src/lsp.rs` | New (Task 4): framing, JSON-RPC, the request and notification types used, mapping to `Symbol`/`Relation`/`FindingDraft`. `LanguageServerHub` and the process layer (Task 5) |
| `crates/workspace-engine/src/process_registry.rs` | `ProcessKind::LanguageServer` (Task 5) |
| `crates/workspace-engine/src/config.rs`, `profile.rs`, `effective_policy.rs` | `language_server.<language>` (Task 5). `symbol_context_max_tokens` (Task 8) |
| `crates/workspace-engine/src/context_manager.rs` | The `symbols` item (Task 8) |
| `crates/workspace-engine/src/workspace_engine.rs` | `symbol_index()`, `find_symbols()` (Task 3). Hub access (Task 5). `record_language_server_findings()` (Task 7). `ContextManager` gets the symbol source (Task 8). `symbol_references()` (Task 9) |
| `crates/workspace-engine/src/lib.rs` | Module registration and re-exports (Tasks 1–9) |
| `crates/workspace-engine/tests/symbol_index.rs` | New (Task 1). Every later engine task adds to it |
| `crates/workspace-engine/tests/permission_profiles.rs` | Forbidden-class and rule-label entries and a weakening case (Task 5), a weakening case (Task 8) |
| `crates/workspace-engine/tests/token_ceiling.rs` | Only if Task 8 moves its call count (`context.md` §9) |
| `crates/damaian-cli/src/main.rs` | `symbols` (Task 3), `symbol-references` (Task 9) |
| `crates/desktop-shell/src/lib.rs` | `handle_findings` records language-server findings (Task 7). Symbol, reference and server-status endpoints (Task 10). Hub shutdown on exit (Task 5) |
| `crates/desktop-shell/static/app.js`, `index.html`, `style.css`, `docs/ui-style-guide.html` | Symbol search, heuristic marking, server status (Task 10) |
| `docs/USER_GUIDE.md`, `docs/TROUBLESHOOTING.md`, `SECURITY.md` | Task 11 |

Not in this spec: `chat.rs` and `edit.rs`.

## Interface reference

Checked on 2026-10-09 and re-checked on 2026-10-12. Re-check a line before
editing near it: other specs edit these files.

- `detect_language(path) -> &'static str` (`language.rs:3`).
  `extract_symbols(content, language) -> Vec<String>` (`:29`), lines at `:31`,
  per-language prefixes at `:35-61`. `extract_imports` (`:69-115`).
  `collect_after_prefix` (`:117`), `collect_variable` (`:126`),
  `take_identifier` (`:135`), `dedupe` (`:161`).
- `FileRecord` (`indexer.rs:22-33`), `symbols`/`imports` at `:29-30`, no serde.
  `SearchResult` (`:42`). `RepositoryIndex` (`:52`), `keyword_search` (`:61`),
  `semantic_search` (`:93`). `index_repository` (`:152`), `index_single_file`
  (`:222`), `add_file` (`:257`), redaction at `:284-286`, extraction at
  `:288-289`. `score_record` (`:355`), symbol names in terms at `:360-370`.
  `vector_index.rs:112` copies `symbols` and `imports`.
- `SecretScanner::redact(text) -> Redaction` (`secret_scanner.rs:42`),
  `contains_secrets` (`:65`).
- `PathPolicy::is_restricted(relative_path, is_directory)`
  (`path_policy.rs:138`).
- `RepositoryMap::build(index, policy)` (`repository_map.rs:291`),
  `root_for_path` (`:525`), `ProjectRoot.test_paths` (`:254`). `MapLoad`
  (`:752`), `repository_map_rebuilt` audit (`:844`).
  `WorkspaceEngine::repository_map(root) -> (RepositoryMap, MapLoad)`
  (`workspace_engine.rs:160`).
- `ProcessRegistry::open` (`process_registry.rs:139`), `register(kind,
  session_id, pid)` (`:172`), `ProcessKind` (`:64`, `as_str` `:78`),
  `sweep_with` (`:286`), start-time refusal (`:333`), `terminate` (`:393`),
  `RegistrationHandle`'s `Drop` (`:534`). `sweep_own` (`:430`), run by
  `install_shutdown_handler` (`:478`) on `SIGINT`/`SIGTERM`/`SIGHUP`. The
  shell installs it (`desktop-shell/src/main.rs:27`), and a spec 57 worker
  runs `sweep_own` on exit (`desktop-shell/src/worker.rs:97`).
- `StdioTransport` (`mcp.rs:302-438`): spawn `:315`, shell split `:328-339`,
  register `:365`, reader thread `:374-387`, `Drop` `:433`.
- `IndexCache` (`index_cache.rs:40`), its `OnceLock` registry (`:28`),
  `get_or_build` (`:43`).
- `FindingSource::LanguageServer` (`finding.rs:38`), `Severity` (`:70`),
  `SourceRange` (`:101`, 1-based, repository-relative), `FindingDraft`
  (`:114`), `Finding::new(draft, scanner)` (`:151`), random id (`:158`),
  `with_file_hash` (`:183`), `resolve_finding_path` (`:268`).
  `SessionStore::record_finding(session_id, &Finding)` (`session.rs:1688`).
  `Evidence::Findings` (`plan.rs:95`).
- `ContextItem` (`context_manager.rs:24`), `ContextManager` (`:42`),
  `RepositoryMapSource` (`:52`), `new` (`:61`), `build_context` (`:81`): the
  `repository_map` item sits after `project_rule` and before `retrieved_file`.
  `ContextManager::new` has one caller (`workspace_engine.rs:71`).
- Config: `DEFAULT_CONTEXT_TOKEN_BUDGET` (`config.rs:13`),
  `DEFAULT_IGNORE_PATTERNS` (`:15`), `DEFAULT_RESTRICTED_PATTERNS` (`:31`),
  `shell` scoped as Forbidden (`:1170`), MCP server scopes (`:1389-1431`),
  `ConfigOverlay::set`'s `shell` arm (`:2926`), `classify_overlay_fields!`
  (`:2673`), `repository_map_max_tokens => Capability` (`:2680`),
  `hooks => Capability` (`:2734`).
- Spec 31's tests: `the_preference_keys_are_exactly_spec_34s_free_keys`
  (`tests/permission_profiles.rs:103`), `weakening_cases` (`:152`),
  `every_capability_key_resists_weakening_from_repository_scope` (`:417`),
  the Forbidden table (`:1189`), the rule-label table (`:2153`).
- Shell: `GET /api/findings` → `handle_findings`
  (`desktop-shell/src/lib.rs:324`, `:3775`).
- `chat.rs`, read-only, for orientation: `build_mcp_runtime` (`:539`),
  `search_codebase` execution (`:3756-3786`), tool definitions
  (`:4501-4610`), `format_search_results` (`:5215`).

---

## Task 1: Structured heuristic declarations

**Requirements:** 1 (the fallback tier) and proposal §5.3. Acceptance criteria
"symbol kind is recorded, and a Rust method inside an `impl` block is recorded
as `Method` with its container", "a declaration inside a comment or a string
literal is not indexed as a symbol", and the heuristic half of "symbol
accuracy fixtures pass for each supported language". **Files:**
`crates/workspace-engine/src/language.rs`, `lib.rs`; create
`crates/workspace-engine/tests/symbol_index.rs` and fixture files under
`crates/workspace-engine/tests/fixtures/symbols/`.

This task changes no caller. `extract_symbols` keeps its signature and returns
the names of `extract_declarations`. Nothing reads the line or kind until Task
2.

**Interfaces:**

```rust
/// What source text alone says about one declaration. Task 2 turns it into a
/// `Symbol` by adding the path, root and provenance.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Declaration {
    pub name: String,
    pub kind: SymbolKind,
    /// 1-based.
    pub line: u32,
    /// 1-based, the first character of the name.
    pub column: Option<u32>,
    /// The enclosing type, for a Rust method inside `impl Type` or
    /// `impl Trait for Type`: the `Type`.
    pub container: Option<String>,
}

pub fn extract_declarations(content: &str, language: &str) -> Vec<Declaration>;
```

`SymbolKind` (proposal §5.1) is defined in this task, in `language.rs`, and
re-exported. Task 2 moves nothing. It gains `#[derive(Serialize, Deserialize)]`
with `rename_all = "snake_case"` here, so Task 3 does not touch it.

- [ ] **Step 1: Write the accuracy fixtures first.** One file per language
  (`rust.rs`, `typescript.ts`, `javascript.js`, `python.py`, `go.go`,
  `java.java`, `kotlin.kt`, `php.php`). Each holds the declarations
  `extract_symbols` finds today, plus the known misses and false positives:
  a declaration inside a line comment, a block comment and a string or raw
  string; a multi-line string containing `fn x(`; for Rust, `impl Foo { fn a }`,
  `impl Bar for Foo { fn b }`, `pub(crate) fn`, `async fn`, `const fn`,
  `trait`, `type` and `const`; for JS/TS, a `const` local inside a function
  body. Each fixture has an expected list `(name, kind, line, container)`
  beside it as a Rust table in the test.
- [ ] **Step 2: Record the "before" accuracy.** Run today's `extract_symbols`
  over each fixture. Record, per language, how many expected names it found,
  how many it found that are not expected, and the precision and recall. This
  is proposal §7's "before" number. Record it in the progress row now, because
  Step 4 destroys it.
- [ ] **Step 3: Write the failing tests** in `tests/symbol_index.rs`: each
  fixture's expected list equals `extract_declarations`' output. Also, for
  every fixture, `extract_symbols` equals the deduplicated names of
  `extract_declarations`, which pins Task 1's "no caller changes" claim.
- [ ] **Step 4: Implement** `extract_declarations`:
  - a per-language lexical state carried across lines: in a line comment, in
    a block comment (`/* */`, Python `"""` docstrings), or in a string (`"`,
    `'`, `` ` ``, Rust `r#"…"#`). Declaration matching runs only on code
    outside these states. This is a scanner of comment and string delimiters,
    not a parser (proposal §4);
  - each existing prefix maps to a `SymbolKind` instead of being discarded;
  - Rust: strip any visibility (`pub`, `pub(crate)`, `pub(super)`,
    `pub(in …)`) and the qualifiers `async`, `const`, `unsafe`, `extern "C"`,
    then match `fn`, `struct`, `enum`, `trait`, `type`, `const`, `static` and
    `mod`. Track `impl … {` with a brace depth, so an `fn` inside it is
    `Method` with `container` set to the implementing type;
  - JS/TS: `const`/`let` count only at brace depth 0, so locals are not
    declarations;
  - `extract_symbols` becomes `dedupe(names of extract_declarations)`.
- [ ] **Step 5: Record the "after" accuracy** in the same form as Step 2. If
  comment skipping and `impl` handling do not move Rust materially, say so
  (proposal §7).
- [ ] **Step 6: Scoped checks.**
  `CARGO_TARGET_DIR="$PWD/target" cargo nextest run -p workspace-engine --test symbol_index --locked`,
  the `language.rs` unit tests, `cargo fmt --all -- --check`,
  `CARGO_TARGET_DIR="$PWD/target" cargo clippy -p workspace-engine --all-targets --locked -- -D warnings`
  and `typos`.
- [ ] **Step 7: Falsify.** At least: comment skipping off (the comment
  fixture fails); `impl` tracking off (the method has no container);
  brace-depth check off for JS (the local appears); `extract_symbols` not
  deduplicated (the equality test fails).
- [ ] **Step 8: Update the progress row, show the change, and ask before
  committing.**

## Task 2: `SymbolIndex::build`: symbols, relations, lookup

**Requirements:** 2 (definitions, kinds, locations, imports, file
dependencies, source-to-test), 3, 6 (local), and proposal §5.1, §5.2 and §5.11.
Acceptance criteria "searching for a symbol returns its definition with an
exact file and line" (heuristic tier), "an unresolvable import is recorded as
`External`", "a relative import resolves to a repository file", and "with no
language server configured, symbol extraction works and every item is
`Heuristic`". **Files:** create `symbol_index.rs`; modify `indexer.rs`,
`lib.rs`, `tests/symbol_index.rs`.

- `FileRecord` gains `declarations: Vec<Declaration>`. `add_file` extracts
  them from the original content, then drops any declaration whose `name` or
  `container` the scanner flags (`context.md` §4.2). `symbols` keeps being
  derived from redacted text, exactly as today.
- `Symbol`, `Provenance`, `Relation`, `RelationKind`, `RelationTarget`,
  `SymbolRef` as proposal §5.1–§5.2, all serde with `rename_all` as written
  there. `Provenance` derives `Ord` with `LanguageServer` first.
  `SymbolRef { path, name, line }`.
- `SymbolIndex::build(index: &RepositoryIndex, map: &RepositoryMap, policy: &PathPolicy) -> SymbolIndex`
  is pure. It skips restricted files (`context.md` §7), fills `root_path`
  from `map.root_for_path`, and sets every item to `Heuristic`. Relations are
  `Imports`, resolved per `context.md` §11, and `TestOf`. Output is sorted
  (by path, line, name), so it is deterministic.
- `SymbolIndex::find(&self, query: &str, limit: usize) -> Vec<SymbolMatch>`:
  exact name, then prefix, then fuzzy (case-insensitive subsequence), ordered
  by provenance, then match quality, then path. `SymbolMatch` carries the
  `Symbol` and the match quality.

Tests:
- The fixtures from Task 1, indexed: `find("upload_retry")` returns the
  definition at its exact path and line, with `Heuristic`.
- A PEM block above a function: the symbol's line is the function's true line.
  A function whose name matches a configured secret pattern is absent.
- A restricted `.env.local` with a `const` in it contributes nothing.
- Two roots (spec 24's `packages/api` and `packages/web`) each with
  `index.ts`: each symbol's `root_path` is its own root.
- `import { x } from "./upload"` resolves to `upload.ts`, `"../lib"` to
  `lib/index.ts`, `"react"` stays `External { specifier: "react" }`, and a
  relative specifier with no file stays `External`. Never a `File` edge to a
  path not in the index.
- `src/upload.rs` ↔ `tests/upload.rs` and `foo.ts` ↔ `foo.test.ts` give
  `TestOf`, both `Heuristic`.
- Two builds from the same index serialise identically, one from a reversed
  `files` list.

Mutations: extraction from redacted content (the PEM line test fails);
restricted filter removed; an unresolved relative import becomes a `File`
edge; `Provenance` ordering reversed (`find` order fails once Task 6 exists,
so here assert the derived `Ord` directly).

## Task 3: Persistence, rebuild reporting, engine API, CLI

**Requirements:** 6 (versioned), proposal §5.10, and acceptance criterion "a
corrupt or version-mismatched symbol index rebuilds cleanly and reports it".
**Files:** `symbol_index.rs`, `workspace_engine.rs`, `lib.rs`,
`damaian-cli/src/main.rs`, `tests/symbol_index.rs`.

- `SymbolIndexStore::new(data_dir, audit_log)` and
  `load_or_build(&index, &map, &policy) -> Result<(SymbolIndex, SymbolLoad)>`,
  with `SymbolLoad { Reused, Built, Rebuilt { reason } }` and the reasons
  `Corrupt`, `SchemaMismatch` and `Stale`, following `RepositoryMapStore`
  (`context.md` §5). The file is
  `<data_dir>/symbol-index/<repository_id>.json` and records `schemaVersion`
  and a `contentHash` per file.
- On load, entries for files whose hash matches are kept, from either tier.
  Files whose hash changed, or that are new, are rebuilt heuristically, and
  their language-server entries are dropped. Files no longer in the index are
  dropped.
- A rebuild of any kind audits `symbol_index_rebuilt` with `reason`, the file
  count and the duration.
- `WorkspaceEngine::symbol_index(root) -> Result<(SymbolIndex, SymbolLoad)>`
  and `find_symbols(root, query, limit) -> Result<Vec<SymbolMatch>>`, from
  the cached index and map.
- CLI `damaian symbols <repo> <query> [--json]` prints
  `name  kind  path:line  root  provenance`.

Tests: garbage bytes rebuild as `Corrupt` and audit; another schema version
rebuilds as `SchemaMismatch`; an unchanged repository is `Reused` with an
identical index; editing one file re-extracts that file only (assert another
file's stored entry, marked `LanguageServer` by hand in the fixture, survives
while the edited file's does not). Measure `load_or_build` on this repository
(`Built` and `Reused`, release build) and record both numbers and the file
size, for proposal §7.

Mutations: no schema check; a changed file's language-server entry kept; no
audit on rebuild.

## Task 4: LSP framing and protocol, without a process

**Requirements:** the protocol half of 1 and proposal §5.4. **Files:** create
`lsp.rs`; modify `lib.rs`.

Everything here runs over `Read`/`Write` and is tested in `lsp.rs`'s own test
module against in-memory buffers. No process is spawned (`context.md` §2).

- Framing: `write_message(&mut impl Write, &Value)` writes
  `Content-Length: N\r\n\r\n` and the body. `read_message(&mut impl BufRead)`
  reads headers until a blank line, ignores `Content-Type`, and reads exactly
  `N` bytes. A missing or non-numeric `Content-Length`, EOF mid-body, or a
  body over `MAX_LSP_MESSAGE_BYTES` (16 MiB) is an error, never a panic or an
  unbounded allocation.
- JSON-RPC: request ids, matching responses to requests, routing
  notifications, and turning an error response into a `ClientError`. Server
  requests that need a reply (`workspace/configuration`,
  `client/registerCapability`, `window/workDoneProgress/create`) get a
  minimal valid reply, so a server never stalls waiting for one.
- The lifecycle and requests the proposal names, and no others:
  `initialize` (capabilities limited to what is used),
  `initialized`, `shutdown`, `exit`, `textDocument/didOpen`,
  `textDocument/didClose`, `textDocument/documentSymbol`,
  `textDocument/definition`, `textDocument/references`,
  `textDocument/implementation`, and the `textDocument/publishDiagnostics`
  notification.
- Mapping: `DocumentSymbol` (hierarchical, children give `container`) and
  `SymbolInformation` (flat, `containerName`) both become `Symbol` with
  `LanguageServer` provenance. The line comes from `selectionRange`, converted
  0-based → 1-based, and the kind follows `context.md` §8's table. A
  `Location` becomes a `SymbolRef`. A `file://` URI outside the repository
  root is dropped. A diagnostic becomes a `FindingDraft` with
  `FindingSource::LanguageServer` (`context.md` §10).

Tests: round trip of framing, including a body with embedded `\r\n` and
multibyte UTF-8 (the length is in bytes); each malformed case above is an
error; a response arriving after an unrelated notification still matches its
request; a hierarchical and a flat `documentSymbol` response give the same
`Symbol`s; a URI outside the root and a percent-encoded path; each severity.

Mutations: length counted in chars; the 16 MiB bound removed (the oversized
test must fail by error, not by OOM, so assert the error before any
allocation); `range` used instead of `selectionRange`.

## Task 5: Configuration, the language-server hub and process lifetime

**Requirements:** 1 (language servers where available), 5 (unavailable and
crashing servers degrade), 9 (PID tracking), proposal §5.4 and §5.8.
Acceptance criteria "a repeatedly crashing language server is not restarted in
a loop", and "no language server process outlives the session that started
it, and a recorded PID whose start time no longer matches is not killed", as
restated in `context.md` §3. **Files:** `config.rs`, `profile.rs`,
`effective_policy.rs`, `process_registry.rs`, `lsp.rs`, `workspace_engine.rs`,
`lib.rs`, `desktop-shell/src/lib.rs` (shutdown path only),
`tests/symbol_index.rs`, `tests/permission_profiles.rs`.

- `language_server.<language>=<command line>`, as `context.md` §6 describes:
  a capability, Forbidden at repository scope, not a profile key, with a
  parse error for an unknown language. Every exhaustive site is handled, and
  `rule_label` reads "Language server for <language>".
- `ProcessKind::LanguageServer` (`context.md` §1).
- `LanguageServerHub`, process-wide, keyed by `(repository_id, language)`
  (`context.md` §3). A server starts lazily on first use. The spawn follows
  `StdioTransport::spawn`: shell split, piped stdin and stdout, null stderr,
  `process_group(0)`, registered before anything else, a reader thread
  feeding a channel, and `initialize` under
  `LANGUAGE_SERVER_REQUEST_TIMEOUT`.
- Each entry has a state: `Starting`, `Ready`, `Unavailable { reason }` (spawn
  failed, `initialize` failed or timed out) or `Crashed { restarts }`. A
  request to a server that is not `Ready` returns "unavailable" at once.
  A crash (EOF on stdout, or the child exits) marks the entry `Crashed`. It
  is restarted once, on the next request. A second crash leaves it down for
  the life of the entry.
- Shutdown: `shutdown` then `exit`, a grace period, then `Child::kill` and
  `wait`. This runs on idle expiry (`LANGUAGE_SERVER_IDLE_SECS`), from the
  hub's `Drop`, and from `LanguageServerHub::shutdown_all()`, which the
  desktop shell calls on exit. On a signal, the existing `sweep_own` kills
  every registered server by PID, so no new signal handling is added
  (`context.md` §1 and §3).
- `LanguageServerHub::status(repository_id) -> Vec<LanguageServerStatus>`
  for Task 10.

Tests, with the fake server from `context.md` §2. Its behaviour is chosen by
an environment variable: answer normally, never answer `initialize`, exit
after N requests, or exit at once.
- No `language_server.*` configured: no process is spawned and no registry
  entry is written.
- A repository config with `language_server.rust=/bin/sh` is refused and
  audited, and the user's value stands. A profile that carries the key is
  rejected.
- A normal fake server: one registry entry of kind `language_server` while it
  runs. None after `shutdown_all`, and the PID no longer exists.
- Idle expiry, with the timeout lowered by a test-only constructor: the server
  is gone and its entry removed.
- A server that never answers `initialize` is `Unavailable` within the
  timeout, and a request returns at once.
- Exit after one request, then again after restart: one restart, then
  `Crashed`, and no third process (count registry entries and spawns).
- A `LanguageServer` registry entry whose recorded start time does not match
  the live process is refused by the sweep (`context.md` §1).

Mutations: register after the first write to stdin (assert the registry entry
exists before `initialize` is sent); unlimited restarts; idle expiry
disabled; the repository scope allowed (the refusal test fails).

## Task 6: Eager definitions and merging the tiers

**Requirements:** 1, 3, 5, proposal §5.5 (definitions) and §5.6. Acceptance
criteria "searching for a symbol returns its definition with an exact file and
line" (language-server tier), "killing a language server mid-task degrades to
heuristics without failing the task", and "a language server that indexes only
part of the repository does not cause heuristic symbols elsewhere to
disappear". **Files:** `symbol_index.rs`, `lsp.rs`, `workspace_engine.rs`,
`tests/symbol_index.rs`.

- The eager pass, on the hub's thread, per `context.md` §8: for each indexed,
  non-restricted file of a configured language, `didOpen` →
  `documentSymbol` → `didClose`, each request under the timeout. Results are
  merged into the stored index (Task 3's store) as each file completes, so
  progress survives a restart. A file whose stored language-server entries
  match its content hash is skipped.
- `merge(heuristic, language_server)` per `context.md` §8: superseded within
  `MERGE_LINE_TOLERANCE`, `Interface` and `Trait` compatible, everything
  unmatched kept, sorted with `Provenance`'s `Ord`.
- Readers never wait. `find_symbols` and the context item read the stored
  snapshot. A pass in progress is visible as a mixture of both tiers.

Tests:
- The fake server answers `documentSymbol` for `src/upload.rs` only:
  `find("upload_retry")` returns one symbol with `LanguageServer` and the
  server's line, and heuristic symbols in every other file are still present.
- Same name in two files: only the file the server covered is superseded.
- The heuristic line is 41, the server's `selectionRange` line is 42: merged.
  At a distance of 10 lines: both kept.
- The fake server exits halfway through the pass: the pass stops, the files it
  covered keep their language-server symbols, the rest are heuristic, and a
  `find_symbols` call during and after returns without error. A mock-model
  turn run while it happens completes (use `DAMAIAN_MOCK_MODEL_RESPONSE`; this
  turn needs no `chat.rs` change, only the engine).
- A server that never answers `documentSymbol` costs at most one timeout per
  file and never delays `find_symbols`.

**Measurement (proposal §7).** Ask the user before installing anything
(`context.md` §13). If a real server is available, add an `#[ignore]`d test
with a doc comment giving its command, run it against this repository, and
record: the server and version, the median and p90 `documentSymbol` latency
per file, the full pass's wall time, the warm launch against the cold one, and
whether eager definition indexing is viable as specified. If not, record "not
measured" and why.

Mutations: a heuristic symbol dropped when any server symbol exists in the
file; tolerance 0 (the 41/42 test fails); the reader waiting on the pass (the
never-answers test fails on time).

## Task 7: Diagnostics as findings

**Requirements:** 8 and proposal §5.9. Acceptance criterion "LSP diagnostics
appear as `Finding`s with `source: language_server` and a range, and do not
block a plan step on their own". **Files:** `lsp.rs`, `workspace_engine.rs`,
`desktop-shell/src/lib.rs` (`handle_findings` only), `tests/symbol_index.rs`,
the shell's `tests` module.

Implements `context.md` §10.

- The hub keeps the latest `publishDiagnostics` per file. A notification for
  a restricted file, or a URI outside the repository, is dropped.
- `WorkspaceEngine::record_language_server_findings(session_id, repository_root) -> Result<usize>`
  records the current diagnostics the session does not hold yet, deduplicated
  on `(path, range, code, summary, file_hash)`, with no task id. It returns
  how many it recorded.
- `handle_findings` calls it before listing. Any failure is logged to stderr
  and does not fail the request: the panel shows what it has.

Tests: the fake server publishes an error and a warning on `src/x.rs`. The
recorded findings have `source: language_server`, 1-based ranges, the file's
hash and no task id. A second call records nothing new. A new notification
that clears the file records nothing and removes nothing (spec 22's statuses
belong to the user). A session with an active plan step and a recorded
language-server error: the step's evidence and phase are unchanged. A shell
test: `GET /api/findings` returns the language-server finding.

Mutations: no deduplication (the second call records again); 0-based lines
kept; the task id taken from the session's plan.

## Task 8: The `symbols` context item

**Requirements:** 4 (model context), 7 (search integration), proposal §5.7
and §5.11. Acceptance criterion "a heuristic-derived item is labelled as such
in the UI and in rendered model context — asserted by test on the rendered
context string". **Files:** `symbol_index.rs`, `config.rs`, `profile.rs`,
`effective_policy.rs`, `context_manager.rs`, `workspace_engine.rs`, `lib.rs`,
`tests/symbol_index.rs`, `tests/permission_profiles.rs`, and
`tests/token_ceiling.rs` only if needed (`context.md` §9). Not `chat.rs` and
not `edit.rs`.

Implements `context.md` §9.

- `symbol_context_max_tokens: usize`, default 1,600, a **capability** key,
  lower-wins at repository scope, with the same merge as
  `repository_map_max_tokens`. It also needs a `classify_overlay_fields!`
  entry, the policy texts, `rule_label`, and a weakening case. `0` disables
  the item.
- `SymbolIndex::render_for_model(&self, matches, max_tokens) -> RenderedSymbols { text, tokens, shown, total }`.
  One line per symbol, with the label for heuristics. When it drops matches
  to fit, it says so ("8 of 23 matching symbols shown"). Tokens are estimated
  with `len.div_ceil(4)`, as `add_text` does.
- `ContextManager` gets a symbol source at construction, as spec 24 gave it
  the map. `build_context` adds a `symbols` item, with `path: None`, after
  `repository_map` and before `retrieved_file`, when an index is present, the
  ceiling is non-zero, and the prompt's identifiers match at least one
  symbol. A failure omits the item and never fails the turn. Before
  starting, check spec 32's progress table: if its Task 6 has landed,
  `build_context` delegates to `build_context_with_hooks`, and the item goes
  in that shared body (`context.md` §9).

Tests:
- A prompt naming `upload_retry` against the fixture: the plan has exactly one
  `symbols` item, placed after `repository_map` and before any
  `retrieved_file`, and its content contains
  `upload_retry (function, src/upload.rs:42, heuristic — may be inaccurate)`.
- With a merged language-server symbol, the same line has no hedge.
- With the ceiling at `0`, and with a prompt matching nothing: no item.
- A small ceiling drops matches and states how many were shown.
- `symbol_context_max_tokens=100000` in repository config is refused; `50` is
  applied.
- Run `tests/token_ceiling.rs`. If its exact call count moved, apply
  `context.md` §9's fix and record why.

Run the deterministic eval tier
(`CARGO_TARGET_DIR="$PWD/target" cargo run -p eval-harness -- run --tier deterministic`).
Every scenario must still pass. Record the token metric's change against
`evals/baseline.json`.

Mutations: the label dropped for heuristics (the rendered-string test fails);
the item placed after `retrieved_file`; no ceiling; the key merged as a
preference.

## Task 9: References and implementations on demand

**Requirements:** 2 (references where reliable, implementations), proposal
§5.5. Acceptance criterion "references are resolved on demand and a cached set
is discarded when the file's content hash changes". **Files:**
`symbol_index.rs`, `lsp.rs`, `workspace_engine.rs`, `lib.rs`,
`damaian-cli/src/main.rs`, `tests/symbol_index.rs`.

The user asks through the shell (Task 10) and the CLI. The agent cannot ask,
because that needs a `chat.rs` tool (`context.md` §12, item 1).

- `WorkspaceEngine::symbol_references(root, symbol: &SymbolRef) -> Result<SymbolReferences>`
  and `symbol_implementations(..)`. With a `Ready` server, these send
  `textDocument/references` (`includeDeclaration: false`) and
  `textDocument/implementation`, giving `References` and `Implements`
  relations with `LanguageServer` provenance. Without one, the result is
  empty and `available: false`, never a guess: proposal §3 asks for
  references only "where reliable".
- Results are cached in the store under the defining file's content hash and
  every referencing file's hash. A cached set is returned only if every hash
  still matches. Otherwise it is discarded and re-requested.
- These calls are the one place a caller waits on a server, because the user
  explicitly asked. The wait is bounded by `LANGUAGE_SERVER_REQUEST_TIMEOUT`,
  and a timeout returns `available: false` with the reason.
- CLI `damaian symbol-references <repo> <path>:<line> <name> [--json]`.

Tests: the fake server answers `references` with two locations: two
`References` relations, each with `LanguageServer`. A second call is served
from the cache (the fake server counts requests). Editing a referencing file
discards the cache and the next call asks again. No server, and a server that
times out: `available: false`, with no heuristic references.

Mutations: the cache checked against the defining file only (editing a
referencing file is missed); a heuristic fallback added.

## Task 10: Shell API and UI

**Requirements:** 4 (UI), 7 (search and clickable references), proposal §5.7.
The UI half of the acceptance criterion on labelling. **Files:**
`desktop-shell/src/lib.rs`, `app.js`, `index.html`, `style.css`,
`docs/ui-style-guide.html`.

- `GET /api/symbols?repo=&q=` returns matches with their provenance.
  `GET /api/symbol-references?repo=&path=&line=&name=` returns Task 9's
  result. `GET /api/language-servers?repo=` returns hub status. All three are
  `handle_*` functions, not inline arms (observation 26), and refuse a
  repository outside the allowed roots.
- A symbol search in the UI, following `docs/UI_STYLE_GUIDE.md`. Each result
  shows name, kind and `path:line` as a clickable reference (spec 05), and
  its root when it is not `""`, as spec 24 Task 8 does. A heuristic result
  carries a "likely" marker whose hover text says it was derived from source
  text rather than a language server. Selecting a result offers "Find
  references". That shows Task 9's result, or "No language server for
  <language>" when `available` is false.
- Settings shows each configured language server's state, with the reason
  when it is unavailable or crashed.

Tests: shell tests for the three endpoints (the JSON shape, the provenance
field, and a foreign repository refused). Verify in the running app: rebuild,
start on a port other than 4765 with its own `DAMAIAN_DATA_DIR`, drive it from
the browser, and stop it by PID. Check a heuristic result's marker and hover
text, a clickable `path:line`, references with and without a server, and the
Settings status after the fake or a real server crashes.

## Task 11: Docs, acceptance criteria, close the spec

**Requirements:** proposal §6 and §7. **Files:** `docs/USER_GUIDE.md`,
`docs/TROUBLESHOOTING.md`, `SECURITY.md`, `proposal.md` §7, `CHANGELOG.md`,
and every status record.

- `docs/USER_GUIDE.md`:
  - what symbol search finds, and what "likely" means;
  - configuring `language_server.<language>` in `user.conf`, with an
    absolute path, and that a repository cannot set it;
  - what Damaian sends a language server (`context.md` §6);
  - that a server stops after 10 idle minutes and when Damaian quits, and
    that a server that crashes twice stays down until then;
  - that diagnostics appear in the findings panel with no task;
  - `symbol_context_max_tokens`, and how to turn the item off.
- `docs/TROUBLESHOOTING.md`:
  - `symbol-index/<repo-id>.json`, and forcing a rebuild by deleting it;
  - the `symbol_index_rebuilt` audit event;
  - `damaian symbols` and `damaian symbol-references`;
  - reading a server's state, a server not found on a GUI app's `PATH`, and
    `language_server` entries in the process registry and the sweep's audit.
- `SECURITY.md`: `language_server.*` joins `shell` as a key a repository
  cannot set.
- Map every acceptance criterion in proposal §6 to its test in a new §7.1.
  Mark `context.md` §12's items as not met for the agent, with the reason, and
  name the follow-up `chat.rs` task as proposed work.
- Answer proposal §7's recording questions from Tasks 1, 3 and 6: the servers
  tested and `documentSymbol` latency (or "not measured" and why), heuristic
  accuracy before and after per language, and the symbol index's size against
  the vector index for this repository.
- Run the full seven-command gate from `AGENTS.md`, every cargo command with
  `CARGO_TARGET_DIR="$PWD/target"`, plus `npm run specs:check` and the
  deterministic eval tier. Confirm no regression against
  `evals/baseline.json` beyond the token change recorded in Task 8.
- Close out per `AGENTS.md` "When a spec becomes Done":
  - set every status record to Done: this proposal's `Status:`, its README
    row, and this file's progress table and `Started`/`Done` header;
  - mark #25 built in #26's `Depends on:` line;
  - re-derive "What to build next" (#26's last unmet dependency is #25);
  - remove the symbol bullet ("Find where a symbol is defined and what uses
    it…") from `CHANGELOG.md`'s `Unreleased` table.
