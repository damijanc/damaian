//! Spec 108 D3: archiving a session is durable, filters the session list,
//! is refused for a running worker, and does not stand in the way of delete.

use std::path::PathBuf;

use workspace_engine::{ARCHIVE_RUNNING_WORKER_REFUSAL, ArchivedFilter, SessionStore};

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "damaian-session-archive-{name}-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("scratch dir");
    dir
}

fn ids(sessions: &[workspace_engine::Session]) -> Vec<String> {
    let mut ids: Vec<String> = sessions.iter().map(|session| session.id.clone()).collect();
    ids.sort();
    ids
}

#[test]
fn archive_and_unarchive_survive_a_fresh_store() {
    let data_dir = scratch("round-trip");
    let store = SessionStore::new(&data_dir);
    let session = store.create_session("repo_1", "Title").unwrap();
    assert!(!session.archived);

    let archived = store.archive_session(&session.id, false).unwrap();
    assert!(archived.archived);
    assert_eq!(archived.updated_at_ms, session.updated_at_ms);

    let fresh = SessionStore::new(&data_dir);
    assert!(fresh.read_session(&session.id).unwrap().unwrap().archived);

    // A rename keeps the flag rather than resetting it.
    let renamed = fresh.rename_session(&session.id, "Renamed").unwrap();
    assert!(renamed.archived);
    assert!(
        SessionStore::new(&data_dir)
            .read_session(&session.id)
            .unwrap()
            .unwrap()
            .archived
    );

    let unarchived = fresh.unarchive_session(&session.id).unwrap();
    assert!(!unarchived.archived);
    let reread = SessionStore::new(&data_dir)
        .read_session(&session.id)
        .unwrap()
        .unwrap();
    assert!(!reread.archived);
    assert_eq!(reread.title, "Renamed");
}

#[test]
fn archiving_twice_writes_one_event() {
    let data_dir = scratch("idempotent");
    let store = SessionStore::new(&data_dir);
    let session = store.create_session("repo_1", "Title").unwrap();
    store.archive_session(&session.id, false).unwrap();
    store.archive_session(&session.id, false).unwrap();
    store.unarchive_session(&session.id).unwrap();
    store.unarchive_session(&session.id).unwrap();

    let log = std::fs::read_to_string(
        data_dir
            .join("sessions")
            .join(format!("{}.jsonl", session.id)),
    )
    .unwrap();
    assert_eq!(log.matches("\"session_archived\"").count(), 1);
    assert_eq!(log.matches("\"session_unarchived\"").count(), 1);
}

#[test]
fn listing_filters_archived_sessions() {
    let data_dir = scratch("filter");
    let store = SessionStore::new(&data_dir);
    let kept = store.create_session("repo_1", "Kept").unwrap();
    let archived = store.create_session("repo_1", "Archived").unwrap();
    let other_repo = store.create_session("repo_2", "Elsewhere").unwrap();
    store.archive_session(&archived.id, false).unwrap();
    store.archive_session(&other_repo.id, false).unwrap();

    let all = store.list_sessions(Some("repo_1")).unwrap();
    assert_eq!(ids(&all), ids(&[kept.clone(), archived.clone()]));
    assert_eq!(
        ids(&store
            .list_sessions_filtered(Some("repo_1"), ArchivedFilter::Include)
            .unwrap()),
        ids(&all)
    );
    assert_eq!(
        ids(&store
            .list_sessions_filtered(Some("repo_1"), ArchivedFilter::Exclude)
            .unwrap()),
        vec![kept.id.clone()]
    );
    let only = store
        .list_sessions_filtered(Some("repo_1"), ArchivedFilter::Only)
        .unwrap();
    assert_eq!(ids(&only), vec![archived.id.clone()]);
    assert!(only[0].archived);
    assert_eq!(
        ids(&store
            .list_sessions_filtered(None, ArchivedFilter::Only)
            .unwrap()),
        ids(&[archived, other_repo])
    );
}

#[test]
fn a_running_worker_is_refused() {
    let data_dir = scratch("running-worker");
    let store = SessionStore::new(&data_dir);
    let session = store.create_session("repo_1", "Worker").unwrap();

    let error = store.archive_session(&session.id, true).unwrap_err();
    assert!(
        error.to_string().contains(ARCHIVE_RUNNING_WORKER_REFUSAL),
        "{error}"
    );
    assert!(!store.read_session(&session.id).unwrap().unwrap().archived);
}

#[test]
fn an_unknown_session_is_refused() {
    let data_dir = scratch("unknown");
    let store = SessionStore::new(&data_dir);
    assert!(store.archive_session("session_missing", false).is_err());
    assert!(store.unarchive_session("session_missing").is_err());
}

#[test]
fn delete_still_deletes_an_archived_session() {
    let data_dir = scratch("delete");
    let store = SessionStore::new(&data_dir);
    let session = store.create_session("repo_1", "Doomed").unwrap();
    store.archive_session(&session.id, false).unwrap();

    store.delete_session(&session.id).unwrap();
    assert!(store.read_session(&session.id).unwrap().is_none());
    assert!(
        store
            .list_sessions_filtered(None, ArchivedFilter::Only)
            .unwrap()
            .is_empty()
    );
}

#[test]
fn a_log_from_before_archive_reads_as_not_archived() {
    let data_dir = scratch("legacy");
    let store = SessionStore::new(&data_dir);
    let log = data_dir.join("sessions").join("legacy.jsonl");
    std::fs::create_dir_all(log.parent().unwrap()).unwrap();
    std::fs::write(
        &log,
        "{\"eventId\":\"e\",\"seq\":1,\"timestampMs\":1,\"eventType\":\"session_created\",\"payload\":{\"id\":\"legacy\",\"repositoryId\":\"repo_1\",\"title\":\"Old\",\"createdAtMs\":1,\"updatedAtMs\":1,\"summary\":\"\"}}\n",
    )
    .unwrap();

    assert!(!store.read_session("legacy").unwrap().unwrap().archived);
    assert_eq!(
        store
            .list_sessions_filtered(Some("repo_1"), ArchivedFilter::Exclude)
            .unwrap()
            .len(),
        1
    );
}

/// A pending turn on disk serialises its `Session`; one written before the
/// flag existed must still load.
#[test]
fn a_serialised_session_from_before_archive_still_deserialises() {
    let session: workspace_engine::Session = serde_json::from_str(
        r#"{"id":"s","repository_id":"r","title":"t","created_at_ms":0,"updated_at_ms":0,"summary":"","origin":"user"}"#,
    )
    .unwrap();
    assert!(!session.archived);
}
