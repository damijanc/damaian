use crate::error::Result;
use crate::hash::now_millis;
use crate::indexer::{ProjectIndexer, RepositoryIndex};
use notify::{RecursiveMode, Watcher};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::sync::{Arc, Mutex, OnceLock};
use std::thread;

/// Safety-net rescan interval: even if the watcher never reports an error,
/// force a full walk periodically so any drift (missed events, watcher
/// backend quirks) self-corrects, per the spec's "fall back to periodic
/// rescan" requirement.
const FULL_RESCAN_INTERVAL_MS: u128 = 5 * 60 * 1000;

struct CachedIndex {
    index: Option<RepositoryIndex>,
    last_full_rescan_ms: u128,
    /// Whether this repository already has a watcher. The watcher itself is
    /// owned by the thread `spawn_watcher` starts, not by this struct, so that
    /// registering it never blocks a caller — see `spawn_watcher`.
    watcher_started: bool,
}

type Registry = Mutex<HashMap<String, Arc<Mutex<CachedIndex>>>>;

static REGISTRY: OnceLock<Registry> = OnceLock::new();

fn registry() -> &'static Registry {
    REGISTRY.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Process-wide cache of repository indexes, shared by every caller
/// (`ChatOrchestrator`, `EditOrchestrator`) regardless of how many
/// short-lived `WorkspaceEngine`/`ProjectIndexer` instances are constructed
/// around it. A background filesystem watcher keeps each cached repository
/// fresh incrementally; a periodic full rescan is the fallback for anything
/// the watcher misses.
pub struct IndexCache;

impl IndexCache {
    pub fn get_or_build(
        indexer: &ProjectIndexer,
        root_path: impl AsRef<Path>,
    ) -> Result<RepositoryIndex> {
        let root = std::fs::canonicalize(root_path)?;
        let repository_id = indexer.repository_id_for_path(&root)?;

        let entry = registry()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .entry(repository_id)
            .or_insert_with(|| {
                Arc::new(Mutex::new(CachedIndex {
                    index: None,
                    last_full_rescan_ms: 0,
                    watcher_started: false,
                }))
            })
            .clone();

        let mut cached = entry
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let stale = cached.index.is_none()
            || now_millis().saturating_sub(cached.last_full_rescan_ms) > FULL_RESCAN_INTERVAL_MS;
        if stale {
            cached.index = Some(indexer.index_repository(&root)?);
            cached.last_full_rescan_ms = now_millis();
        }
        if !cached.watcher_started && indexer.config().enable_index_watcher {
            cached.watcher_started = true;
            spawn_watcher(indexer.clone(), root.clone(), entry.clone());
        }
        Ok(cached
            .index
            .clone()
            .expect("index was just populated above"))
    }

    /// Test-only: drop every cached repository so tests don't observe state
    /// left behind by earlier tests sharing the same process-wide registry.
    #[cfg(test)]
    pub fn reset_for_tests() {
        registry()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clear();
    }
}

/// Starts watching `root`, off the calling thread.
///
/// Registration happens on the spawned thread rather than here on purpose.
/// `notify`'s FSEvents backend blocks inside `watch()` until `fseventsd` has
/// registered the stream, and on macOS that routinely costs ten seconds or
/// more. Paying it here stalled the first turn against every newly opened
/// repository, and — because each test builds its own throwaway repository —
/// it was where the test suite spent the bulk of its wall time.
///
/// The watcher is owned by the spawned thread, which parks on `rx` for as long
/// as the process lives. That matches the lifetime of the cache entry it
/// feeds: the registry is process-wide and entries are never evicted.
fn spawn_watcher(indexer: ProjectIndexer, root: PathBuf, entry: Arc<Mutex<CachedIndex>>) {
    thread::spawn(move || {
        let (tx, rx) = mpsc::channel();
        let Ok(mut watcher) = notify::recommended_watcher(tx) else {
            return;
        };
        if watcher.watch(&root, RecursiveMode::Recursive).is_err() {
            return;
        }

        // FSEvents only reports what happens after registration, and
        // registration finished well after `get_or_build` handed the index
        // back. Anything that changed in between was seen by neither the walk
        // nor the watcher, so the index cannot be vouched for: force one
        // rescan on next access rather than serve it until the periodic one
        // comes round.
        if let Ok(mut cached) = entry.lock() {
            cached.last_full_rescan_ms = 0;
        }

        // `watcher` holds the sending half, so keeping it alive for the rest of
        // this scope is what keeps `rx` connected.
        for event in rx {
            match event {
                Ok(event) => {
                    for path in &event.paths {
                        apply_single_path_change(&indexer, &root, path, &entry);
                    }
                }
                Err(_) => {
                    // Watcher-level error (e.g. an overflowed event queue):
                    // force a full rescan on next access rather than trying
                    // to reason about what was missed.
                    if let Ok(mut cached) = entry.lock() {
                        cached.last_full_rescan_ms = 0;
                    }
                }
            }
        }
    });
}

fn apply_single_path_change(
    indexer: &ProjectIndexer,
    root: &Path,
    changed_path: &Path,
    entry: &Arc<Mutex<CachedIndex>>,
) {
    let Ok(relative_path) = changed_path.strip_prefix(root) else {
        return;
    };
    let relative_path = relative_path.to_string_lossy().replace('\\', "/");
    if relative_path.is_empty() {
        return;
    }

    let Ok(mut cached) = entry.lock() else {
        return;
    };
    let Some(index) = cached.index.as_mut() else {
        return;
    };
    let repository_id = index.repository_id.clone();

    match indexer.index_single_file(&repository_id, root, &relative_path) {
        Ok(Some(record)) => {
            index.files.retain(|file| file.path != relative_path);
            index.files.push(record);
        }
        Ok(None) => {
            index.files.retain(|file| file.path != relative_path);
        }
        Err(_) => {
            // Leave the cached record as-is; the periodic full rescan will
            // correct any drift if this keeps failing.
        }
    }
}
