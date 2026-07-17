# Feature Spec: Real Semantic Search (Local Embeddings + Vector Index)

Status: Done
Order: 2 of 5
Related spec sections: `ai_coding_assistant_specification.md` §7.2 (Project Indexer), §18 (Open Questions — "Which semantic search backend..."), §19 (Recommended Technology Direction — "A local vector index for semantic search").

## 1. Motivation

The specification's Project Indexer requirements (§7.2) and acceptance criteria explicitly require that "semantic search and keyword search can both return relevant files for a coding question," implying two genuinely different retrieval strategies. §19 recommends a local vector index for this. §18 leaves the embedding backend as an open question — meaning it was deferred, not decided against.

Today, `RepositoryIndex::semantic_search` (`crates/workspace-engine/src/indexer.rs:92-127`) is **not semantic at all**: it tokenizes the query and each chunk's text (`path + symbols + chunk.text`) and counts overlapping terms — the same family of algorithm as `keyword_search` (`indexer.rs:61-90`), just scored per-chunk instead of per-file. There is no embedding model, no vector storage, and no similarity metric anywhere in the codebase. This means a query like "where do we validate the refresh token" will not surface a function named `checkRotatedCredential` even if it's semantically the right match — it can only find literal term overlaps.

This is the most significant gap between the spec and the implementation, and it directly affects answer quality for the Context Manager (§7.4), which relies on both search modes to select relevant files.

## 2. Goals

- Generate and store local vector embeddings for each indexed text chunk (`TextChunk`, `indexer.rs:12-19`), reusing the existing chunk boundaries so no re-chunking logic is needed.
- Provide true nearest-neighbor semantic search over those embeddings, replacing the term-overlap implementation in `semantic_search`.
- Keep embedding generation fully local — consistent with the local-first architecture principle (§17.1) and the "no data leaves the machine except model calls" posture. No embeddings API calls to a remote provider by default.
- Recompute embeddings only for changed chunks on incremental refresh (mirrors the existing "recompute only changed chunks when possible" requirement, §7.2), using `TextChunk.text_hash` (`indexer.rs:18`) to detect unchanged chunks and skip re-embedding.
- Preserve the existing `SearchResult` shape (`indexer.rs:41-49`) and the `keyword_search`/`semantic_search` dual-method API on `RepositoryIndex` so callers (context manager, chat orchestrator) don't need to change their call sites, only get better results.

## 3. Non-Goals

- Replacing keyword search — both retrieval modes remain, per spec (§7.2 acceptance criteria explicitly asks for both).
- Hybrid re-ranking / reciprocal-rank-fusion of keyword + semantic results — worth a follow-up once both are proven independently, not required for this spec.
- Remote/cloud embedding APIs — out of scope; if ever added, it must go through the same provider-isolation principle as the model adapter (§17.3) and be opt-in.
- Cross-repository semantic search (spec's `SearchResult`/`RepositoryIndex` are already single-repository scoped; keep that boundary).

## 4. Design

### 4.1 Embedding model

Use a small local embedding model that runs on-device without a network call:
- Preferred: a compact sentence-embedding ONNX model (e.g. an `all-MiniLM`-class model, ~80MB, 384-dim vectors) run via the `ort` (ONNX Runtime) Rust crate, or `candle` if the team prefers a pure-Rust inference stack with no native ONNX Runtime binary dependency.
- The model file ships as a bundled resource (similar to how `desktop-app/icons` bundles static assets) or is downloaded once on first use into the client's data directory (consistent with "cache and index data are stored in a client-owned directory and can be cleared," §6.2).
- Embedding generation must run off the UI thread / async, since it will be the slowest step of indexing a large repository.

### 4.2 Vector storage

Add a lightweight local vector index alongside the existing `RepositoryIndex`:
- Store one embedding vector per `TextChunk`, keyed by `(file path, chunk.ordinal, chunk.text_hash)`.
- For MVP repository sizes (spec targets a single developer's local repos, not enterprise monorepos with millions of files), an in-memory flat vector store with brute-force cosine similarity is sufficient — no need for HNSW/IVF indexing initially. Re-evaluate if repository sizes in practice make brute-force scanning too slow (profile before optimizing).
- Persist vectors to disk under the client's existing index-cache location (see `index_cache.rs`) so embeddings survive app restart and don't need full recomputation — mirrors "index updates should be resumable after app restart" (§12.2).
- Storage format: a simple binary format (e.g. `bincode`) mapping chunk keys to `Vec<f32>`, versioned by embedding-model identifier so a model upgrade invalidates stale vectors cleanly.

### 4.3 API surface

- Add `RepositoryIndex::semantic_search` replacement (or a new `VectorIndex` type composed alongside `RepositoryIndex`) that: embeds the query string using the same local model, computes cosine similarity against all stored chunk vectors, and returns the top-`limit` results in the existing `SearchResult` shape.
- Keep `keyword_search` untouched.
- Incremental refresh: when `index_cache.rs`'s file watcher detects a changed file, only re-embed chunks whose `text_hash` differs from the stored value; drop vectors for removed files/chunks.

### 4.4 Fallback behavior

- If the embedding model fails to load (missing file, unsupported hardware, disabled by config), semantic search should degrade to the current term-overlap behavior rather than erroring, so existing behavior is a safety net, not a regression risk. Surface this degraded state via a config/log entry, not a user-facing error, since it doesn't block core functionality (§11 error handling posture: "continue with partial index when safe").

## 5. Data Model Changes

No changes to `FileRecord` or `TextChunk` structs are required — embeddings are stored in a separate index keyed by existing chunk identity fields, keeping the change additive and low-risk to existing indexing code.

## 6. Acceptance Criteria

- A query using different wording than the code (e.g. "how do we retry failed API calls" against code that says `backoff`/`retry_attempt` without the word "retry" appearing in the query, or vice versa with synonyms) returns relevant chunks that term-overlap search would miss or rank low.
- Editing a file triggers re-embedding of only the changed chunks, not the whole repository (verified by timing/instrumentation, consistent with §7.2's incremental-refresh acceptance criterion).
- Restarting the app after indexing a repository does not require full re-embedding — vectors load from the persisted store.
- If the embedding model is unavailable, semantic search still returns results (via fallback), and the app does not crash or block indexing.
- No embedding computation makes a network call by default.

## 7. Open Questions / Decisions Needed

- `ort` (ONNX Runtime bindings, requires a native runtime library) vs `candle` (pure Rust, no native dependency, slightly less mature for this exact use case) — this affects the desktop app's binary size and macOS code-signing/notarization story for the bundled `.dmg`. Recommendation: evaluate `candle` first for a pure-Rust dependency story consistent with the rest of the workspace-engine.
- Exact embedding model choice and its license/size trade-off — needs a decision before implementation starts, since it affects first-run download size or bundled app size.
- Whether embeddings should be computed synchronously during initial indexing (blocking "ready" state) or lazily/in the background with keyword search available immediately — recommend background, matching "initial index for a medium repository should complete in the background without blocking chat UI" (§12.1).

## 8. Implementation Notes (as built)

- Went with `candle` (per explicit direction: "use rust native dependency"), not `ort`. Model: `sentence-transformers/all-MiniLM-L6-v2` (384-dim, ~90MB), loaded via `candle-transformers`' `BertModel`.
- **Rejected `hf-hub` for model download**: it pulls in a large async stack (`reqwest`, `quinn`/QUIC, `tokio`, a Xet storage client, `redb`, ~100 extra crates) even with its "blocking" feature enabled, which directly contradicts this project's established avoidance of `reqwest` (the model-provider transport is deliberately curl-based). Instead, `embeddings.rs` shells out to `curl` for the one-time download of `config.json`/`tokenizer.json`/`model.safetensors` from `https://huggingface.co/sentence-transformers/all-MiniLM-L6-v2/resolve/main/<file>`, into `<data_dir>/models/all-MiniLM-L6-v2/`, mirroring the existing `Command::new("curl")` pattern in `model.rs`'s `CurlModelTransport`. Downloads land in a `.part` file and are renamed atomically so an interrupted download can't be mistaken for a complete one on the next run.
- **Known trade-off**: `candle-core` v0.11.0 unconditionally depends on `tokenizers` with its `onig` feature (a native C oniguruma library, compiled at build time via a build script — not a shared library that needs bundling/signing separately). This is unrelated to my own `tokenizers` dependency choice (added `--no-default-features --features fancy-regex` for the copy I use directly) — `onig` comes in regardless, forced by candle itself. Still far lighter than the alternative (bundling a platform-specific ONNX Runtime shared library for `ort`), and only requires a C compiler at build time (already present on any machine that can build this Tauri app).
- **Config-gated, opt-in, default off**: `Config.enable_semantic_search: bool` (default `false`) — enabling it is what triggers the one-time model download, and the spec's own local-first/no-implicit-network principle (main spec §4.2) argues against silently starting a network fetch. `damaian config-set repo <repo> enable_semantic_search true` (or `user`/`admin` scope) turns it on. `ContextManager` now takes `data_dir`/`enable_semantic_search`, and only calls `VectorIndexCache::semantic_search` when enabled — otherwise it calls the original term-overlap `RepositoryIndex::semantic_search`, so default behavior is completely unchanged.
- `EmbeddingModel` is a process-wide, lazily-initialized singleton (`OnceLock`) — a load failure (no network on first use, etc.) is cached as `None` for the process lifetime so semantic search degrades to term-overlap once per process rather than re-attempting (and re-failing) on every request, per the spec's fallback requirement.
- `VectorIndexCache` (`vector_index.rs`) mirrors `IndexCache`'s process-wide-registry pattern: one persisted, bincode-serialized vector store per repository under `<data_dir>/vector-index/<repository_id>.bin`, keyed by `(path, chunk ordinal)`, re-embedding only chunks whose `text_hash` changed and dropping entries for removed files/chunks, matching the incremental-refresh requirement in §7.2.
- Verified with two new integration tests in `crates/workspace-engine/tests/semantic_search.rs`, both requiring real network access on first run (cached afterward under a stable OS temp dir shared across test runs, not the per-test throwaway dir):
  1. `embedding_model_scores_semantically_similar_text_higher_than_unrelated_text` — proves genuine semantic understanding (two zero-word-overlap sentences score meaningfully higher than an unrelated pair).
  2. `vector_index_cache_finds_semantically_related_code_that_keyword_search_misses` — indexes a small fixture repo, confirms `VectorIndexCache::semantic_search` ranks the semantically relevant file first for a query sharing no real keywords with it (and that `keyword_search`'s top hit is a different file, so the test is actually meaningful — `keyword_search`'s substring-without-stopword-filtering scoring turned out to be far leakier than expected, matching almost anything on short tokens like "a"), plus that an unchanged repeat search doesn't rewrite the persisted vector file.
  3. Also manually verified end-to-end through the actual CLI (`damaian ask`, mock model): with `enable_semantic_search=true` set via `config-set repo`, a natural-language query sharing no keywords with the fixture code correctly pulled in the right file as context (`context_files=src/resilience.rs`).
- Full workspace build/tests: 89 tests pass (was 87 before this feature), zero regressions, default config behavior unchanged.
