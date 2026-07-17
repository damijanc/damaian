use std::fs;
use std::path::{Path, PathBuf};
use workspace_engine::embeddings::EmbeddingModel;
use workspace_engine::vector_index::VectorIndexCache;
use workspace_engine::{AuditLog, Config, ProjectIndexer, SecretScanner};

/// A stable (non-timestamped) cache directory shared across test runs on
/// this machine, so the ~90MB embedding model is downloaded once rather
/// than on every `cargo test` invocation. Real network access is required
/// the first time; later runs reuse the cached files.
fn embedding_cache_dir() -> PathBuf {
    let dir = std::env::temp_dir().join("damaian-test-embedding-cache");
    fs::create_dir_all(&dir).expect("cache dir should be created");
    dir
}

fn temp_repo_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "damaian-semantic-{name}-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&dir).expect("temp dir should be created");
    dir
}

fn write_fixture(root: &Path, relative_path: &str, content: &str) {
    let path = root.join(relative_path);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, content).unwrap();
}

#[test]
fn embedding_model_scores_semantically_similar_text_higher_than_unrelated_text() {
    let model = EmbeddingModel::load(&embedding_cache_dir())
        .expect("embedding model should load (requires network on first run)");

    let query = model
        .embed("how do we retry a failed network request")
        .expect("query should embed");
    let related = model
        .embed("the client automatically retries the API call after a timeout using exponential backoff")
        .expect("related text should embed");
    let unrelated = model
        .embed("the cat sat quietly on the windowsill in the afternoon sun")
        .expect("unrelated text should embed");

    let similarity_related = cosine(&query, &related);
    let similarity_unrelated = cosine(&query, &unrelated);

    assert!(
        similarity_related > similarity_unrelated,
        "expected semantically related text to score higher: related={similarity_related}, unrelated={similarity_unrelated}"
    );
    // The two sentences share zero words, so this margin can only come from
    // genuine semantic similarity, not lexical overlap.
    assert!(
        similarity_related - similarity_unrelated > 0.15,
        "expected a meaningful margin: related={similarity_related}, unrelated={similarity_unrelated}"
    );
}

#[test]
fn vector_index_cache_finds_semantically_related_code_that_keyword_search_misses() {
    let repo = temp_repo_dir("vector-index");
    write_fixture(
        &repo,
        "src/resilience.rs",
        "fn resend_with_growing_delay(payload: Payload) -> Outcome {\n    \
         // Sends the payload again with a longer pause each time it does\n    \
         // not succeed, and eventually stops trying and reports the problem.\n    \
         resend_with_backoff(3, || channel.transmit(payload.clone()))\n}\n",
    );
    write_fixture(
        &repo,
        "src/unrelated.rs",
        "fn render_greeting(name: &str) -> String {\n    format!(\"Hello, {name}!\")\n}\n",
    );

    let config = Config {
        data_dir: repo.join(".damaian"),
        enable_semantic_search: true,
        ..Config::default()
    };
    let scanner = SecretScanner::new(config.secret_patterns.clone());
    let audit_log = AuditLog::new(&config.data_dir, true, scanner.clone());
    let indexer = ProjectIndexer::new(config.clone(), scanner, audit_log);
    let index = indexer
        .index_repository(&repo)
        .expect("repository should index");

    let query = "what happens when a network operation keeps failing";

    // `keyword_search` matches on substring containment with no stopword
    // filtering (see `score_record` in indexer.rs), so short/common tokens
    // like "a" trivially match almost any file. It's not a useful signal
    // here; the point of this test is what `semantic_search` finds that
    // ranking alone would bury.
    let keyword_hits = index.keyword_search(query, 5);
    let keyword_top_hit = keyword_hits.first().map(|hit| hit.path.as_str());

    let semantic_hits = VectorIndexCache::semantic_search(&config.data_dir, &index, query, 5);
    let top_semantic_hit = semantic_hits
        .first()
        .expect("semantic search should return at least one result");
    assert_eq!(
        top_semantic_hit.path, "src/resilience.rs",
        "expected semantic search to rank the retry logic first by meaning, got: {semantic_hits:?}"
    );
    assert_ne!(
        keyword_top_hit,
        Some("src/resilience.rs"),
        "test is only meaningful if keyword search did not already find the right file for the wrong reason"
    );

    let vector_index_path = config
        .data_dir
        .join("vector-index")
        .join(format!("{}.bin", index.repository_id));
    assert!(
        vector_index_path.exists(),
        "expected vector index to be persisted to disk"
    );

    // Re-running the same search against an unchanged index should not
    // rewrite the persisted file (nothing to re-embed).
    let modified_before = fs::metadata(&vector_index_path).unwrap().modified().unwrap();
    std::thread::sleep(std::time::Duration::from_millis(20));
    let _ = VectorIndexCache::semantic_search(&config.data_dir, &index, query, 5);
    let modified_after = fs::metadata(&vector_index_path).unwrap().modified().unwrap();
    assert_eq!(
        modified_before, modified_after,
        "expected an unchanged index to skip re-writing the vector store"
    );
}

fn cosine(a: &[f32], b: &[f32]) -> f32 {
    let dot: f32 = a.iter().zip(b).map(|(x, y)| x * y).sum();
    let norm_a: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let norm_b: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();
    dot / (norm_a * norm_b)
}
