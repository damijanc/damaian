# Context: Symbol and Relationship Index

Background for [`tasks.md`](tasks.md). This file records where
[`proposal.md`](proposal.md) (the flat spec, unchanged in substance) no longer
matches the code, checked on 2026-10-09 against `main` after spec 24 closed,
and re-checked on 2026-10-12 after work from specs 32, 50, 56 and 57 landed
(new process kinds, a shutdown sweep, hook configuration, and drifted lines in
`chat.rs`, `config.rs`, `session.rs` and the shell). It also records the
decisions the proposal leaves open. Read it before Task 1. Do
not "fix" any of these back to the proposal's wording without re-reading the
code they cite.

The proposal was written before specs 24, 46 and 47 landed. Its descriptions of
`language.rs` and `FileRecord` still hold. Its line numbers for the indexer and
the MCP transport have drifted. Four of its claims about the surrounding system
are wrong, and each changes a task: §1 (the registry is built), §2 (MCP
framing), §3 (there is no session close) and §4 (where extraction runs).

## 1. The process registry is built, and a language server is one more kind

Proposal §2 says process tracking is "specified but not built" and points at
spec 17 §5.7. Spec 46 built it (Done 2026-09-17), and specs 32 and 57 have
since added kinds to it:

- `ProcessRegistry::register(kind, session_id, pid) -> RegistrationHandle`
  (`process_registry.rs:172`) records PID, start time and process group on
  disk before it returns, so a running child is never unrecorded.
- `ProcessKind` (`:64`) is `McpServer`, `ModelCall`, `Terminal`, `Command`,
  `Hook` (spec 32) and `Worker` (spec 57), with `as_str` at `:78`. It is
  matched exhaustively.
- The launch sweep (`sweep`, `:417`; `sweep_with`, `:286`) kills entries whose
  owning instance is gone. It refuses an entry whose start time no longer
  matches (`:333`) and terminates by process group only when the leader's
  identity matched (`terminate`, `:393`).
- The shutdown sweep, `sweep_own` (`:430`), kills this instance's own entries.
  `install_shutdown_handler` (`:478`) runs it on `SIGINT`, `SIGTERM` and
  `SIGHUP`, and the desktop shell installs it in `main`
  (`desktop-shell/src/main.rs:27`). A spec 57 worker runs its own `sweep_own`
  on exit (`desktop-shell/src/worker.rs:97`).
- Dropping the `RegistrationHandle` removes the entry (`:534`).

**Decision (Task 5):** add `ProcessKind::LanguageServer` (`"language_server"`).
A live server is stopped through its owned `Child` handle, the way
`StdioTransport`'s `Drop` does it (`mcp.rs:433-438`). An owned, unreaped child
cannot have had its PID recycled, so the start-time check matters only to the
sweep. The acceptance criterion "a recorded PID whose start time no longer
matches is not killed" is therefore proved by a sweep test over a
`LanguageServer` entry, alongside spec 46's own tests, not by new kill code.

## 2. The MCP stdio transport is the model for spawning, not for framing

Proposal §5.4 says the transport is "the same shape as the MCP stdio
transport (`mcp.rs:288-345`)". `StdioTransport` is now at `mcp.rs:302-438`.
Its spawn path is the right model: shell-split the configured command
(`:328-339`), `stderr(Stdio::null())`, `process_group(0)`, register before
touching the child (`:365`), and a reader thread feeding a channel so reads are
bounded by `recv_timeout` (`:374-387`).

Its framing is not. MCP stdio is newline-delimited JSON (`reader.lines()`,
`:377`; `writeln!`, `:397`). LSP frames every message with a
`Content-Length:` header and a blank line, and the body may contain newlines.

**Decision (Task 4):** a new `lsp.rs` module whose framing and JSON-RPC layer
run over generic `Read`/`Write`. It is unit-tested against in-memory buffers,
with no process. The process layer (Task 5) puts a child's pipes under it.
Nothing in `mcp.rs` changes.

**Test servers.** Observation 25 says freshly written executables stall on
this Mac, so a test cannot write a fake server script and run it. The fake
server is the test binary itself, re-executed into an `#[ignore]`d helper test
that speaks LSP on stdin and stdout. That is the pattern
`tests/process_registry.rs:69` already uses. Tests that need a real language
server are `#[ignore]` with a doc comment giving the command (`AGENTS.md`,
"Tests with real side effects").

## 3. There is no session close, so a server lives for the process or an idle timeout

Proposal §5.8 says a server is "shut down when the last session using that
repository closes", and §6 says "no language server process outlives the
session that started it". Checked:

- Sessions are never closed. `SessionStore::delete_session` (`session.rs:557`)
  deletes, and nothing else ends one. Spec 32 reached the same conclusion
  independently: its `session_end` hook fires on deletion, because "there is
  no session teardown" (`32_hooks/context.md` §3).
- MCP servers do not model this either. `build_mcp_runtime` (`chat.rs:539-566`)
  builds an `McpRuntime` per turn, so an MCP stdio server lives for one turn.
  That is wrong for a language server: rust-analyzer can take minutes to index
  a workspace, and restarting it every turn would mean it never answers.

**Decision (Task 5):** a process-wide `LanguageServerHub`, keyed by
`(repository_id, language)`, held in a `OnceLock` registry the way
`IndexCache` holds indexes (`index_cache.rs:28`). A server is started lazily,
on the first request for that language in that repository, and stopped when:

- it has had no request for `LANGUAGE_SERVER_IDLE_SECS` (600), checked on the
  next hub access and by the hub's own reaper thread;
- the Damaian process exits normally (the hub's `Drop` and an explicit
  `LanguageServerHub::shutdown_all`, called from the shell's exit path, which
  send `shutdown` and `exit` first);
- the process is signalled, in which case `sweep_own` kills every registered
  server by PID (§1);
- the process dies without either, in which case spec 46's launch sweep kills
  it on the next launch.

**Workers (spec 57).** A worker is a separate process, so it gets its own hub.
Its servers serve the worker's worktree, which is a different repository root
and so a different `repository_id`. They are never shared with the shell's.
They are registered with the worker as owner, so the worker's own
`sweep_own` (§1) kills them when it exits.

The session-scoped criteria are restated in these terms. "No language server
process outlives the session that started it" becomes "outlives neither the
Damaian process that started it nor its idle timeout, and is swept after a
crash". "Not restarted automatically more than once per session" becomes
"once per hub entry": a server that crashes twice stays down until the
repository's entry is dropped by idle expiry or restart, and the heuristic tier
covers it meanwhile. The `session_id` passed to `register` is the session that
caused the spawn, for the audit trail only.

**One server per repository, not per project root.** Spec 24's roots are
working directories, not separate projects as far as a language server is
concerned: rust-analyzer and `typescript-language-server` each discover
workspaces themselves. `rootUri` is the repository root.

## 4. Heuristic extraction already runs in the index pass, on redacted content

Proposal §2 cites `indexer.rs:357-358` for the extraction calls. They are at
`indexer.rs:288-289`, inside `add_file` (`:257`), and they read
`indexed_content`, the scanner-redacted text (`:284-286`). `FileRecord` is
`indexer.rs:22-33`, `symbols`/`imports` at `:29-30`. Its other readers are
`keyword_search` and `semantic_search` (`:61`, `:93-112`), `score_record`
(`:355-370`) and `vector_index.rs:112`. `RepositoryIndex` and `FileRecord`
derive no serde and live only in memory (spec 24 `context.md` §3).

Three consequences:

**4.1 Records, not names.** Task 1 adds
`extract_declarations(content, language) -> Vec<Declaration>`, where
`Declaration { name, kind, line, column, container }` is what the source text
alone can tell. `extract_symbols` stays, returning the declarations' names
(deduplicated, as today), so keyword scoring keeps its shape. Task 2 adds
`FileRecord.declarations`. `Symbol` (proposal §5.1) is assembled later, in
`SymbolIndex::build`, which adds `path`, `root_path` (from spec 24's
`RepositoryMap::root_for_path`, `repository_map.rs:525`) and `provenance`. A
`FileRecord` cannot carry `root_path`, for the reason spec 24 `context.md` §3
gives: one new manifest would have to re-stamp every record beneath it.

**4.2 Line numbers survive redaction only if extraction reads the original.**
`SecretScanner::redact` (`secret_scanner.rs:42-63`) replaces each secret's
whole span with a placeholder. A multi-line secret, such as a PEM block,
collapses to one line, and every declaration after it would report a wrong
line. **Decision (Task 2):** extract declarations from the original content.
Then pass every `name` and `container` through the scanner, and drop any
declaration in which it finds a secret. Nothing unscanned leaves the indexer:
the content itself is never stored, and only names and line numbers are kept.
A test puts a PEM block above a function and asserts the function's true line.

**4.3 Persistence pays off for language-server results, not re-extraction.**
Proposal §5.10 says persisting symbols means a launch "does not also pay for
re-extraction". Heuristic extraction is a few prefix matches per line inside a
pass that already reads, redacts, tokenises and chunks every file, so
re-extraction is not the cost. What persistence saves is language-server
results, which take a server round trip per file, and the reference cache. The
store keeps both tiers, keyed by content hash. The measurement that matters is
the language-server tier's warm launch against a cold one (Task 6, recorded in
proposal §7 by Task 11).

## 5. Persistence follows the repository map's store, not the vector index

The proposal names `vector_index.rs` (`vector_index_path`, `:149-153`;
`load`/`save`, `:22`/`:29`; no schema version) as the pattern. Since then,
spec 24 built the better one: `RepositoryMapStore` with
`MapLoad { Reused, Built, Rebuilt { reason } }` (`repository_map.rs:752`) and
a `repository_map_rebuilt` audit event (`:844`).

**Decision (Task 3):** `SymbolIndexStore` at
`<data_dir>/symbol-index/<repository_id>.json`, with `schema_version` and a
per-file `content_hash`, returning a `SymbolLoad` of the same shape and
auditing `symbol_index_rebuilt` with the reason. On load, a file whose hash
still matches keeps every stored entry, from either tier. A changed file is
re-extracted heuristically. Its language-server entries and cached references
are dropped until they are fetched again (§5.5's discard rule, applied to
definitions too).

## 6. `language_server.<language>` runs a program, so it belongs with `shell`

Proposal §5.4 adds `language_server.rust=rust-analyzer` without classifying
it. A value of that key is a program Damaian spawns, with the repository as
its working directory. That is the same power `shell` has, and spec 34 makes
`shell` Forbidden at repository scope (`scoped(shell, "shell", trusted,
forbidden, rejected)`, `config.rs:1170`;
`repository_config_cannot_redirect_the_shell`,
`tests/repository_config_trust.rs:144`).

**Decision (Task 5):** `language_servers: BTreeMap<String, String>` in
`Config`, set by `language_server.<language>=<command line>`.

- It is a **capability**. It is Forbidden at repository scope, so a cloned
  repository can never name a program for Damaian to run. It is not a profile
  key, as `mcp_servers` definitions are not (`config.rs:1419-1423`).
- **The alternative, not taken.** Spec 32 lets a repository *define* a hook,
  inert until the user reviews it (`hooks`/`hook_reviews_by_repository`,
  `config.rs:358-364`, classified at `:2731-2735`), as spec 34 did for MCP
  servers. A
  repository could then ship `language_server.typescript=./node_modules/.bin/…`.
  This spec does not copy that. A hook runs at a named event, and its review
  is shown at the moment it would run. A language server starts unprompted in
  the background on the first symbol lookup, so there is no natural moment to
  ask, and the user's own `user.conf` line costs one edit. If users ask for
  repository-supplied servers, the review-gated model is the way to add them
  later. It is a widening, so it needs a spec change, not a task.
- The language must be one that `detect_language` (`language.rs:3`) returns
  and `extract_declarations` handles. Anything else is a parse error, so a
  typo is reported rather than silently ignored.
- The value is shell-split like an MCP `command` (`mcp.rs:328-339`). A
  GUI-launched app may not inherit the user's `PATH`
  (`command_runner.rs:450`), so the user guide recommends an absolute path. A
  spawn failure means the server is unavailable: the language degrades to
  heuristics and the UI shows the error.
- Exhaustive handling: `classify_overlay_fields!` (`config.rs:2673`), both
  `to_policy_text`s, `ConfigOverlay::set`, `apply_overlay_scoped`'s
  destructure, `split_profile_keys`, and `rule_label`. In
  `tests/permission_profiles.rs` it joins the Forbidden table (`:1189`) and the
  rule-label table (`:2153`), and gets a weakening case. The
  repository-refusal test goes in this spec's own test file. Spec 34's
  `tests/repository_config_trust.rs` is not edited.

**What the server is sent.** Damaian sends `didOpen` with the file's original
content to a local process the user configured. That process reads the same
repository from disk anyway. It is the user's own editor tooling, not a model
or a network endpoint, so it is not the boundary the secret scanner guards.
Everything coming *back* is scanned before it is stored or shown: symbol names
as in §4.2, and diagnostic text through `Finding::new` (`finding.rs:151`),
which redacts by construction.

## 7. Restricted files are indexed, so every symbol output filters them

`ProjectIndexer` and `tree_walk` do not consult `restricted_patterns`, which
appear only in `PathPolicy` (`path_policy.rs:138`). The defaults include
`.env`, `*.pem` and `*.key` (`config.rs:31-43`), and none of those is in
`DEFAULT_IGNORE_PATTERNS` (`config.rs:15-29`). So a restricted file that is not
git-ignored is in the index, and its declarations would be too. Context files
are protected, because `ContextManager::add_file` goes through
`FileAccessController`.

**Decision (Task 2):** `SymbolIndex::build` takes the `PathPolicy` and
leaves out every file it reports restricted. That covers the store, the
lookup, the context item and the shell together. The eager pass (Task 6) never
sends a restricted file to a language server, and the diagnostics path (Task
7) drops any diagnostic on one.

Not verified here: whether `search_codebase` already returns snippets of such
files to the model. Its path in `chat.rs` (`:3756-3786`) shows no policy
check. That is outside this spec and is recorded as an observation, not fixed.

## 8. Eager definitions are bounded and never awaited by a turn

Proposal §5.5 indexes definitions eagerly, one `documentSymbol` per file. Two
facts the proposal does not account for:

- Many servers answer `documentSymbol` only for an open document
  (`typescript-language-server` does). Each request is therefore
  `didOpen` → `documentSymbol` → `didClose`.
- A freshly started server can take minutes to answer at all, while it builds
  its own index.

**Decision (Task 6):** the eager pass runs on the hub's thread, never on a
request thread. It walks the configured languages' files in index order, under
`LANGUAGE_SERVER_REQUEST_TIMEOUT` (5 s) per request, and merges results into
the stored index as they arrive. Every reader (lookup, context item, shell)
reads the current snapshot and never waits on a server. A file not reached yet
is heuristic. That is requirement 5's "never blocks a task", stated as
architecture. Proposal §7's latency measurement decides whether the pass is
worth its cost on a medium repository.

**Merging (proposal §5.6).** A language-server symbol supersedes a heuristic
one with the same `name`, `path` and a `line` within `MERGE_LINE_TOLERANCE`
(3). The language-server `line` is the start of its `selectionRange` (the
name), not of `range`, because `range` starts at a doc comment or attribute
that the heuristic never sees. LSP lines and columns are 0-based. `Symbol`
stores 1-based, as `SourceRange` does (`finding.rs:101-108`).

**Kinds.** LSP `SymbolKind`s map to proposal §5.1's enum: `Function`,
`Method` (and `Constructor`), `Struct`, `Enum`, `Class`, `Interface`, `Type`
(`TypeParameter` excluded), `Constant`, `Variable`, `Module` (and
`Namespace`, `Package`). Rust traits arrive as `Interface` from rust-analyzer
and are kept as `Interface`, because LSP has no `Trait`; the heuristic tier
records `Trait`, and the merge treats the two as compatible. `Field`,
`Property`, `EnumMember`, `Key` and the remaining kinds are not indexed. They
multiply the index without serving "where is X defined".

## 9. Search integration and the model, without `chat.rs`

Requirement 7 integrates symbols with search. Checked:

- The model's `search_codebase` tool is defined, parsed and executed in
  `chat.rs` (definition `:4571`, parsing `:4780-4806`, execution
  `:3756-3786`). Its output, `format_search_results` (`chat.rs:5215`), prints
  path, score and snippet, and ignores `SearchResult.symbols`.
- Every model tool is defined in `chat.rs` (`:4501-4610`).
- `ContextManager::build_context` (`context_manager.rs:81`) runs the keyword
  and semantic search over the prompt for both the chat and edit flows. Spec
  24 added the `repository_map` item there (after `project_rule`, before
  `retrieved_file`) without touching `chat.rs`.

**Decision (Task 8):** the model sees symbols as one more `ContextItem`, of
kind `symbols`, added in `build_context` after `repository_map` and before
`retrieved_file`. Its content is the lookup (proposal §5.11) run over the
prompt's identifiers: exact, then prefix, ordered by provenance, then match
quality. It is rendered one per line, for example
`upload_retry (function, src/upload.rs:42, heuristic — may be inaccurate)`.
A language-server item carries no hedge. It is bounded by
`symbol_context_max_tokens`, a lower-wins capability key with the same merge
as `repository_map_max_tokens` (spec 24 `context.md` §7). Its default is
**1,600**, which is spec 26's planned 10% share
(`context_budget.symbols=0.10`, `26_context_assembly.md:186`) of the
16,000-token default budget. `0` turns it off. Spec 26 absorbs the item as its
planned `ContextCategory::Symbols`.

The `search_codebase` output is unchanged. Search integration otherwise lives
in the engine (`WorkspaceEngine::find_symbols`), the CLI and the shell.

Spec 24's Task 9 found that a new per-request item shifts
`tests/token_ceiling.rs`'s exact call count (spec 24 proposal §7.3). Task 8
runs that file. If the count moves, it sets `symbol_context_max_tokens: 0` in
`engine_with_ceiling`, as spec 24 did for the map.

**Spec 32 edits the same function.** Its Task 6 (not started on 2026-10-12)
adds `build_context_with_hooks`, which takes `hook_context` items, and makes
`build_context` delegate to it (`32_hooks/tasks.md`, Task 6). Whichever lands
second rebases. The `symbols` item belongs in the shared body, so both entry
points get it. Its place stays after `repository_map` and before
`retrieved_file`, wherever `hook_context` ends up.

## 10. Diagnostics: recorded per session, with no task, and unable to block a step

Proposal §5.9 attributes a diagnostic "to the task active when it arrives".
Checked:

- Findings are session events: `SessionStore::record_finding(session_id,
  &Finding)` (`session.rs:1688`). The findings panel reads them through
  `GET /api/findings?repo=&session_id=` (`handle_findings`,
  `desktop-shell/src/lib.rs:3775`).
- `FindingSource::LanguageServer` exists (`finding.rs:38`, `"language_server"`
  at `:59`) and nothing produces it.
- A plan step is blocked only by `Evidence::Findings` (`plan.rs:95`), which the
  turn loop attaches for a check it ran. A finding recorded outside the loop is
  linked to no step and cannot block one.
- Finding ids are random (`create_id("finding")`, `finding.rs:158`), so
  recording the same diagnostic twice makes two findings.

**Decision (Task 7):** the hub keeps the latest `publishDiagnostics` per file,
since each notification replaces that file's set. An engine function,
`record_language_server_findings(session_id, repository_root)`, records the
current ones the session does not hold yet. They are deduplicated on
`(path, range, code, summary, file_hash)` and recorded with no task id. The
shell calls it in `handle_findings` before listing. The range path is the
URI made repository-relative, refusing anything outside the repository or
restricted. `file_hash` is the file's current content hash, so spec 22's
staleness rule applies unchanged. Severity 1 maps to `Error`, 2 to `Warning`,
and 3 and 4 to `Info` (`finding.rs:70-74`).

The active-task attribution needs the turn loop. That is `chat.rs`, so it is
in §12.

## 11. Imports, `TestOf` and what resolves

`extract_imports` (`language.rs:69-115`) returns raw specifiers. For Python it
keeps leading dots (`take_identifier` allows `.`, `:135-142`). For Rust it
keeps the whole `use` tail, braces included.

**Decision (Task 2):** proposal §5.2's resolution rule, applied as written:

- **JavaScript/TypeScript** `./` and `../` specifiers resolve against the
  importing file's directory. Each tries the bare path, then the extensions
  `.ts`, `.tsx`, `.js`, `.jsx`, `.mjs`, `.cjs`, then `index` with each
  extension. The first indexed file wins.
- **Python** relative imports (`.upload`, `..pkg.mod`) resolve to `upload.py`
  or `upload/__init__.py` relative to the package directory.
- **Everything else stays `External`**: bare package names, `tsconfig`
  aliases, Go import paths, Java/Kotlin packages, and every Rust `use`
  (`crate::`, `super::` and `self::` included). Resolving Rust module paths
  means modelling `mod` declarations and `#[path]`, which is a parser's job
  (proposal §4).

`TestOf` is always `Heuristic`. It pairs a test file with a subject in the same
spec 24 root, using the root's `test_paths` (`repository_map.rs:254`) and
these conventions: Rust `tests/<name>.rs` ↔ `src/<name>.rs`; JS/TS
`<name>.test.<ext>` and `<name>.spec.<ext>` ↔ `<name>.<ext>`; Python
`test_<name>.py` ↔ `<name>.py`; Go `<name>_test.go` ↔ `<name>.go`.

## 12. What needs `chat.rs`, and is not in this spec

#25 runs in parallel with a `chat.rs` spec and must not edit `chat.rs` or
`build_model_prompt` (`tasks.md`, Global Constraints). Three things in the
proposal can be done only there. None is in any task. Each is named so that
the close-out reports it rather than leaving it quietly absent.

1. **An agent-facing symbol tool.** Proposal §5.5 resolves references "when
   the user or the agent asks". The agent can ask only through a tool, and
   tools are defined and dispatched in `chat.rs` (§9). Without one, the agent
   sees definitions through the `symbols` context item, but cannot request
   references or implementations. The user can, through the shell and the
   CLI. Requirement 2's references and implementations are therefore met for
   the user and **not met for the agent**.
2. **Diagnostics attributed to the active task.** The active task id lives in
   the turn loop. Diagnostics are recorded with no task id instead (§10). The
   proposal allows that case for when no task is active, so the findings are
   still shown; they are just never tied to a task.
3. **Symbols in `search_codebase`'s result text** (`format_search_results`,
   `chat.rs:5215`). Not needed for any acceptance criterion: the criterion
   "searching for a symbol returns its definition with an exact file and line"
   is met by `find_symbols`.

A follow-up that adds items 1–3 is one `chat.rs` task. It should be sequenced
against whichever `chat.rs` spec is in flight. Task 11 records it as
proposed follow-up work. It is not started here.

## 13. Language servers on this machine

Checked on 2026-10-09: `~/.cargo/bin/rust-analyzer` is a rustup proxy, and
the component is not installed ("Unknown binary 'rust-analyzer' in official
toolchain"). `typescript-language-server`, `pyright-langserver` and `gopls`
are not on `PATH`. Proposal §7 asks for measured `documentSymbol` latency on a
medium repository, which needs at least one real server. Installing one
(`rustup component add rust-analyzer` is the cheapest, and this repository is
the medium repository) is the user's decision. Task 6 asks before measuring,
and records "not measured" if the answer is no.

## 14. Things the proposal says that were checked and hold

- `extract_symbols` (`language.rs:29`) iterates `content.lines()` (`:31`)
  and matches prefixes per language (`:35-61`). It discards the line and the
  kind. For Rust it strips one `pub ` and matches `fn `, `struct ` and
  `enum ` only: an indented `fn` inside an `impl` is recorded as a bare name
  with no container. `pub(crate) fn`, `async fn`, `const fn`, `trait`, `type`
  and `const` are missed. The JS/TS `const`/`let` matches also record locals
  inside function bodies, which the accuracy fixtures should count.
- `detect_language` (`language.rs:3`) maps an extension to a language name.
- `keyword_search` (`indexer.rs:61`) scores over `FileRecord.terms`, which
  include the symbol names (`:360-361`).
- There is no LSP client, no symbol lookup by name, and no producer of
  `FindingSource::LanguageServer`.
- The keyword index is in memory only and rebuilt per launch
  (`index_cache.rs`, spec 24 `context.md` §3).
