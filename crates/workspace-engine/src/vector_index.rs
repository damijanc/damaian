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

/// `semantic_search_max_chunks` as a count this machine can index with.
pub fn semantic_search_max_chunks(config: &crate::config::Config) -> usize {
    usize::try_from(config.semantic_search_max_chunks).unwrap_or(usize::MAX)
}

/// Turns chunk texts into vectors. [`EmbeddingModel`] in the app; tests
/// substitute one that records what it was asked.
pub trait ChunkEmbedder {
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
    /// Chunks are still pending: the budget ran out, or the model failed.
    Partial,
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
                return SyncState::Partial;
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

    fn search(
        &self,
        query_vector: &[f32],
        index: &RepositoryIndex,
        limit: usize,
        allowed: &dyn Fn(&str) -> bool,
    ) -> Vec<SearchResult> {
        let mut scored: Vec<(f32, &StoredChunkVector)> = self
            .entries
            .iter()
            .filter(|entry| allowed(&entry.path))
            .map(|entry| (cosine_similarity(query_vector, &entry.vector), entry))
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
            SyncState::Complete => vectors.search(&query_vector, index, limit, allowed),
            SyncState::Partial => fallback(),
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
    use std::cell::RefCell;

    /// Records the size of every batch it is asked to embed.
    #[derive(Default)]
    struct RecordingEmbedder {
        batches: RefCell<Vec<usize>>,
        fail: bool,
    }

    impl ChunkEmbedder for RecordingEmbedder {
        fn embed_batch(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>> {
            if self.fail {
                return Err(ClientError::Io("no model".to_string()));
            }
            self.batches.borrow_mut().push(texts.len());
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
                    chunks: (0..chunks_per_file)
                        .map(|ordinal| TextChunk {
                            ordinal,
                            start: 0,
                            end: 0,
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
        let batches = embedder.batches.borrow();
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
            embedder.batches.borrow().iter().sum::<usize>(),
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
        embedder.batches.borrow_mut().clear();

        let (state, saves) = sync(&mut vectors, &index, &embedder, 99, Duration::MAX);

        assert_eq!(state, SyncState::OverCap { total_chunks: 100 });
        assert!(embedder.batches.borrow().is_empty());
        assert!(vectors.entries.is_empty());
        assert_eq!(saves, 1, "the emptied index replaces the file");
    }

    #[test]
    fn an_unchanged_repository_embeds_and_saves_nothing() {
        let index = repository(3, 3);
        let embedder = RecordingEmbedder::default();
        let mut vectors = VectorIndex::default();
        sync(&mut vectors, &index, &embedder, 100, Duration::MAX);
        embedder.batches.borrow_mut().clear();

        let (state, saves) = sync(&mut vectors, &index, &embedder, 100, Duration::MAX);

        assert_eq!(state, SyncState::Complete);
        assert!(embedder.batches.borrow().is_empty());
        assert_eq!(saves, 0);
    }

    #[test]
    fn a_model_failure_keeps_earlier_vectors_and_reports_partial() {
        let index = repository(2, 2);
        let mut vectors = VectorIndex::default();
        let failing = RecordingEmbedder {
            fail: true,
            ..RecordingEmbedder::default()
        };

        let (state, _) = sync(&mut vectors, &index, &failing, 100, Duration::MAX);

        assert_eq!(state, SyncState::Partial);
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
}
