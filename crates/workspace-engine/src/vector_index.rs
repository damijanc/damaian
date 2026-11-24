use crate::embeddings::{EmbeddingModel, shared_model};
use crate::error::Result;
use crate::indexer::{RepositoryIndex, SearchResult, TextChunk};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

/// Chunks per call to the embedding model. The model's working memory grows
/// with the batch (each 128-token chunk carries its attention scores and
/// activations), so this, not the repository, sets the peak.
const SYNC_BATCH_CHUNKS: usize = 32;

/// Batches between saves of a sync still in progress.
const SAVE_EVERY_BATCHES: usize = 8;

/// How long one search may spend embedding before it answers. A search that
/// runs out answers from term-overlap search, and the next one carries on.
const SYNC_BUDGET: Duration = Duration::from_secs(2);

/// Repositories whose vectors stay loaded; the least recently searched one is
/// dropped from memory past this (its file stays on disk).
const CACHED_REPOSITORIES: usize = 4;

/// How long the background fill holds a repository's vectors at a time. It
/// lets go between slices so a search for that repository is never kept
/// waiting for the whole fill.
const BACKGROUND_SLICE: Duration = Duration::from_millis(500);

/// `semantic_search_max_chunks` as a count this machine can index with.
pub fn semantic_search_max_chunks(config: &crate::config::Config) -> usize {
    usize::try_from(config.semantic_search_max_chunks).unwrap_or(usize::MAX)
}

/// Turns chunk texts into vectors. [`EmbeddingModel`] in the app; tests
/// substitute one that records what it was asked.
pub trait ChunkEmbedder: Sync {
    fn embed_batch(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>>;
}

impl ChunkEmbedder for EmbeddingModel {
    fn embed_batch(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>> {
        EmbeddingModel::embed_batch(self, texts)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SyncState {
    /// Every chunk has a current vector.
    Complete,
    /// Chunks are still pending because the budget ran out.
    Partial,
    /// The model failed on a batch; the chunks before it were kept.
    Failed,
    /// The repository has more chunks than `semantic_search_max_chunks`.
    OverCap { total_chunks: usize },
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct StoredChunkVector {
    path: String,
    ordinal: usize,
    text_hash: String,
    vector: Vec<f32>,
}

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
struct VectorIndex {
    entries: Vec<StoredChunkVector>,
}

impl VectorIndex {
    fn load(path: &Path) -> Self {
        std::fs::read(path)
            .ok()
            .and_then(|bytes| bincode::deserialize(&bytes).ok())
            .unwrap_or_default()
    }

    fn save(&self, path: &Path) {
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Ok(bytes) = bincode::serialize(self) {
            let _ = std::fs::write(path, bytes);
        }
    }

    /// Brings the stored vectors up to date with `index`, a batch at a time.
    ///
    /// Only chunks whose stored hash no longer matches are embedded, and
    /// entries for files or chunks that no longer exist are dropped. Each call
    /// to the model gets at most [`SYNC_BATCH_CHUNKS`] chunks, so peak memory is
    /// set by the batch rather than the repository: one call for every chunk
    /// of a large repository is what once took all of a Mac's memory. The call
    /// returns once `budget` has passed with chunks still pending, so a large
    /// first sync spreads over several searches instead of stalling one, and
    /// `save` is offered the progress every [`SAVE_EVERY_BATCHES`] batches so an
    /// interrupted sync resumes where it stopped.
    fn sync(
        &mut self,
        index: &RepositoryIndex,
        embedder: &dyn ChunkEmbedder,
        max_chunks: usize,
        budget: Duration,
        save: &mut dyn FnMut(&VectorIndex),
    ) -> SyncState {
        let total_chunks: usize = index.files.iter().map(|file| file.chunks.len()).sum();
        if total_chunks > max_chunks {
            // Over the cap nothing is embedded or held: vectors for part of a
            // repository would rank only that part, and holding them is the
            // memory the cap exists to bound.
            let had_entries = !self.entries.is_empty();
            self.entries = Vec::new();
            if had_entries {
                save(self);
            }
            return SyncState::OverCap { total_chunks };
        }

        let mut existing: HashMap<(String, usize), StoredChunkVector> = self
            .entries
            .drain(..)
            .map(|entry| ((entry.path.clone(), entry.ordinal), entry))
            .collect();
        let matched_count = existing.len();

        // Borrowed from `index`, not copied: a first sync would otherwise hold
        // a second copy of the repository's text for its whole duration.
        let mut pending: Vec<(&str, &TextChunk)> = Vec::new();
        for file in &index.files {
            for chunk in &file.chunks {
                match existing.remove(&(file.path.clone(), chunk.ordinal)) {
                    Some(entry) if entry.text_hash == chunk.text_hash => self.entries.push(entry),
                    _ => pending.push((file.path.as_str(), chunk)),
                }
            }
        }
        let removed = !existing.is_empty() || matched_count != self.entries.len();
        if pending.is_empty() {
            if removed {
                save(self);
            }
            return SyncState::Complete;
        }

        let started = Instant::now();
        let mut batches = pending.chunks(SYNC_BATCH_CHUNKS).enumerate().peekable();
        while let Some((ordinal, batch)) = batches.next() {
            let texts: Vec<&str> = batch.iter().map(|(_, chunk)| chunk.text.as_str()).collect();
            let Ok(vectors) = embedder.embed_batch(&texts) else {
                save(self);
                return SyncState::Failed;
            };
            for ((path, chunk), vector) in batch.iter().zip(vectors) {
                self.entries.push(StoredChunkVector {
                    path: (*path).to_string(),
                    ordinal: chunk.ordinal,
                    text_hash: chunk.text_hash.clone(),
                    vector,
                });
            }
            if batches.peek().is_none() {
                break;
            }
            if started.elapsed() >= budget {
                save(self);
                return SyncState::Partial;
            }
            if (ordinal + 1) % SAVE_EVERY_BATCHES == 0 {
                save(self);
            }
        }
        save(self);
        SyncState::Complete
    }

    /// `demote_tests` halves a test file's similarity before ranking, as
    /// keyword search halves its score (`indexer::demoted`).
    fn search(
        &self,
        query_vector: &[f32],
        index: &RepositoryIndex,
        limit: usize,
        allowed: &dyn Fn(&str) -> bool,
        demote_tests: bool,
    ) -> Vec<SearchResult> {
        let mut scored: Vec<(f32, &StoredChunkVector)> = self
            .entries
            .iter()
            .filter(|entry| allowed(&entry.path))
            .map(|entry| {
                let similarity = cosine_similarity(query_vector, &entry.vector);
                if demote_tests && crate::indexer::is_test_path(&entry.path) {
                    (similarity * 0.5, entry)
                } else {
                    (similarity, entry)
                }
            })
            .collect();
        scored.sort_by(|left, right| {
            right
                .0
                .partial_cmp(&left.0)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        scored
            .into_iter()
            .take(limit)
            .filter_map(|(score, entry)| {
                let file = index.files.iter().find(|file| file.path == entry.path)?;
                let chunk = file
                    .chunks
                    .iter()
                    .find(|chunk| chunk.ordinal == entry.ordinal)?;
                Some(SearchResult {
                    path: file.path.clone(),
                    language: file.language.clone(),
                    symbols: file.symbols.clone(),
                    imports: file.imports.clone(),
                    // Cosine similarity for these (L2-normalized) embeddings
                    // is in roughly [0, 1] for related text; scale to an
                    // integer so it sorts consistently with keyword_search's
                    // count-based score in combined result lists.
                    score: (score.max(0.0) * 1000.0) as usize,
                    snippet: chunk.text.chars().take(500).collect(),
                    lines: Some(chunk.lines),
                })
            })
            .collect()
    }
}

fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    let dot: f32 = a.iter().zip(b).map(|(x, y)| x * y).sum();
    let norm_a: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let norm_b: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm_a == 0.0 || norm_b == 0.0 {
        0.0
    } else {
        dot / (norm_a * norm_b)
    }
}

struct CachedVectorIndex {
    index: VectorIndex,
    path: PathBuf,
    /// Reported once per process, so a repository over the cap is not
    /// announced on every search.
    over_cap_reported: bool,
}

type Registry = Mutex<HashMap<String, (Arc<Mutex<CachedVectorIndex>>, Instant)>>;
static REGISTRY: OnceLock<Registry> = OnceLock::new();

fn registry() -> &'static Registry {
    REGISTRY.get_or_init(|| Mutex::new(HashMap::new()))
}

fn vector_index_path(data_dir: &Path, repository_id: &str) -> PathBuf {
    data_dir
        .join("vector-index")
        .join(format!("{repository_id}.bin"))
}

/// The cached vectors for `repository_id`, loading them on first use and
/// dropping the least recently used repository past [`CACHED_REPOSITORIES`].
fn cached_index(data_dir: &Path, repository_id: &str) -> Arc<Mutex<CachedVectorIndex>> {
    let mut registry = registry()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let now = Instant::now();
    let entry = registry
        .entry(repository_id.to_string())
        .or_insert_with(|| {
            let path = vector_index_path(data_dir, repository_id);
            (
                Arc::new(Mutex::new(CachedVectorIndex {
                    index: VectorIndex::load(&path),
                    path,
                    over_cap_reported: false,
                })),
                now,
            )
        });
    entry.1 = now;
    let cached = entry.0.clone();
    while registry.len() > CACHED_REPOSITORIES {
        let Some(oldest) = registry
            .iter()
            .filter(|(id, _)| id.as_str() != repository_id)
            .min_by_key(|(_, (_, used))| *used)
            .map(|(id, _)| id.clone())
        else {
            break;
        };
        registry.remove(&oldest);
    }
    cached
}

/// Repositories whose vectors a background thread is filling in. One at most
/// per repository: two would embed the same chunks and race on one file.
static FILLING: OnceLock<Mutex<std::collections::HashSet<String>>> = OnceLock::new();

fn filling() -> &'static Mutex<std::collections::HashSet<String>> {
    FILLING.get_or_init(|| Mutex::new(std::collections::HashSet::new()))
}

fn is_filling(repository_id: &str) -> bool {
    filling()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .contains(repository_id)
}

/// Removes the repository from [`FILLING`] however the fill ends, a panic
/// included, so a later search can start another.
struct FillingGuard(String);

impl Drop for FillingGuard {
    fn drop(&mut self) {
        filling()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .remove(&self.0);
    }
}

/// Finishes `index`'s vectors on a background thread, a slice at a time, so a
/// large repository's first index completes in the time embedding takes
/// rather than over dozens of searches each spending [`SYNC_BUDGET`].
///
/// Memory stays what one search's sync uses: the same batches, and the same
/// saves. The thread owns a copy of the repository index for as long as it
/// runs; that is the price of not holding the caller's. Returns `None` when a
/// fill for this repository is already running.
fn fill_in_background(
    entry: Arc<Mutex<CachedVectorIndex>>,
    index: RepositoryIndex,
    embedder: &'static dyn ChunkEmbedder,
    max_chunks: usize,
) -> Option<std::thread::JoinHandle<()>> {
    let repository_id = index.repository_id.clone();
    if !filling()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .insert(repository_id.clone())
    {
        return None;
    }
    let guard = FillingGuard(repository_id);
    std::thread::Builder::new()
        .name("vector-index-fill".to_string())
        .spawn(move || {
            let _guard = guard;
            loop {
                let mut cached = entry
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                let CachedVectorIndex {
                    index: vectors,
                    path,
                    ..
                } = &mut *cached;
                let mut save = |vectors: &VectorIndex| vectors.save(path);
                let state = vectors.sync(&index, embedder, max_chunks, BACKGROUND_SLICE, &mut save);
                drop(cached);
                if state != SyncState::Partial {
                    break;
                }
            }
        })
        .ok()
}

/// Real, embedding-based semantic search, backed by a process-wide cache of
/// per-repository vectors (mirroring `IndexCache`'s pattern for
/// `RepositoryIndex`). Falls back to `RepositoryIndex::semantic_search`'s
/// term-overlap search whenever embeddings cannot answer: no local model
/// (first-run download failed, or embedding the query failed), a repository
/// over `max_chunks`, or vectors still being built. Callers always get a
/// result either way.
pub struct VectorIndexCache;

impl VectorIndexCache {
    pub fn semantic_search(
        data_dir: &Path,
        index: &RepositoryIndex,
        query: &str,
        limit: usize,
        max_chunks: usize,
    ) -> Vec<SearchResult> {
        Self::semantic_search_allowed(data_dir, index, query, limit, max_chunks, &|_| true)
    }

    /// [`Self::semantic_search`] over files `allowed` admits.
    ///
    /// The filter is applied before the cap in both the embedding path and its
    /// term-overlap fallback, so outside hits cannot consume the caller's
    /// result slots.
    pub fn semantic_search_allowed(
        data_dir: &Path,
        index: &RepositoryIndex,
        query: &str,
        limit: usize,
        max_chunks: usize,
        allowed: &dyn Fn(&str) -> bool,
    ) -> Vec<SearchResult> {
        let fallback = || index.semantic_search_allowed(query, limit, allowed);
        let Some(model) = shared_model(data_dir) else {
            return fallback();
        };
        let Ok(query_vector) = model.embed(query) else {
            return fallback();
        };

        // A background fill is finishing this repository: answer by keyword
        // now rather than wait for it to let go of the vectors.
        if is_filling(&index.repository_id) {
            return fallback();
        }
        let entry = cached_index(data_dir, &index.repository_id);
        let mut cached = entry
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let CachedVectorIndex {
            index: vectors,
            path,
            over_cap_reported,
        } = &mut *cached;
        let mut save = |vectors: &VectorIndex| vectors.save(path);
        match vectors.sync(index, model, max_chunks, SYNC_BUDGET, &mut save) {
            SyncState::Complete => vectors.search(
                &query_vector,
                index,
                limit,
                allowed,
                !crate::indexer::query_mentions_tests(&crate::indexer::search_terms(query)),
            ),
            SyncState::Partial => {
                drop(cached);
                fill_in_background(entry, index.clone(), model, max_chunks);
                fallback()
            }
            SyncState::Failed => fallback(),
            SyncState::OverCap { total_chunks } => {
                if !*over_cap_reported {
                    *over_cap_reported = true;
                    eprintln!(
                        "Semantic search skipped for {}: {total_chunks} chunks is over \
                         semantic_search_max_chunks ({max_chunks}); using keyword search",
                        index.root_path.display()
                    );
                }
                fallback()
            }
        }
    }

    /// Test-only: drop every cached vector index so tests don't observe
    /// state left behind by earlier tests sharing the same process-wide
    /// registry.
    #[cfg(test)]
    pub fn reset_for_tests() {
        registry()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::ClientError;
    use crate::indexer::FileRecord;

    /// Records the size of every batch it is asked to embed.
    #[derive(Default)]
    struct RecordingEmbedder {
        batches: Mutex<Vec<usize>>,
        fail: bool,
    }

    impl ChunkEmbedder for RecordingEmbedder {
        fn embed_batch(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>> {
            if self.fail {
                return Err(ClientError::Io("no model".to_string()));
            }
            self.batches.lock().unwrap().push(texts.len());
            Ok(texts.iter().map(|_| vec![1.0, 0.0]).collect())
        }
    }

    fn repository(files: usize, chunks_per_file: usize) -> RepositoryIndex {
        RepositoryIndex {
            repository_id: "repo_test".to_string(),
            root_path: PathBuf::from("/repo"),
            indexed_at_ms: 0,
            files: (0..files)
                .map(|file| FileRecord {
                    repository_id: "repo_test".to_string(),
                    path: format!("src/file_{file}.rs"),
                    language: "rust".to_string(),
                    size: 0,
                    modified_time_ms: 0,
                    content_hash: String::new(),
                    symbols: Vec::new(),
                    declarations: Vec::new(),
                    imports: Vec::new(),
                    terms: Vec::new(),
                    name_terms: Vec::new(),
                    symbol_terms: Vec::new(),
                    chunks: (0..chunks_per_file)
                        .map(|ordinal| TextChunk {
                            ordinal,
                            start: 0,
                            end: 0,
                            lines: crate::file_access::LineRange { start: 1, end: 1 },
                            text: format!("file {file} chunk {ordinal}"),
                            text_hash: format!("hash-{file}-{ordinal}"),
                        })
                        .collect(),
                })
                .collect(),
            skipped: Vec::new(),
        }
    }

    fn sync(
        vectors: &mut VectorIndex,
        index: &RepositoryIndex,
        embedder: &dyn ChunkEmbedder,
        max_chunks: usize,
        budget: Duration,
    ) -> (SyncState, usize) {
        let mut saves = 0;
        let state = vectors.sync(index, embedder, max_chunks, budget, &mut |_| saves += 1);
        (state, saves)
    }

    #[test]
    fn a_large_sync_never_sends_the_model_more_than_one_batch() {
        let index = repository(100, 10);
        let embedder = RecordingEmbedder::default();
        let mut vectors = VectorIndex::default();

        let (state, _) = sync(&mut vectors, &index, &embedder, 100_000, Duration::MAX);

        assert_eq!(state, SyncState::Complete);
        assert_eq!(vectors.entries.len(), 1_000);
        let batches = embedder.batches.lock().unwrap();
        assert!(
            batches.iter().all(|size| *size <= SYNC_BATCH_CHUNKS),
            "{batches:?}"
        );
        assert_eq!(batches.iter().sum::<usize>(), 1_000);
    }

    #[test]
    fn a_sync_that_runs_out_of_budget_keeps_its_progress_and_resumes() {
        let index = repository(10, 10);
        let embedder = RecordingEmbedder::default();
        let mut vectors = VectorIndex::default();

        let (first, saves) = sync(&mut vectors, &index, &embedder, 100_000, Duration::ZERO);

        assert_eq!(first, SyncState::Partial);
        assert_eq!(vectors.entries.len(), SYNC_BATCH_CHUNKS);
        assert_eq!(saves, 1, "progress is saved before answering");

        let (second, _) = sync(&mut vectors, &index, &embedder, 100_000, Duration::MAX);

        assert_eq!(second, SyncState::Complete);
        assert_eq!(vectors.entries.len(), 100);
        assert_eq!(
            embedder.batches.lock().unwrap().iter().sum::<usize>(),
            100,
            "chunks embedded before the budget ran out are not embedded again"
        );
    }

    #[test]
    fn a_long_sync_saves_its_progress_along_the_way() {
        let index = repository(1, SYNC_BATCH_CHUNKS * SAVE_EVERY_BATCHES * 2);
        let embedder = RecordingEmbedder::default();
        let mut vectors = VectorIndex::default();

        let (state, saves) = sync(&mut vectors, &index, &embedder, 100_000, Duration::MAX);

        assert_eq!(state, SyncState::Complete);
        assert_eq!(saves, 2, "one save mid-way and one at the end");
    }

    #[test]
    fn a_repository_over_the_cap_embeds_nothing_and_drops_what_it_held() {
        let index = repository(10, 10);
        let embedder = RecordingEmbedder::default();
        let mut vectors = VectorIndex::default();
        sync(&mut vectors, &index, &embedder, 100, Duration::MAX);
        embedder.batches.lock().unwrap().clear();

        let (state, saves) = sync(&mut vectors, &index, &embedder, 99, Duration::MAX);

        assert_eq!(state, SyncState::OverCap { total_chunks: 100 });
        assert!(embedder.batches.lock().unwrap().is_empty());
        assert!(vectors.entries.is_empty());
        assert_eq!(saves, 1, "the emptied index replaces the file");
    }

    #[test]
    fn an_unchanged_repository_embeds_and_saves_nothing() {
        let index = repository(3, 3);
        let embedder = RecordingEmbedder::default();
        let mut vectors = VectorIndex::default();
        sync(&mut vectors, &index, &embedder, 100, Duration::MAX);
        embedder.batches.lock().unwrap().clear();

        let (state, saves) = sync(&mut vectors, &index, &embedder, 100, Duration::MAX);

        assert_eq!(state, SyncState::Complete);
        assert!(embedder.batches.lock().unwrap().is_empty());
        assert_eq!(saves, 0);
    }

    #[test]
    fn a_model_failure_keeps_earlier_vectors_and_reports_it() {
        let index = repository(2, 2);
        let mut vectors = VectorIndex::default();
        let failing = RecordingEmbedder {
            fail: true,
            ..RecordingEmbedder::default()
        };

        let (state, _) = sync(&mut vectors, &index, &failing, 100, Duration::MAX);

        assert_eq!(state, SyncState::Failed);
        assert!(vectors.entries.is_empty());
    }

    #[test]
    fn only_the_most_recently_searched_repositories_stay_loaded() {
        let data_dir =
            std::env::temp_dir().join(format!("damaian-vector-cache-{}", std::process::id()));
        for repository in 0..=CACHED_REPOSITORIES {
            cached_index(&data_dir, &format!("repo_evict_{repository}"));
        }
        let loaded: Vec<String> = registry()
            .lock()
            .unwrap()
            .keys()
            .filter(|id| id.starts_with("repo_evict_"))
            .cloned()
            .collect();
        assert!(loaded.len() <= CACHED_REPOSITORIES, "{loaded:?}");
        assert!(!loaded.contains(&"repo_evict_0".to_string()), "{loaded:?}");
    }

    fn leaked(embedder: RecordingEmbedder) -> &'static RecordingEmbedder {
        Box::leak(Box::new(embedder))
    }

    fn entry_at(path: PathBuf) -> Arc<Mutex<CachedVectorIndex>> {
        Arc::new(Mutex::new(CachedVectorIndex {
            index: VectorIndex::default(),
            path,
            over_cap_reported: false,
        }))
    }

    fn scratch_file(name: &str) -> PathBuf {
        std::env::temp_dir()
            .join(format!(
                "damaian-vector-fill-{name}-{}-{}",
                std::process::id(),
                crate::hash::now_millis()
            ))
            .join("vectors.bin")
    }

    fn with_id(mut index: RepositoryIndex, id: &str) -> RepositoryIndex {
        index.repository_id = id.to_string();
        index
    }

    #[test]
    fn a_background_fill_finishes_the_index_in_bounded_batches_and_saves_it() {
        let index = with_id(repository(40, 10), "repo_fill_finishes");
        let embedder = leaked(RecordingEmbedder::default());
        let path = scratch_file("finishes");
        let entry = entry_at(path.clone());

        fill_in_background(entry.clone(), index, embedder, 100_000)
            .expect("a fill starts")
            .join()
            .unwrap();

        assert_eq!(entry.lock().unwrap().index.entries.len(), 400);
        let batches = embedder.batches.lock().unwrap();
        assert!(
            batches.iter().all(|size| *size <= SYNC_BATCH_CHUNKS),
            "{batches:?}"
        );
        assert_eq!(
            VectorIndex::load(&path).entries.len(),
            400,
            "the finished index is on disk"
        );
        assert!(
            !is_filling("repo_fill_finishes"),
            "the repository is free for the next fill"
        );
    }

    #[test]
    fn only_one_fill_runs_per_repository() {
        filling()
            .lock()
            .unwrap()
            .insert("repo_fill_once".to_string());
        let index = with_id(repository(2, 2), "repo_fill_once");
        let embedder = leaked(RecordingEmbedder::default());

        let second = fill_in_background(entry_at(scratch_file("once")), index, embedder, 100);

        assert!(second.is_none());
        assert!(embedder.batches.lock().unwrap().is_empty());
        filling().lock().unwrap().remove("repo_fill_once");
    }

    #[test]
    fn a_fill_whose_model_fails_stops_instead_of_retrying_forever() {
        let index = with_id(repository(5, 5), "repo_fill_fails");
        let embedder = leaked(RecordingEmbedder {
            fail: true,
            ..RecordingEmbedder::default()
        });

        fill_in_background(entry_at(scratch_file("fails")), index, embedder, 100)
            .expect("a fill starts")
            .join()
            .unwrap();

        assert!(!is_filling("repo_fill_fails"));
    }

    #[test]
    fn a_fill_over_the_cap_embeds_nothing() {
        let index = with_id(repository(5, 5), "repo_fill_cap");
        let embedder = leaked(RecordingEmbedder::default());
        let entry = entry_at(scratch_file("cap"));

        fill_in_background(entry.clone(), index, embedder, 10)
            .expect("a fill starts")
            .join()
            .unwrap();

        assert!(embedder.batches.lock().unwrap().is_empty());
        assert!(entry.lock().unwrap().index.entries.is_empty());
    }

    /// Measures a real background fill: the embedding model on 2,000
    /// source-sized chunks, timed, with the process's peak memory.
    /// `#[ignore]`d per `AGENTS.md` because it needs the model, which the first
    /// run downloads from huggingface.co. Run it by hand:
    ///
    /// ```sh
    /// cargo test -p workspace-engine --lib --release -- --ignored --nocapture \
    ///   vector_index::tests::a_real_background_fill_stays_within_memory
    /// ```
    #[test]
    #[ignore]
    fn a_real_background_fill_stays_within_memory() {
        let data_dir = std::env::temp_dir().join("damaian-test-embedding-cache");
        let model = shared_model(&data_dir).expect("the embedding model loads");
        let mut index = with_id(repository(200, 10), "repo_fill_real");
        for file in &mut index.files {
            for chunk in &mut file.chunks {
                chunk.text = format!(
                    "fn handler_{}_{}() {{ {} }}",
                    file.path,
                    chunk.ordinal,
                    "let value = compute(input) + offset; ".repeat(60)
                );
            }
        }
        let entry = entry_at(scratch_file("real"));
        let started = Instant::now();

        fill_in_background(entry.clone(), index, model, 100_000)
            .expect("a fill starts")
            .join()
            .unwrap();

        let elapsed = started.elapsed();
        let mut usage: libc::rusage = unsafe { std::mem::zeroed() };
        // SAFETY: `usage` is a zeroed `rusage` that `getrusage` fills in.
        unsafe { libc::getrusage(libc::RUSAGE_SELF, &mut usage) };
        // macOS reports `ru_maxrss` in bytes.
        let peak_mb = usage.ru_maxrss / (1024 * 1024);
        let embedded = entry.lock().unwrap().index.entries.len();
        println!(
            "filled {embedded} chunks in {elapsed:?} ({:.0}/s); peak memory {peak_mb} MB",
            embedded as f64 / elapsed.as_secs_f64()
        );
        assert_eq!(embedded, 2_000);
        assert!(peak_mb < 1_500, "peak memory {peak_mb} MB");
    }
}
