//! Search latency on a large codebase (spec 02 §8, "two-stage ranking").
//!
//! Not a pass/fail test: it prints how long indexing and each search took, so
//! a ranking change can be measured before and after on the same corpus.

use std::path::PathBuf;
use std::time::{Duration, Instant};
use workspace_engine::{AuditLog, Config, ProjectIndexer, SecretScanner};

const QUERIES: &[&str] = &[
    "what part of the code is responsible for user approval and what code is taking care that user is protected",
    "where is an http request retried after a timeout",
    "parse a toml table into a struct",
    "deep water",
    "how are unicode grapheme clusters segmented",
    "serialize a map with sorted keys",
];

fn median(mut samples: Vec<Duration>) -> Duration {
    samples.sort();
    samples[samples.len() / 2]
}

/// Indexes the directory in `DAMAIAN_SEARCH_BENCH_ROOT` and times keyword and
/// term-overlap search over [`QUERIES`], five runs each.
///
/// `#[ignore]`d per `AGENTS.md` because it reads a directory outside the
/// repository and takes minutes on a large one. Its data directory is a fresh
/// temporary one, so it never touches the user's Damaian state. Run it with a
/// release build, for example over the local cargo registry cache:
///
/// ```text
/// DAMAIAN_SEARCH_BENCH_ROOT=~/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f \
///   cargo test --release -p workspace-engine --test search_benchmark -- --ignored --nocapture
/// ```
#[test]
#[ignore]
fn search_latency_on_a_large_codebase() {
    let Some(root) = std::env::var_os("DAMAIAN_SEARCH_BENCH_ROOT").map(PathBuf::from) else {
        println!("set DAMAIAN_SEARCH_BENCH_ROOT to the directory to index");
        return;
    };
    let data_dir =
        std::env::temp_dir().join(format!("damaian-search-bench-{}", std::process::id()));
    let config = Config {
        data_dir: data_dir.clone(),
        enable_index_watcher: false,
        ..Config::default()
    };
    let scanner = SecretScanner::new(config.secret_patterns.clone());
    let audit_log = AuditLog::new(&config.data_dir, false, scanner.clone());
    let indexer = ProjectIndexer::new(config, scanner, audit_log);

    let started = Instant::now();
    let index = indexer
        .index_repository(&root)
        .expect("the benchmark root should index");
    let chunks: usize = index.files.iter().map(|file| file.chunks.len()).sum();
    println!(
        "indexed {} files, {} chunks, {} skipped in {:.1?}",
        index.files.len(),
        chunks,
        index.skipped.len(),
        started.elapsed()
    );

    for query in QUERIES {
        let mut keyword = Vec::new();
        let mut overlap = Vec::new();
        let mut top = Vec::new();
        for _ in 0..5 {
            let started = Instant::now();
            top = index.keyword_search(query, 8);
            keyword.push(started.elapsed());
            let started = Instant::now();
            index.semantic_search(query, 8);
            overlap.push(started.elapsed());
        }
        println!(
            "{query:?}\n  keyword {:.1?} (median of 5), term overlap {:.1?}; top: {}",
            median(keyword),
            median(overlap),
            top.iter()
                .take(3)
                .map(|hit| format!("{} ({})", hit.path, hit.score))
                .collect::<Vec<_>>()
                .join(", ")
        );
    }
    let _ = std::fs::remove_dir_all(data_dir);
}
