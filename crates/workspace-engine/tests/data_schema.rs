//! The data directory schema marker, per
//! `docs/specs/15_install_and_update_verification.md` §5.2 and §5.3.
//!
//! The realistic failure this guards is a user who updates, hits a problem,
//! reinstalls the previous version, and has that older build write over data
//! the newer one had already reorganised. Refusing is cheap; recovering the
//! sessions is not — so the load-bearing assertion here is that a refusal
//! changes nothing on disk.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};
use workspace_engine::{
    CURRENT_DATA_SCHEMA_VERSION, DataSchemaOutcome, SessionStore, ensure_data_dir_schema,
};

static COUNTER: AtomicU64 = AtomicU64::new(1);

/// A path under the system temp directory that does not exist yet. Callers that
/// need the directory create it themselves, because "the directory is absent"
/// is one of the cases under test.
fn temp_path(name: &str) -> PathBuf {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock should work")
        .as_nanos();
    std::env::temp_dir().join(format!(
        "damaian-schema-{name}-{now}-{}",
        COUNTER.fetch_add(1, Ordering::Relaxed)
    ))
}

fn marker_path(data_dir: &Path) -> PathBuf {
    data_dir.join("schema.conf")
}

/// A data directory as every existing install has it: real content, no marker.
fn data_dir_with_session(name: &str) -> PathBuf {
    let data_dir = temp_path(name);
    let store = SessionStore::new(&data_dir);
    store
        .create_session("repo-1", "existing work")
        .expect("session should be created");
    data_dir
}

/// Every file under `root`, with its length and modification time. Compared
/// before and after a refusal: a refusal that still writes is not a refusal.
fn snapshot(root: &Path) -> BTreeMap<PathBuf, (u64, SystemTime)> {
    let mut entries = BTreeMap::new();
    let mut pending = vec![root.to_path_buf()];
    while let Some(dir) = pending.pop() {
        let Ok(read) = fs::read_dir(&dir) else {
            continue;
        };
        for entry in read {
            let entry = entry.expect("directory entry should be readable");
            let metadata = entry.metadata().expect("metadata should be readable");
            if metadata.is_dir() {
                pending.push(entry.path());
                continue;
            }
            entries.insert(
                entry.path(),
                (
                    metadata.len(),
                    metadata.modified().expect("mtime should be readable"),
                ),
            );
        }
    }
    entries
}

#[test]
fn absent_data_directory_is_initialized_at_the_current_version() {
    let data_dir = temp_path("first-run");

    let outcome = ensure_data_dir_schema(&data_dir).expect("first run should be accepted");

    assert_eq!(outcome, DataSchemaOutcome::Initialized);
    assert_eq!(
        fs::read_to_string(marker_path(&data_dir)).expect("marker should be written"),
        format!("schema_version={CURRENT_DATA_SCHEMA_VERSION}\n")
    );
}

#[test]
fn existing_content_without_a_marker_is_adopted_as_version_one() {
    let data_dir = data_dir_with_session("adopt");

    let outcome = ensure_data_dir_schema(&data_dir).expect("existing install should be adopted");

    assert_eq!(outcome, DataSchemaOutcome::Adopted);
    assert_eq!(
        fs::read_to_string(marker_path(&data_dir)).expect("marker should be written"),
        "schema_version=1\n"
    );
    let sessions = SessionStore::new(&data_dir)
        .list_sessions(None)
        .expect("sessions should still load");
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].title, "existing work");
}

#[test]
fn marker_at_the_current_version_is_accepted_and_left_alone() {
    let data_dir = data_dir_with_session("current");
    fs::write(
        marker_path(&data_dir),
        format!("schema_version={CURRENT_DATA_SCHEMA_VERSION}\n"),
    )
    .expect("marker should be written");
    let before = snapshot(&data_dir);

    let outcome = ensure_data_dir_schema(&data_dir).expect("current version should be accepted");

    assert_eq!(outcome, DataSchemaOutcome::Current);
    assert_eq!(snapshot(&data_dir), before);
}

#[test]
fn unknown_newer_version_is_refused_without_touching_the_directory() {
    let data_dir = data_dir_with_session("newer");
    fs::write(marker_path(&data_dir), "schema_version=999\n").expect("marker should be written");
    let before = snapshot(&data_dir);

    let error = ensure_data_dir_schema(&data_dir).expect_err("a newer version should be refused");

    let message = error.to_string();
    assert!(
        message.contains(&data_dir.to_string_lossy().to_string()),
        "refusal should name the data directory: {message}"
    );
    assert!(
        message.contains("999"),
        "refusal should name the version found: {message}"
    );
    assert!(
        message.contains(&CURRENT_DATA_SCHEMA_VERSION.to_string()),
        "refusal should name the version supported: {message}"
    );
    assert_eq!(snapshot(&data_dir), before);
}

#[test]
fn malformed_version_is_refused_rather_than_treated_as_missing() {
    let data_dir = data_dir_with_session("malformed");
    fs::write(marker_path(&data_dir), "schema_version=banana\n").expect("marker should be written");
    let before = snapshot(&data_dir);

    let error =
        ensure_data_dir_schema(&data_dir).expect_err("a malformed marker should be refused");

    let message = error.to_string();
    assert!(
        message.contains("banana"),
        "refusal should quote what it found: {message}"
    );
    assert!(
        message.contains(&data_dir.to_string_lossy().to_string()),
        "refusal should name the data directory: {message}"
    );
    assert_eq!(snapshot(&data_dir), before);
}
