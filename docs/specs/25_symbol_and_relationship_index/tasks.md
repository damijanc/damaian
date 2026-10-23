# Symbol and Relationship Index Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Implements:** [`proposal.md`](proposal.md), except the three `chat.rs` items
[`context.md`](context.md) §12 defers · corrections and the decisions it left
open in [`context.md`](context.md)
**Started:** 2026-10-13 (planned 2026-10-09)

## Progress

| Task | State | Notes |
|---|---|---|
| 1 · Structured heuristic declarations | Done 2026-10-14 | **Landed:** `SymbolKind` (serde, `snake_case`), `Declaration` and `extract_declarations` in `language.rs`, re-exported from `lib.rs`; `extract_symbols` is now the deduplicated names of `extract_declarations`, and `indexer.rs` is unchanged. A per-language mask blanks comments and strings, carried across lines: line comments, block comments (nested for Rust and Kotlin), `"`/`'`/`` ` `` strings, Python `"""`/`'''`, Java and Kotlin `"""`, Rust raw strings, and Rust char literals told apart from lifetimes. Matching runs on the masked line and keeps its columns. Fixtures are in `tests/fixtures/symbols/`, one per language. Their tables in `tests/symbol_index.rs` record every declaration as `Found`, `Missed`, `Inexact` (right name, wrong kind or container) or `FalsePositive`. Remaining gaps are pinned, not left out. **Accuracy, by name** (true = distinct true names, extra = names that are not declarations). **Before** (Step 2, today's `extract_symbols`): rust 11/22 found, 4 extra, P 0.73 R 0.50 · typescript 8/12, 5 extra, P 0.62 R 0.67 · javascript 8/10, 5 extra, P 0.62 R 0.80 · python 4/6, 3 extra, P 0.57 R 0.67 · go 4/7, 2 extra, P 0.67 R 0.57 · java 1/4, 2 extra, P 0.33 R 0.25 · kotlin 3/5, 2 extra, P 0.60 R 0.60 · php 3/4, 2 extra, P 0.60 R 0.75. After that run, the Rust fixture gained a `macro_rules!` body, which the old `fn ` prefix also matches. Against the final fixture, Rust before is therefore 11/22, 5 extra, P 0.69 R 0.50 (derived, not re-run). **After** (Step 5): rust 22/22, 1 extra, P 0.96 R 1.00 · typescript 8/12, 0 extra, P 1.00 R 0.67 · javascript 8/10, 0, P 1.00 R 0.80 · python 4/6, 0, P 1.00 R 0.67 · go 4/7, 0, P 1.00 R 0.57 · java 1/4, 0, P 1.00 R 0.25 · kotlin 3/5, 0, P 1.00 R 0.60 · php 3/4, 0, P 1.00 R 0.75. Every found Rust declaration has the right kind, line and container. Python, Kotlin and PHP methods are found by name but recorded as `Function` with no container (`Inexact`), because only Rust tracks containers. **For proposal §7:** comment and string skipping took precision to 1.00 in every language. The Rust changes moved Rust recall materially, 0.50 → 1.00, through visibility, qualifiers, the new keywords and `impl` tracking. Recall in the other languages did not move. Their remaining misses are prefix gaps, not parsing problems: modifiers (`public class`, `private fun`, `data class`), `async def`/`async function`, TS `type`/`enum`, Go receivers, and class methods. **Tests:** `symbol_index` 12/12; `language.rs` unit tests 4/4; workspace-engine `--lib` + `semantic_search` + `repository_map` + `symbol_index`: 465 passed, 4 skipped; `cargo fmt --check`, `cargo clippy -p workspace-engine --all-targets -D warnings` and `typos` clean. Deterministic eval tier: all 16 scenarios pass. Tokens are 108,592 against 107,345 for the same tree with HEAD's `language.rs` (+1,247, +1.2%). That comes from keyword scoring seeing different names. `evals/baseline.json`'s 101,963 (2026-09-18) predates other specs' drift. **Mutations** (each failed, then reverted): block comments not skipped → 7 fixture tests and the column test fail; `impl`/`trait` containers not pushed → rust fixture fails; JS depth check off → TS and JS fixtures fail; `extract_symbols` not deduplicated → the equality test (and the accuracy test's sanity assert) fail; raw strings not recognised → rust fixture fails; char literals not recognised → rust fixture fails. The raw-string fixture held an even number of quotes, so a plain `"` scanner masked the same lines. It now holds one quote, which makes the raw-string mutation fail. **Deviations:** a Rust `trait` body is a container too (`fn put` in `trait Store` → `Method`, container `Store`), because rust-analyzer reports trait methods under the trait and Task 6 merges against that. Associated `const`/`type` inside an `impl` or `trait` get the container. `static` maps to `Variable` and `mod` to `Module`. Go `type` takes its kind from what follows (`struct` → `Struct`, `interface` → `Interface`, otherwise `Type`). A declaration after an inline block comment (`/* x */ fn y`) now matches. **Known limits, for later tasks:** a `fn` inside a `macro_rules!` body is still a declaration (pinned as `FalsePositive`, name `$name`); JS regex literals, PHP heredocs and PHP `#[…]` attributes are not lexed; only Rust tracks containers. |
| 2 · `SymbolIndex::build`: symbols, relations, lookup | Done 2026-10-15 | **Landed:** `FileRecord.declarations` (`indexer.rs`), extracted in `add_file` from the original content and then filtered: a declaration whose `name` or `container` `SecretScanner::contains_secrets` flags is dropped. `symbols` and `imports` are still extracted from the redacted text, unchanged. New `symbol_index.rs`, re-exported from `lib.rs`: `Provenance` (`Ord`, `LanguageServer` first), `Symbol`, `RelationKind`, `SymbolRef`, `RelationTarget`, `Relation` with proposal §5.1–§5.2's serde shapes, plus `MatchQuality { Exact, Prefix, Fuzzy }`, `SymbolMatch { symbol, quality }` and `SymbolIndex { repository_id, symbols, relations }` (serde, camelCase). `SymbolIndex::build(index, map, policy)` is pure: it drops every file `PathPolicy::is_restricted` reports, as a symbol source, an import source, an import target and a test or subject. It fills `root_path` from `map.root_for_path` and marks every item `Heuristic`. It emits `Imports` (resolved per `context.md` §11) and `TestOf`. Symbols are sorted by (path, line, column, name, container), and relations are sorted and deduplicated. `find(query, limit)`: exact (case-sensitive equality), then case-insensitive prefix, then case-insensitive subsequence, ordered by provenance, quality, path, line, name. An empty query returns nothing. **Tests:** `symbol_index` 24/24 (12 new): fixtures indexed, `find("upload_retry")` at exact php/python/rust lines; every item `Heuristic`; exact/prefix/fuzzy ranking and limit; `Provenance` `Ord`; a PEM block in a comment above `fn after_the_key` (true line 7; redacted text would give 4); a secret in a name and in a container; restricted `secrets/config.ts` and `.env.local` contribute nothing, with a control build under an empty restricted list proving the policy is the cause (`.env.local` alone would be vacuous: it detects as `text` and has no declarations); two roots plus the repository root; JS/TS and Python import resolution, Rust `use` external, and no `File` edge outside the index; `TestOf` for Rust, TS `.test`/`.spec`, Python and Go; no `TestOf` across roots; byte-identical JSON from two builds and from a reversed `files` list. workspace-engine `--lib` + `repository_map` + `semantic_search`: 473 run, 471 passed in the sandbox. The 2 `semantic_search` failures were 300 s timeouts: the sandbox sets `TMPDIR=/tmp/claude-502`, so the tests missed the embedding model cached in the real temp dir and tried to download it. Re-run with the real `TMPDIR`, outside the sandbox: 2/2 pass. `cargo fmt --check`, `cargo clippy -p workspace-engine --all-targets --locked -D warnings` and `typos` clean. Eval harness not run: nothing reads `declarations` or the symbol index yet, so no prompt or context changes. **Mutations** (each failed, then reverted): extraction from redacted content → the PEM line test and the secret-name test fail; restricted filter removed → the restricted test fails; an unresolved relative specifier becomes a `File` edge → the import test and the restricted test fail; `Provenance` variants swapped → the `Ord` test fails; name not scanned, and container not scanned → the secret-name test fails for each; same-root check removed → the cross-root test fails; relations not sorted → the determinism test and the import test fail; prefix tier removed → the ranking test fails. `relations.dedup()` removed **survived** at first, because no fixture made a duplicate edge. The import fixture now imports both `./upload` and `./upload.ts`, which resolve to the same file, and the mutation fails. **Deviations:** a file-level relation end (the importing file, the test file) is `SymbolRef { path, name: "", line: 0 }`, because §5.2's `Imports` and `TestOf` are file edges and `SymbolRef` has no optional fields. `MatchQuality` is a new public enum. An import whose target is restricted stays `External`, and so does a `../` that climbs above the repository root. Python `from . import x` resolves to the package's `__init__.py`. `TestOf` conventions are those of §11, plus: for a JS/TS or Python test inside one of its root's `test_paths`, the subject is also looked for at the same relative place under `<root>/src/` and `<root>/`, after the test's own directory. Rust pairs only a direct child of a test directory with `src/<name>.rs`. The first indexed candidate in the same root wins. **For Task 3:** `SymbolIndex` has no `schema_version` or per-file `content_hash` yet; `FileRecord.content_hash` is the hash to store. `build` takes a `RepositoryMap`, and the Interface reference's `RepositoryMap::build(index, policy)` takes a **`CommandPolicy`**, not a `PathPolicy`. The tests build it with `CommandPolicy::new(config)`. Running `semantic_search` inside the sandbox needs the real `TMPDIR` (see Tests). |
| 3 · Persistence, rebuild reporting, engine API, CLI | Done 2026-10-15 | **Landed:** `SymbolIndex` gains `schemaVersion` (`SYMBOL_INDEX_SCHEMA_VERSION` = 1) and `files: Vec<SymbolFile { path, contentHash }>`, the non-restricted indexed files sorted by path, so the stored file is the serialised `SymbolIndex`. `SymbolIndexStore::new(data_dir, audit_log)`, `path(data_dir, repository_id)` (`<data_dir>/symbol-index/<repository_id>.json`) and `load_or_build(&index, &map, &policy) -> Result<(SymbolIndex, SymbolLoad)>`, following `RepositoryMapStore`: it reads `schemaVersion` before the shape, a foreign `repositoryId` is `Corrupt`, and it writes through `repository_map::write_replacing` (now `pub(crate)`). `SymbolLoad { Reused, Built, Rebuilt { reason } }` reuses spec 24's `RebuildReason` (`Corrupt`, `SchemaMismatch { found }`, `Stale`) rather than a copy. `build` is now `assemble(index, map, policy, None)`. With a stored index, a file at the same content hash keeps its stored symbols from either tier, with `rootPath` re-stamped from the current map. A changed or new file is taken from `FileRecord.declarations` as heuristics, and a file no longer indexed (or now restricted) is dropped. Heuristic relations are always derived again, because import resolution depends on which files exist. A stored non-heuristic relation is kept only when its `from` file and its `File`/`Symbol` target file are both unchanged. `Reused` means the assembled index equals the stored one, and nothing is written. Any difference is written and is `Rebuilt { Stale }`, including a change that re-extracts no file (a new manifest moving a root). `Built` (nothing stored) is not audited, as for the map. `symbol_index_rebuilt` records `repositoryId`, `reason`, `fileCount` (files taken from the index pass), `totalFiles`, `durationMs`, and `foundSchemaVersion` on a mismatch. `WorkspaceEngine::symbol_index(root)` (cached index → stored map → store) and `find_symbols(root, query, limit)`. `Provenance::as_str` and `SymbolKind::as_str` (the serde names), and `symbol_matches_json` for the CLI, which has no serde dependency. CLI `damaian symbols <repo> <query> [--json]`: a load line, then `name  kind  path:line  root  provenance` per match (root `.` for the repository root), at most 50; `--json` prints `{"load":…,"matches":[…]}` with the same load shape as `repo-map --json`. Checked by hand on this repository: `load_or_build` → two `heuristic` methods at `repository_map.rs:811` and `symbol_index.rs:427`, then `reused`. **Tests:** `symbol_index` 31/31 (7 new, 1 ignored measurement): built then reused untouched, with one `contentHash` per file and no temp file left; garbage bytes → `Corrupt`, audited once with `fileCount` 2 and a numeric `durationMs`, then `Reused`; another `schemaVersion` → `SchemaMismatch { found }`, audited; another repository's index → `Corrupt`; stored symbols hand-marked `language_server` on line 7, plus a stored `references` relation alpha → beta: `Reused` keeps them all; after editing `beta.rs` and adding `delta.rs`, alpha's language-server symbol survives, beta's is replaced by its heuristics, the relation is dropped, and the audit is `stale` with `fileCount` 2 of 3; deleting `alpha.rs` drops it; a new `package.json` moves an unchanged file's `rootPath`; the engine's `find_symbols` finds `alpha` at `src/alpha.rs:1`, `Heuristic`, and the second `symbol_index` is `Reused`. `symbol_index` + `repository_map`: 83 passed. `scripts/quality-gate.sh fmt typos` and `cargo clippy -p workspace-engine -p damaian-cli --all-targets --locked -D warnings` clean. Workspace clippy and nextest are left to CI. No eval run: nothing in prompts or context reads the index yet. **Measurement (proposal §7)**, release build, this repository (309 indexed files, 4,743 symbols, 851 relations), `load_or_build` alone, three runs: `Built` 3.3–4.2 ms, `Reused` 7.9–8.1 ms, file 1,632,054 bytes (pretty-printed JSON, as the map store writes). Reuse is slower than a build: reading and parsing 1.6 MB costs more than turning the index pass's declarations into symbols. That confirms `context.md` §4.3: persistence pays off only for language-server results (Task 6), not for heuristics. The test is `measures_load_or_build_on_this_repository`, `#[ignore]`d, with its command in its doc comment. **Mutations** (each failed, then reverted): schema check removed → the schema test fails; a changed file's stored entries kept (reuse by path, not hash) → the incremental test fails; no audit on rebuild → the corrupt, schema and incremental tests fail; a stored relation's `Symbol` target not checked → the incremental test fails; never `Reused` → 4 tests fail. `rootPath` not re-stamped **survived** at first; the root test was added for it and now fails. **Deviations:** the reasons are spec 24's `RebuildReason`, not a new enum. The file count is split into `fileCount` and `totalFiles`. An incremental update is reported as `Rebuilt { Stale }`, the map store's meaning of a readable but outdated store. **For Task 4 and later:** the store has no write method besides `load_or_build`. Task 6, merging language-server results as each file completes, needs one, built on `write_replacing`, and must keep `files` hashes in step. Stored language-server relations survive only between unchanged files; Task 9's reference cache needs every referencing file's hash, which `files` already holds. `SymbolKind::as_str` and `Provenance::as_str` are there for Task 8's rendering. |
| 4 · LSP framing and protocol, without a process | Done 2026-10-15 | **Landed:** new `lsp.rs`, registered and re-exported from `lib.rs`. No process, hub, config key or registry kind; `mcp.rs`, `chat.rs` and `build_model_prompt` untouched. **Framing:** `write_message(&mut impl Write, &Value)` writes `Content-Length: <bytes>\r\n\r\n` and the body, then flushes. `read_message(&mut impl BufRead) -> Result<Option<Value>>`: `None` is a clean end of stream before a message began (a server that exited). The header name is case-insensitive, other headers (`Content-Type`) are ignored, and a missing, non-numeric or duplicated `Content-Length`, a header line with no `:`, a non-UTF-8 header, a stream ending mid-header or mid-body, and a body that is not JSON are errors. `MAX_LSP_MESSAGE_BYTES` (16 MiB) is checked before any body byte is read. The body is read through `take(N)` into a buffer that starts at 64 KiB and grows, so a length that lies about a body that never comes costs only the bytes that arrived. Header lines are bounded too (8 KiB each, 32 per message), so a server that never sends a newline cannot grow a buffer. **JSON-RPC:** `LspClient<W: Write>` owns the writer and a `pending` table of `u64` id → `LspRequest`. Ids start at 1. `LspRequest` is a closed enum of the six requests (`initialize`, `shutdown`, `documentSymbol`, `definition`, `references`, `implementation`); `initialized`, `exit`, `didOpen` and `didClose` are notifications. `receive(Value) -> Result<LspEvent>` classifies one message: `Response { id, request, outcome }` (an error response is `Err(ClientError::Io("Language server refused <method>: <message> (code N)"))`), `Diagnostics(params)`, `Notification { method, params }`, `ServerRequest { method }` (already answered), or `Unmatched { id }`. Server requests are answered inside `receive`: `workspace/configuration` gets one `null` per item, `client/registerCapability` and `window/workDoneProgress/create` get `null`, anything else gets `-32601`, so no server waits on a reply. `await_response(id, next, on_event)` feeds messages from `next` until `id`'s response arrives and hands every other event to `on_event`; `next` returning `None` is "server gone" and removes the pending entry. `forget(id)` drops a timed-out request, so its late response is `Unmatched`. `initialize` advertises only what is used: no dynamic registration, `hierarchicalDocumentSymbolSupport`, `symbolKind` 1–26 (so a server reports `Struct` rather than falling back to the original 18 kinds), `linkSupport` for `definition`/`implementation`. It does not advertise `workspace.configuration` or `window.workDoneProgress`; those requests are answered anyway. **Mapping:** `RepositoryUris { root }`: `root_uri`, `uri_for(relative)` (percent-encodes everything but RFC 3986 unreserved and `/`), and `relative_path(uri) -> Option<String>`, which accepts `file:///…` and `file://localhost/…`, decodes `%XX` in either case, and returns `None` for another scheme or host, a bad or truncated escape, non-UTF-8 bytes, `?`/`#`, a `..` segment (also percent-encoded), a sibling such as `/repo-two` for root `/repo`, and the root itself. `symbol_kind_from_lsp(u64)` follows `context.md` §8 (`Module` ← 2–4, `Class` 5, `Method` ← 6 and 9, `Enum` 10, `Interface` 11, `Function` 12, `Variable` 13, `Constant` 14, `Struct` 23; every other kind `None`). `symbols_from_document_symbols(result, uris, path, root_path, scanner) -> Vec<Symbol>` takes both shapes, marks every symbol `LanguageServer`, converts 0-based → 1-based, and sorts by (line, column, name, container). `symbol_refs_from_locations(result, uris) -> Vec<SymbolRef>` takes `null`, a `Location`, `Location[]` or `LocationLink[]` (line from `targetSelectionRange`). `findings_from_diagnostics(params, uris) -> Option<FileDiagnostics { path, findings }>`: severity 1 `Error`, 2 `Warning`, 3 and 4 `Info`; 1-based start and end; `code` from a string or a number; `FindingSource::LanguageServer`. **Tests:** `lsp` unit tests 25/25: framing round trip with `\r\n` and multibyte text in the body (header asserted to be the byte length, which the test checks differs from the char count); a hand-framed body holding a raw blank line and multibyte text, followed by a second message, read exactly; case-insensitive header and ignored headers; every malformed case, each asserted by its own message; the oversized claim refused with the reader still at the end of the header, and `Content-Length: usize::MAX` refused; a body of exactly 16 MiB accepted; a header line with no newline cut at 8 KiB (reader position asserted); request ids and methods in order, and the `initialize`, `references` and position params; a response after a log notification, a diagnostics notification, another request's response and an unknown id still matches its request, and each of those is routed as its event; an error response becomes a `ClientError` naming the method, message and code; EOF before a response is an error and clears the pending entry; a forgotten request's late response is `Unmatched`; the four server-request replies byte for byte; non-JSON-RPC messages are errors; hierarchical and flat `documentSymbol` responses give the same four `Symbol`s; the struct's line is its `selectionRange` (11), not its `range` (7, the doc comment); null and empty results; a secret in a name and in a parent's name drops the symbol and its child; the kind table for every LSP kind 1–27; URIs inside and outside the root, including percent-encoded paths; a URI built by `uri_for` maps back; locations, location links and an out-of-repository location; each severity, 1-based ranges, codes and multi-line details; diagnostics outside the root are `None`, and an empty set is a clearing `FileDiagnostics`. `symbol_index` 31/31 (1 ignored). `cargo clippy -p workspace-engine --all-targets --locked -D warnings` and `scripts/quality-gate.sh fmt typos` clean. Workspace clippy and nextest are left to CI. No eval run: nothing calls `lsp.rs` yet. **Mutations** (each failed, then reverted): length counted in chars → the round-trip test fails; the 16 MiB bound removed → the oversized test fails on its error text (a mid-body error instead) and its reader position, not by allocation; `range` instead of `selectionRange` → the hierarchical/flat test and the line test fail. Further mutations, for tests that could have been vacuous: the header-line `take` bound removed → the header-line test fails on reader position (its error text alone survived this, so the test asserts position; the earlier error-text-only check was replaced); percent decoding skipped → 3 tests fail; the `..` check removed → the URI test fails; children given the grandparent's container → the hierarchical test and the secret test fail; no sort → the hierarchical/flat test fails; lines kept 0-based → 4 tests fail; server requests not answered → the reply test fails; `await_response` accepting any response → the routing test fails; severity 2 as `Error` → the severity test fails; a flat entry for another file kept → the hierarchical/flat test fails; container not scanned → the secret test fails; duplicate `Content-Length` accepted → the malformed test fails; `LocationLink` read from `targetRange` → the location test fails; the pending entry kept on EOF → the EOF test fails. **Deviations:** `read_message` returns `Option`, to tell a clean exit from a broken stream. Symbol mapping drops a name or container the secret scanner flags, as `context.md` §4.2 does for the heuristic tier; Task 4 did not ask for it. Flat `SymbolInformation` has no `selectionRange`, so its line is `location.range`'s start, and an entry whose URI is not the requested file is dropped. A child's container is its parent's name verbatim, whatever the parent's kind, so a method under rust-analyzer's `impl Foo` object gets container `impl Foo` (or `impl Trait for Foo`), not the heuristic tier's `Foo`; not checked against a real server. Nothing maps to `Type`: `TypeParameter` is excluded per §8, and rust-analyzer reports type aliases as `TypeParameter` (per its source, not checked here), so they stay heuristic. Columns are the server's UTF-16 offsets plus one; the heuristic tier counts characters, so the two differ only on a line with non-ASCII text before the name. An absent diagnostic severity is `Error` (as editors read it) and an out-of-range one is `Info`. A multi-line diagnostic message is kept whole as the details, and its first line is the summary; the diagnostic's `source` field (`rustc`) is not kept. A location's `SymbolRef` has an empty `name`. An error response is `ClientError::Io`. **For Task 5 and later:** the hub drives one `LspClient<ChildStdin>` per server, on the thread that owns it. The reader thread wraps stdout in a `BufReader` and loops `read_message`, sending each `Some(value)` down a channel. `Ok(None)` or `Err` ends the thread and drops the sender: after a framing error the stream cannot be resynchronised, so treat it as a crash. On the owning thread, `await_response(id, next, on_event)` with `next` = `recv_timeout(remaining)` mapped to `Ok(Some(v))` / `Err(timeout)` / `Ok(None)` on disconnect; on timeout call `forget(id)`. Route `LspEvent::Diagnostics` through `findings_from_diagnostics` and drop restricted paths there: this layer knows nothing of `PathPolicy`, and neither does `symbol_refs_from_locations`, so Task 9 filters restricted references too. Lifecycle: `initialize(&uris, std::process::id())` → await → `initialized()`; `shutdown()` → await → `exit()`. `did_open`'s language id can be `detect_language`'s name: the eight languages' names are LSP ids. `RepositoryUris::new` must get the root in the same form the engine uses everywhere and the server is given; nothing here canonicalises, and a `/var` vs `/private/var` mismatch would put every URI outside the root. |
| 5 · Configuration, the language-server hub and process lifetime | Done 2026-10-15 | **Landed:** `language_server.<language>=<command line>`: `Config.language_servers` and `ConfigOverlay.language_servers` (`BTreeMap` by language), `LANGUAGE_SERVER_LANGUAGES` (the eight `extract_declarations` languages, `config.rs`), and a parse error naming the key for any other language or an empty command. Every exhaustive site handles it: `classify_overlay_fields!` (`Capability`), `apply_overlay_scoped` (applied at user and admin scope; refused as `Forbidden` at repository and profile scope, per key `language_server.<language>`), both `to_policy_text`s, `split_profile_keys` (refused, `Forbidden`), and `rule_label` ("Language server for <language>"). `ProcessKind::LanguageServer` (`"language_server"`). In `lsp.rs`: `LanguageServerHub` (`global()`, `with_limits(LanguageServerLimits)`, `request`, `notify`, `status`, `shutdown_all`, `shutdown_global`, `Drop`), `LanguageServerLaunch` (`from_config(config, root, language, session_id)`, `None` when the language is not configured; the root is canonicalised and `repository_id` is `repository_id_for_root`), `LanguageServerState` (`Starting`, `Ready`, `Unavailable { reason }`, `Crashed { restarts }`), `LanguageServerStatus { language, command, state, pid }` (serde: `state` flattened as a camelCase tag), `LanguageServerError` (`Unavailable { reason }`, `TimedOut { method }`, `Refused { message }`), and `LANGUAGE_SERVER_REQUEST_TIMEOUT` (5 s), `LANGUAGE_SERVER_IDLE_SECS` (600) and `LANGUAGE_SERVER_SHUTDOWN_GRACE` (500 ms). `spawn_registered` makes `StdioTransport`'s spawn (`mcp::shell_split`, now `pub(crate)`; piped stdin and stdout, null stderr, `process_group(0)`, the repository root as working directory). It opens the registry before spawning and registers before returning, touching nothing else. A failed registration kills and reaps the child. A reader thread loops `read_message` into a channel, and `Ok(None)` or a framing error ends it, which reads as a crash. `initialize` → `await_response` with `recv_timeout` → `initialized`, all under the request timeout. A timed-out request is `forget`-ed. Entries are keyed by `(repository_id, language)`, each with a short-held `meta` lock (state, restarts, last use, PID) and a per-request `process` lock, so a caller never waits behind a request to learn the state. A request to an entry that is not `Ready` returns `Unavailable` at once. The first request claims the start. A crash (EOF, a write error, or a child found exited by `try_wait` on the way in) kills and reaps the child and marks `Crashed { restarts }`. The next request restarts it once (`LANGUAGE_SERVER_MAX_RESTARTS` = 1). After a second crash the entry stays down until idle expiry drops it. Stopping is `shutdown` (awaited for the grace period), `exit`, a grace period polling `try_wait`, then `Child::kill` and `wait`. The registration drops after the wait. It runs on idle expiry (entries removed on hub access and stopped on a background thread, and by a reaper thread started with the first entry that holds the hub weakly and ends when the hub drops), from `shutdown_all` (in parallel, under `thread::scope`) and from `Drop`. No new signal handling. `WorkspaceEngine::language_servers()` (the global hub) and `language_server_launch(root, language, session_id)`. `desktop-shell/src/lib.rs`: `run_server_with_ready` calls `LanguageServerHub::shutdown_global()` after its listener loop. Nothing else there changed. `chat.rs` and `build_model_prompt` untouched. **Tests:** `symbol_index` 39/39 (8 new; 2 ignored: the proposal §7 measurement and the fake-server helper). No `language_server.*` configured: no launch for any of the eight languages, none for `python` when only `rust` is configured, empty status, no registry entry, no spawn. User `language_server.rust=/usr/local/bin/rust-analyzer` plus repository `language_server.rust=/bin/sh` and `language_server.python=./tools/pyls`: both refused `Forbidden`, the user's value stands, `python` is unset, `RepositoryTrustStore::review` audits `repository_config_key_rejected` naming the key, and the refused value is not in the log. An unknown language (`cobol`) and an empty command are parse errors; the overlay round-trips. A normal fake server: the request answers `[]`. While it runs there is one registry entry, kind `language_server`, session `ses_task5`, with the status PID. After `shutdown_all` there are no entries, the PID is gone, the status is empty, and the log shows `shutdown` and `exit`, with one spawn. Idle expiry at 300 ms, without touching the hub: the entry is removed and the PID gone, by the reaper alone. A server that never answers `initialize`, with a 2 s timeout: `Unavailable` naming `initialize`, after at least the timeout. The second request returns `Unavailable` in under 200 ms, with no registry entry and one spawn. `exit_after_1`, six requests: two answers, two spawns, status `Crashed { restarts: 1 }` with no PID, no registry entry, the last two requests `Unavailable`, and a seventh request in under 200 ms with still two spawns. A hand-written `language_server` entry for this test's own PID with a start time one microsecond off, whose owner is gone: `sweep_with(CrashedOwners)` with the real identity function and an injected kill refuses it (`refused() == 1`, no kill, `orphan_process_kill_refused` with `"kind":"language_server"`, entry removed). `permission_profiles` 51/51 (1 new, 2 extended). A `language_servers` weakening case, which also feeds the profile-split agreement test. `every_rule_has_a_human_name` gains `language_server.rust` ("Language server for rust") as a rule and a refused `language_server.python`. New `a_profile_cannot_carry_a_language_server`: a custom profile's `rust` and `go` servers are refused `Forbidden`, the user's `rust` stands, `go` is unset, `command_access=read_only` still applies, and the export names no server. `lsp` unit tests 26/26 (1 new: `a_server_is_registered_before_anything_is_written_to_it`, with its ignored helper). `repository_config_trust` 48/48, unchanged and unedited. `process_registry` 0 run, 5 ignored (all real-process tests, as before). `cargo clippy -p workspace-engine -p desktop-shell --all-targets --locked -D warnings` and `scripts/quality-gate.sh fmt typos` clean. Workspace clippy and nextest are left to CI. The full seven-check gate passed in CI on 2026-10-15. No eval run: nothing in prompts, context or tools reaches the hub yet. **Mutations** (each failed, then reverted): unlimited restarts → the crash test fails; idle expiry disabled → the idle test fails at its 20 s wait; the repository scope allowed → the refusal test fails, and so does `a_profile_cannot_carry_a_language_server`; `from_config` falling back to `rust-analyzer` → the no-server test fails; `shutdown_all` a no-op → the lifecycle test fails; no `shutdown`/`exit` before the kill → the lifecycle test fails on its log; an `Unavailable` entry restarted → the never-answers test fails; the sweep's start-time check removed → the sweep test fails with a recorded kill; the `rule_label` arm removed → `every_rule_has_a_human_name` fails; an unknown language accepted → the parse test fails; `split_profile_keys` not refusing the key → the split-agreement test fails. Register after the first write to stdin **survived** the planned check, the fake server logging whether its entry existed when it read `initialize`. Creating the entry takes microseconds, so the parent wins that race even in the wrong order, and the check cannot fail. Spawn and registration were split into `spawn_registered`, which writes nothing. The unit test asserts the entry exists while both pipes are still unused. The same mutation, moved after the `initialize` write, fails it. The black-box assertion is kept as a sanity check, with a comment that it proves nothing about the order. **Deviations:** the fake server cannot speak LSP on stdout, because libtest prints `running 1 test` there before the helper runs. It writes to fd 3. The command line is `/bin/sh -c "export …; exec \"$0\" --ignored --exact fake_language_server_helper 3>&1 1>/dev/null" <test binary>`: not a login shell, and it `exec`s, so the recorded PID is the helper's. Its mode and paths travel in that command line, not the parent's environment, so parallel tests cannot see each other's. The fake-server tests themselves are not `#[ignore]`d (only the helper is), as Task 5 lists them; they spawn only the test binary. A crash found on the way into a request restarts the server within that request, while a crash during a request fails that request and the next one restarts. Either way it is one restart per entry, so the crash test counts totals, not which request answered. `Unavailable` is not retried until idle expiry drops the entry. An entry keeps the launch it was created with, so an edited `language_server.<language>` takes effect after expiry or a relaunch. Notifications that arrive during a wait (diagnostics included) are dropped until Task 7. **Not covered, outside Task 5's files:** the global hub is a static, so its `Drop` never runs. `shutdown_global` sits after the shell's listener loop, which the standalone shell only leaves by a signal (`sweep_own` covers that). The Tauri app quits through `tauri::Builder::run`, which exits the process from `desktop-app`, and that `main` installs no shutdown handler. A server there gets EOF on stdin, which most servers treat as exit, and otherwise the next launch's sweep kills it. Hooking the app's exit belongs with Task 10 or 11, or a `desktop-app` change. A spec 57 worker gets its own hub, whose servers its own `sweep_own` kills; `worker.rs` is untouched. `docs/PLAN/` is not in this worktree, so this gap is recorded here rather than in `OBSERVATIONS.md`. **For Task 6 and later:** get a launch with `engine.language_server_launch(root, language, session_id)` (`None`: heuristics only), and drive `engine.language_servers()`. Open a file with `hub.notify(&launch, |client, uris| client.did_open(&uris.uri_for(path), language, 1, text))`. Ask with `hub.request(&launch, |client, uris| client.document_symbol(&uris.uri_for(path)))` → `Result<Value, LanguageServerError>`. Map the result with `symbols_from_document_symbols(&value, &RepositoryUris::new(&launch.root), …)`, using `launch.root` (canonical) for the URIs. Then `did_close`. Every hub call blocks its caller: at most the request timeout, plus spawn and `initialize` (each under the timeout) when it starts or restarts the server. Requests to one server serialise on its process lock. So the eager pass runs on its own thread and nothing a turn or handler waits on calls the hub. `TimedOut` leaves the server `Ready` (move on to the next file). `Refused` is an error response. `Unavailable` covers starting, failed, crashed and just-crashed, so stop the pass on it and leave the rest heuristic. `status(repository_id)` shows why. Task 7 routes `LspEvent::Diagnostics`: today `ServerProcess::await_response` drops every non-response event, and diagnostics that arrive between requests wait in the channel until the next request drains them. The language id passed to `did_open` is the configured language name, which is the LSP id for all eight. |
| 6 · Eager definitions and merging the tiers | Done 2026-10-15 | **Landed:** `symbol_index.rs`: `MERGE_LINE_TOLERANCE` (3). `merge(heuristic, language_server) -> Vec<Symbol>`: a heuristic symbol is dropped only when a server symbol has the same `name` and `path` and a line within the tolerance. Every other heuristic symbol is kept, the server's symbols are added, and the result is sorted by (path, line, column, name, container, provenance), so `Provenance`'s `Ord` breaks the last tie. `assemble` now sorts the same way. `SymbolFile.languageServer: bool` (serde default, omitted when false) records that a server's result for that `contentHash` is merged in. A server that reports nothing for a file still sets it, so the file is not asked again. `assemble` keeps the mark only for an unchanged file. `SymbolIndexStore::merge_language_server(repository_id, path, content_hash, symbols) -> Result<bool>` is the store's second write: it re-reads the stored index, and if the file is still stored at that hash, merges the file's stored *heuristic* symbols with the server's (so a re-merge replaces rather than stacks), sets the mark, and writes once through `write_replacing`. A missing or unreadable index, or a file at another hash, writes nothing and returns `false`. A process-wide `STORE_WRITES` mutex is held around each read-modify-write, in `load_or_build` too, so the pass and a reader's rebuild cannot overwrite each other. It is held for one file read, an in-memory merge and one write, never across a server request. `lsp.rs`: `run_definition_pass(hub, launches, index, map, policy, store, scanner) -> Result<DefinitionPassReport { covered, skipped, missed, stopped }>`. It takes the snapshot with `load_or_build`, then walks `index.files` in index order. It skips a file whose language has no launch, whose language's server already stopped, that the policy restricts (`context.md` §7: the index holds restricted files), or that is marked at its current hash. Otherwise it reads the file from `launch.root`. If the bytes no longer hash to the index's `content_hash`, or are not UTF-8, it skips the file (left to a later pass). Then `didOpen` (version 1, the configured language as the id) → `documentSymbol` → `didClose`, each through the hub under its request timeout. On a result: `symbols_from_document_symbols` with `RepositoryUris::new(&launch.root)` and `map.root_for_path`, then `merge_language_server`. `TimedOut` and `Refused` go to `missed` and on to the next file. `Unavailable` (from any of the three calls) goes to `stopped[language]`, and that language's remaining files are not sent. Other languages carry on. `DefinitionPass` (`is_finished`, `join`) wraps the thread (`damaian-lsp-definitions`). `PassClaim` (crate-private) keeps one pass per repository id per process and is released when the thread ends. `workspace_engine.rs`: `start_definition_pass(root, session_id)` (global hub) and `start_definition_pass_with(hub: impl Deref<Target = LanguageServerHub> + Send + 'static, root, session_id) -> Result<Option<DefinitionPass>>`. It returns `None` with no server configured or a pass already running. Otherwise it spawns at once. The thread does `IndexCache::get_or_build`, the map store and the pass, so the caller does no indexing either. `find_symbols` is unchanged: it reads the stored file through `load_or_build` and never touches the hub. Re-exported: `DefinitionPass`, `DefinitionPassReport`, `run_definition_pass`, `merge`, `MERGE_LINE_TOLERANCE`. `chat.rs` and `build_model_prompt` untouched. **Tests:** `symbol_index` 47/47 (8 new; 2 ignored, the measurement and the fake-server helper). The fake server gained modes, which combine with commas: `no_document_symbol`, `crash_on_symbol_<n>` (exits on reading its `n`-th `documentSymbol`, unanswered, so the crash point is exact rather than racing a later write), and `delay_<ms>`. It also gained `DAMAIAN_FAKE_LSP_SYMBOLS`, a JSON file of path → result: listed files get their result, every other file `null`. It now logs each `documentSymbol` URI and each notification. Tests: (1) the server answers `src/upload.rs` only, placing `upload_retry` on line 6 against the heuristic's 5. Engine `find_symbols("upload_retry")` gives `src/upload.rs:6` `LanguageServer` first, then `src/other.rs:1` and `tools/upload.py:1` `Heuristic`. Unreported `upload_once` in the covered file and `main` in an uncovered one stay heuristic. Exactly the three Rust files are asked, not the Python file and never the restricted `src/private.rs`, whose name appears nowhere in the server's log. A second pass skips all three and sends nothing. (2) `merge`: the same name in two files supersedes only in the server's file. (3) `merge`: 41/42 merges and 41/51 keeps both, plus the edges 41/44, 44/41 (merged) and 41/45 (kept). (4) `merge`: a heuristic `Trait` is superseded by the server's `Interface` at the same line, and the trait's heuristic `put` stays. (5) `run_definition_pass` directly: two files covered, then both skipped. Then `beta.rs` is edited and a reader rebuilds: alpha keeps its server symbol, beta is heuristic. The next pass asks for beta only. (6) `merge_language_server` with a wrong hash writes nothing, and with the right one marks the file. The next `load_or_build` is `Reused`. (7) Six files, `delay_1000,crash_on_symbol_4`: a second start while the first runs is `None`. A mock-model turn (`MockModelAdapter` through `chat_orchestrator.ask`) started while the pass runs completes with its response. Once the store shows a covered file, `find_symbols` returns in under 1 s, with both tiers in its result and the pass still running. The report has `stopped["rust"]`, 3 covered, 4 requests and 1 spawn (the pass does not use the hub's restart). The covered files are `LanguageServer` at line 2, the rest `Heuristic` at line 1. The status is `Crashed { restarts: 0 }` and the registry is empty. (8) `no_document_symbol`, 2 s timeout, 3 files: `start_definition_pass_with` returns in under 500 ms. A `find_symbols` after the first request returns in under 1 s with the pass still running. The pass's 3 files are all `missed`, with 3 requests, 3 opens matched by 3 closes, and a duration of at least 3 timeouts and under 3 timeouts + 20 s. The server stays `Ready`. `permission_profiles` 51/51, `repository_config_trust` 48/48 (unchanged, unedited), `process_registry` 0 run, 5 ignored, as before. `--lib` `lsp` 26/26. `--lib` `symbol_index` has no unit tests (0 run): the spec puts engine tests in `tests/symbol_index.rs`. `cargo clippy -p workspace-engine -p desktop-shell --all-targets --locked -D warnings` and `scripts/quality-gate.sh fmt typos` clean. The slowest scoped run was `symbol_index` at 11 s wall. Workspace clippy and nextest are left to CI. The full seven-check gate passed in CI on 2026-10-15. No eval run: nothing in prompts, context or tools reads the pass's output yet (Task 8 does). **Mutations** (each failed, then reverted): a heuristic symbol dropped when the server reports anything in its file → (1) fails (`upload_once` gone) and (4) fails (`put` gone); tolerance 0 → (3) fails; the reader waiting on the pass (`find_symbols` spinning while a pass is registered) → (8) fails, "a reader waited on the server: 6.02s", and (7) fails; the pass run inline before `start_definition_pass_with` returns → (8) fails, "starting the pass waited: 6.04s", and (7) fails; the restricted check removed → (1) fails, `src/private.rs` asked; the covered-file skip removed → (1) and (5) fail; the mark not kept by `assemble` → (1) and (5) fail; the language stopped on `TimedOut` → (8) fails, 1 missed of 3; the pass carrying on after `Unavailable` → (7) fails: the hub restarts the server and covers `f5` and `f6`; the merge ignoring the path → (2) fails (the end-to-end test (1) survived it, because `src/other.rs`'s `upload_retry` is 5 lines from the server's, which is why (2) exists); `merge_language_server` ignoring the hash → (6) fails; kinds compared strictly → (4) fails, listing `Store` twice. **Not falsified:** the `STORE_WRITES` lock in `load_or_build`. Its failure is a lost update between two writers interleaving within milliseconds, and no test here can force that order without the kind of in-process race that let Task 5's registration-order mutation survive. It is kept on reasoning: `assemble` re-derives from what it read, so an unlocked reader could write back a pre-merge copy. **Deviations:** kinds are not compared in the merge. Task 1 records Python, Kotlin and PHP methods as `Function`, so a strict kind match would list each such method twice once a server reports it as `Method`. The server's kind wins, which makes `Interface`/`Trait` compatible by construction, and (4) pins that. `SymbolFile` gained `languageServer` without a schema bump: it defaults to false, so an index written before this task reads as "not covered" and the pass covers it. The pass's checks are `Unavailable` → stop *that language* (the others continue); and a file that changed after the index pass, or is not UTF-8, is skipped rather than sent. The pass never restarts a crashed server itself: it stops on the first `Unavailable`, and the hub's one restart is spent by the next pass or the next on-demand request. Task 6 lists `DAMAIAN_MOCK_MODEL_RESPONSE` for the turn. That variable only selects `MockModelAdapter` in the CLI, and setting process environment in a parallel test is `unsafe` and racy, so the test uses `MockModelAdapter` directly through `ChatOrchestrator::ask`, the engine's own entry point. The pass rides on the hub's existing `notify` and `request`. Nothing in `lsp.rs`'s process layer changed. **Measurement (proposal §7): not measured.** `~/.cargo/bin/rust-analyzer` is still only the rustup proxy ("Unknown binary 'rust-analyzer' in official toolchain 'stable-aarch64-apple-darwin'", checked 2026-10-15), and `typescript-language-server`, `pyright-langserver` and `gopls` are not on `PATH`. No server is installed without the user's go-ahead (`context.md` §13), so there is no `#[ignore]`d real-server test yet. `rustup component add rust-analyzer` would make one possible against this repository. **Not wired yet:** nothing in the app calls `start_definition_pass`. The engine method is ready, but the caller belongs to the task that gives the pass a moment to run. That is the shell, on repository open or first symbol request (Task 10), or Task 8's context item, which must start it and never join it. The CLI should not: a short-lived process would spawn a server only to orphan it to the next sweep. **For Tasks 7–9:** *diagnostics during the pass* arrive on the pass's thread, inside `ServerProcess::await_response` during each `documentSymbol` wait, and today go to its `|_event| {}`. `didOpen` is what makes most servers publish, so the pass is where Task 7's diagnostics will mostly come from. The hub keeps them per file there, not the pass. Notifications that arrive between requests wait in the channel until the next request drains them. *How the stored index is written:* only through `SymbolIndexStore`, either `load_or_build` (a whole assembled index) or `merge_language_server` (one file), both under `STORE_WRITES`. Task 9's reference cache needs a third writer, under the same lock, that re-reads, checks every involved file's `contentHash` against `files`, and writes once. *How a reader sees the current snapshot:* `engine.symbol_index(root)` → `load_or_build`, which reads the stored file and keeps every unchanged file's symbols and mark, so whatever the pass has merged so far is in it. It never touches the hub, so Task 8's context item reads it the same way and must never call `start_definition_pass(..).join()`. `IndexCache` refreshes only every 5 minutes with the watcher off, so a file edited since then is skipped by the pass (hash mismatch) until the cached index catches up. |
| 7 · Diagnostics as findings | Done 2026-10-16 | **Landed:** `lsp.rs`: each hub entry (`Slot`) holds `diagnostics: Arc<DiagnosticsByFile>`, a `Mutex<BTreeMap<path, Vec<FindingDraft>>>` shared with its `ServerProcess` and kept across restarts. `ServerProcess::await_response`'s event callback, which was `|_event| {}`, now passes `LspEvent::Diagnostics` to `DiagnosticsByFile::publish`: `findings_from_diagnostics` (a URI outside the root is `None`, so dropped), then the entry's `PathPolicy` (a restricted file is dropped), then the file's set is replaced, an empty set included. Every other event is still dropped. The map's lock is held only to replace or copy an entry, never across a request. `LanguageServerLaunch` gains `policy: PathPolicy`, set by `from_config` from the same config (`PathPolicy` now derives `PartialEq, Eq` so the launch keeps its derives). `LanguageServerHub::diagnostics(repository_id) -> Vec<FileDiagnostics>`: a copy of every entry's sets for that repository, sorted by path (stable, so one path's sets stay in language order). It neither waits on nor starts a server, and does not run idle expiry. `workspace_engine.rs`: `record_language_server_findings(session_id, root)` (global hub) and `record_language_server_findings_with(hub, session_id, root) -> Result<usize>`. It canonicalises the root (as the hub keys entries) and reads `hub.diagnostics(repository_id_for_root(..))`. With no non-empty set it returns `Ok(0)` without reading the session log. Otherwise it collects the keys of the session's `LanguageServer` findings, whatever their status, from `read_findings`. Then, per file, it skips a path the engine's current `path_policy` restricts, or one that can no longer be hashed (a file deleted since). It builds each draft through `Finding::new` (which redacts) `.with_file_hash(file_hash(root/path))`, and records it when its key, `(range with path and lines/columns, code, summary as redacted, file_hash)`, is new. No `with_task_id`, no `origin_ref`, no plan or evidence touched. `desktop-shell/src/lib.rs` `handle_findings`: after `session_in_repository` and before `findings_json`, calls it and logs a failure with `eprintln!` without failing the request. `chat.rs` and `build_model_prompt` untouched. **Tests:** `symbol_index` 51/51 (4 new; 2 ignored: the measurement and the fake-server helper). The fake server gained `DAMAIAN_FAKE_LSP_DIAGNOSTICS`, a JSON file of opened path → `[{ uri, diagnostics }]`, a `uri` that is not `file://` being relative to `initialize`'s `rootUri`. On each `didOpen` it re-reads the file and publishes those notifications before reading anything else, and logs `publish <uri>`. Each test drives the hub to a known point with `open_and_settle` (`didOpen` → an answered `documentSymbol` → `didClose`): the server publishes before it reads the request, on the one stream the hub reads in order, so once the answer is in the diagnostics are routed. No sleeps, and no check made from inside the server. (1) An error (`E0308`, LSP 1:4–1:9) and a warning (code `6`, a two-line message) on `src/x.rs`: 2 recorded, `source` `language_server`, no task id, `Open`, the file's hash, the error at 2:5–2:10, the warning at 3:1 with summary its first line and code `"6"`. A second call records 0, and so does one after the file is opened and published again. The warning is dismissed, then a clearing set is published: the hub holds `src/x.rs` with no findings, the call records 0, and the session still has both with `Open` and `Dismissed`. (2) On opening `src/x.rs` the server publishes four sets (the log shows all four): `.env` (default-restricted), `file:///elsewhere/y.rs`, the sibling `<root>-two/src/x.rs`, and `src/x.rs`. The hub holds `src/x.rs` only, and one finding is recorded, `inside diagnostic`. (3) The engine's config restricts `src/x.rs` while the launch's does not: the hub holds the file and nothing is recorded. (4) A task with an `InProgress` step whose evidence is a `FileRead`: after 2 findings are recorded, none has a task id, `read_task_plan` equals the plan before, and the phase stays `Understanding`. Shell `tests` module, 2 new, with their own fake server: `fake_language_server_for_findings_helper` (`#[ignore]`d, re-executed through `/bin/sh -c "… exec \"$0\" … 3>&1 1>/dev/null"` like the engine's, and publishing one error on every opened file). Both put the error into the **global** hub, because that is what `handle_findings` reads, and end with `shutdown_global`. A `GLOBAL_HUB_TESTS` mutex keeps a threaded `cargo test` from shutting one test's server down under the other. `get_findings_returns_a_language_server_diagnostic`: `GET /api/findings` returns it (`language_server`, `error`, code `E1`, `taskId` null, `src/lib.rs` 1:4, `open`), and a second GET still returns one. `get_findings_lists_what_it_has_when_recording_fails`: with the session log made read-only after a compiler finding was recorded, the GET is 200 and lists exactly that finding. The shell's findings tests 6/6 (run outside the sandbox: the session's sandbox refuses binding a local port). `permission_profiles` 51/51, `repository_config_trust` 48/48 (unchanged, unedited), `process_registry` 0 run, 5 ignored, as before. `--lib` `lsp::` 26/26. `cargo clippy -p workspace-engine -p desktop-shell --all-targets --locked -D warnings` and `scripts/quality-gate.sh fmt typos` clean. The slowest scoped run was `symbol_index` at 12 s wall. Workspace clippy and nextest are left to CI. No eval run: nothing in prompts, context or tools reads findings recorded here. **Mutations** (each failed, then reverted): no deduplication → (1) fails, the second call records 2; 0-based start lines kept → (1) fails on the error's range; the task id taken from the session's newest task → (4) fails; the hub's restricted filter removed → (2) fails, the hub holds `.env`; both restricted filters removed → (2) fails the same way; the engine's filter alone removed → (3) fails, 2 recorded (it was added because (2) could not see this one); a clearing set marking the file's recorded findings `Fixed` (with the early return removed, as such an implementation would need) → (1) fails, both `Fixed`; the event callback put back to dropping everything → (1), (2) and (4) fail; `handle_findings` failing the request on a recording error → the read-only test fails; `handle_findings` not recording → the GET test fails with `{"findings":[]}`. **Deviations:** the restricted filter runs twice, as diagnostics arrive (the entry's launch policy, so the hub never holds a restricted file's diagnostics) and when recording (the engine's current policy, since an entry keeps its launch until idle expiry). `path_policy.rs` gained only the `PartialEq, Eq` derive. A diagnostic whose file cannot be hashed (deleted since) is skipped rather than recorded without a hash. The key ignores severity, as the task specifies. A diagnostic that a later edit re-publishes unchanged is recorded again, because its `file_hash` differs, and the earlier one turns `Stale` (spec 22's rule). Diagnostics live and die with their hub entry: idle expiry drops them, and already-recorded findings stay. **For Tasks 8–10:** the per-file diagnostics live in each `Slot`'s `diagnostics` (`lsp.rs`, `DiagnosticsByFile`), filled only on whatever thread is waiting in `ServerProcess::await_response`: the definition pass during each `documentSymbol` after `didOpen`, or an on-demand request (Task 9). A notification that arrives with no request waiting stays in the channel until the next request drains it, so until the pass runs (still not wired; see Task 6), nothing is published. Task 10's server-status endpoint can show counts per file with `hub.diagnostics(repository_id)` next to `hub.status(repository_id)`. Both are cheap and never wait, but `diagnostics` is a full copy, so count before serialising. The findings panel needs no change to show these: they are ordinary findings, `source: "language_server"`, no `taskId`, no `originRef`, ranged and hashed, so they go stale like any other. They are recorded on `GET /api/findings` only, so the panel shows new ones on its next refresh, not live. `POST /api/finding-status` and `POST /api/findings-repair` do not record. |
| 8 · The `symbols` context item | Done 2026-10-23 | **Landed:** `symbol_context_max_tokens` (default 1,600; `0` turns the item off) in `Config`/`ConfigOverlay`, with every exhaustive site: `classify_overlay_fields!` (`Capability`), `apply_overlay_scoped`'s `restrict_only_limit` array (lower-wins at repository scope), both `to_policy_text`s, `ConfigOverlay::set`, `split_profile_keys` (refused in profiles), `equals_base_limit`, and `rule_label` ("Tokens matching symbols may use per turn"). `symbol_index.rs`: `HEURISTIC_HEDGE` ("heuristic — may be inaccurate"), `RenderedSymbols { text, tokens, shown, total }`, `SymbolIndex::render_for_model` (one line per symbol, a header, `kind, path:line`, the hedge only for `Heuristic`, `N of M matching symbols shown.` when it truncates, tokens = `len.div_ceil(4)`), and `find_for_prompt`: the prompt's distinct identifiers (3+ chars, at most 64) looked up exactly, with a case-insensitive prefix match only for code-shaped identifiers (an underscore, a digit or an inner capital), ordered by provenance, quality, path, line and name, each symbol listed once at its best quality. `context_manager.rs`: `SymbolSource { store, path_policy, max_tokens }`, passed to `ContextManager::new` by the engine; `build_context` loads the map once (`load_map` runs when either ceiling is non-zero, so one `0` does not disable the other item) and adds a `symbols` item with `path: None` after `repository_map` and before `retrieved_file` when an index and map are present, the ceiling is non-zero and a prompt identifier matches. A store or render failure, or no match, omits the item and leaves the turn alone. Exports `SymbolSource`, `RenderedSymbols`, `HEURISTIC_HEDGE`. `chat.rs`, `edit.rs` and `build_model_prompt` untouched; spec 32 Task 6 has not landed, so `build_context` is still the single body. **Tests:** `symbol_index` 57 run (2 skipped), including 6 new: one item between the map and retrieved files with the exact hedged line; a merged language-server symbol rendered without the hedge; no item at ceiling 0 or for a prompt naming nothing (each with a control, and the map's ceiling at 0 leaving symbols in); a small ceiling drops matches, stays within budget and ends with the shown/total line; an unwritable store omits the item while the turn keeps its prompt, map and retrieved files; prose words match exactly while only code-shaped words prefix-match. `permission_profiles` 71 run (1 skipped): 50 and 0 are applied, 100000 is refused as `RestrictOnly`, plus the weakening case and the rule label. `token_ceiling` 13 run: the exact call count did not move, because those prompts name no symbol, so `context.md` §9's fix was not needed. **Eval:** the deterministic tier passes 19/19. The token metric is 268,839 against `evals/baseline.json`'s 101,963, but that baseline is from 2026-09-18; with `symbol_context_max_tokens` forced to 0 the same tree gives 265,902, so this task's item accounts for +2,937 estimated tokens (+1.1%) and the rest predates the branch. The baseline was not rewritten. **Falsifications:** the hedge dropped → `a_prompt_naming_a_symbol_gets_one_hedged_symbols_item_after_the_map`; the item moved after `retrieved_file` → the same test; the ceiling ignored → `a_small_ceiling_drops_matches_and_says_how_many_were_shown`; the key merged as a preference (moved from `restrict_only_limit` to `preference`) → `a_repository_may_lower_the_symbol_context_ceiling_but_not_raise_it`, `an_imported_profile_once_selected_cannot_widen_anything`, `every_capability_key_resists_weakening_from_repository_scope` and `the_keys_a_profile_may_carry_agree_with_the_profile_scope_merge`; classifying it `Preference` in `classify_overlay_fields!` alone also fails `every_preference_key_applies_from_repository_scope` and `the_preference_keys_are_exactly_spec_34s_free_keys`. **Deviation:** the code and tests were written on 2026-10-17 on an older base and moved onto current `main` on 2026-10-23; two mechanical merge conflicts were resolved (an import line in `workspace_engine.rs`, and two Task 8 test blocks appended at the end of `tests/permission_profiles.rs`). `find_for_prompt` is the prompt-identifier lookup `context.md` §9 asks for, though the task bullet names only `render_for_model`. |
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

- [x] **Step 1: Write the accuracy fixtures first.** One file per language
  (`rust.rs`, `typescript.ts`, `javascript.js`, `python.py`, `go.go`,
  `java.java`, `kotlin.kt`, `php.php`). Each holds the declarations
  `extract_symbols` finds today, plus the known misses and false positives:
  a declaration inside a line comment, a block comment and a string or raw
  string; a multi-line string containing `fn x(`; for Rust, `impl Foo { fn a }`,
  `impl Bar for Foo { fn b }`, `pub(crate) fn`, `async fn`, `const fn`,
  `trait`, `type` and `const`; for JS/TS, a `const` local inside a function
  body. Each fixture has an expected list `(name, kind, line, container)`
  beside it as a Rust table in the test.
- [x] **Step 2: Record the "before" accuracy.** Run today's `extract_symbols`
  over each fixture. Record, per language, how many expected names it found,
  how many it found that are not expected, and the precision and recall. This
  is proposal §7's "before" number. Record it in the progress row now, because
  Step 4 destroys it.
- [x] **Step 3: Write the failing tests** in `tests/symbol_index.rs`: each
  fixture's expected list equals `extract_declarations`' output. Also, for
  every fixture, `extract_symbols` equals the deduplicated names of
  `extract_declarations`, which pins Task 1's "no caller changes" claim.
- [x] **Step 4: Implement** `extract_declarations`:
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
- [x] **Step 5: Record the "after" accuracy** in the same form as Step 2. If
  comment skipping and `impl` handling do not move Rust materially, say so
  (proposal §7).
- [x] **Step 6: Scoped checks.**
  `CARGO_TARGET_DIR="$PWD/target" cargo nextest run -p workspace-engine --test symbol_index --locked`,
  the `language.rs` unit tests, `cargo fmt --all -- --check`,
  `CARGO_TARGET_DIR="$PWD/target" cargo clippy -p workspace-engine --all-targets --locked -- -D warnings`
  and `typos`.
- [x] **Step 7: Falsify.** At least: comment skipping off (the comment
  fixture fails); `impl` tracking off (the method has no container);
  brace-depth check off for JS (the local appears); `extract_symbols` not
  deduplicated (the equality test fails).
- [x] **Step 8: Update the progress row, show the change, and ask before
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
