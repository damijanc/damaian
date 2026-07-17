use crate::embeddings::{EmbeddingModel, shared_model};
use crate::hash::sha256;
use crate::indexer::{RepositoryIndex, SearchResult};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

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

    /// Recomputes embeddings only for chunks whose stored hash no longer
    /// matches the current repository index (new or edited chunks), and
    /// drops entries for files/chunks that no longer exist. Returns whether
    /// anything changed, so the caller only rewrites the file when needed.
    fn sync(&mut self, index: &RepositoryIndex, model: &EmbeddingModel) -> bool {
        let mut existing: HashMap<(String, usize), StoredChunkVector> = self
            .entries
            .drain(..)
            .map(|entry| ((entry.path.clone(), entry.ordinal), entry))
            .collect();
        let matched_count = existing.len();

        let mut fresh = Vec::new();
        let mut pending: Vec<(String, usize, String)> = Vec::new();
        for file in &index.files {
            for chunk in &file.chunks {
                let key = (file.path.clone(), chunk.ordinal);
                match existing.remove(&key) {
                    Some(entry) if entry.text_hash == chunk.text_hash => fresh.push(entry),
                    _ => pending.push((file.path.clone(), chunk.ordinal, chunk.text.clone())),
                }
            }
        }
        let removed_count = existing.len();
        let changed = !pending.is_empty() || removed_count > 0 || matched_count != fresh.len();

        if !pending.is_empty() {
            let texts: Vec<&str> = pending.iter().map(|(_, _, text)| text.as_str()).collect();
            if let Ok(vectors) = model.embed_batch(&texts) {
                for ((path, ordinal, text), vector) in pending.into_iter().zip(vectors) {
                    fresh.push(StoredChunkVector {
                        path,
                        ordinal,
                        text_hash: sha256(text.as_bytes()),
                        vector,
                    });
                }
            }
        }

        self.entries = fresh;
        changed
    }

    fn search(&self, query_vector: &[f32], index: &RepositoryIndex, limit: usize) -> Vec<SearchResult> {
        let mut scored: Vec<(f32, &StoredChunkVector)> = self
            .entries
            .iter()
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
                let chunk = file.chunks.iter().find(|chunk| chunk.ordinal == entry.ordinal)?;
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
}

type Registry = Mutex<HashMap<String, Arc<Mutex<CachedVectorIndex>>>>;
static REGISTRY: OnceLock<Registry> = OnceLock::new();

fn registry() -> &'static Registry {
    REGISTRY.get_or_init(|| Mutex::new(HashMap::new()))
}

fn vector_index_path(data_dir: &Path, repository_id: &str) -> PathBuf {
    data_dir.join("vector-index").join(format!("{repository_id}.bin"))
}

/// Real, embedding-based semantic search, backed by a process-wide cache of
/// per-repository vectors (mirroring `IndexCache`'s pattern for
/// `RepositoryIndex`). Falls back to `RepositoryIndex::semantic_search`'s
/// term-overlap search whenever a local embedding model isn't available
/// (disabled in config, first-run download failed, or embedding the query
/// failed), so callers always get a result either way.
pub struct VectorIndexCache;

impl VectorIndexCache {
    pub fn semantic_search(
        data_dir: &Path,
        index: &RepositoryIndex,
        query: &str,
        limit: usize,
    ) -> Vec<SearchResult> {
        let Some(model) = shared_model(data_dir) else {
            return index.semantic_search(query, limit);
        };
        let Ok(query_vector) = model.embed(query) else {
            return index.semantic_search(query, limit);
        };

        let path = vector_index_path(data_dir, &index.repository_id);
        let entry = registry()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .entry(index.repository_id.clone())
            .or_insert_with(|| {
                Arc::new(Mutex::new(CachedVectorIndex {
                    index: VectorIndex::load(&path),
                    path: path.clone(),
                }))
            })
            .clone();

        let mut cached = entry.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        if cached.index.sync(index, model) {
            cached.index.save(&cached.path);
        }
        cached.index.search(&query_vector, index, limit)
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
