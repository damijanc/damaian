use std::path::PathBuf;

use workspace_engine::SessionStore;

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "damaian-search-export-{name}-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("scratch dir");
    dir
}

#[test]
fn origin_defaults_to_user_and_survives_a_round_trip() {
    let data_dir = scratch("origin");
    let store = SessionStore::new(&data_dir);
    let session = store.create_session("repo_1", "Title").unwrap();
    assert_eq!(session.origin, "user");

    let read = store.read_session(&session.id).unwrap().unwrap();
    assert_eq!(read.origin, "user");
}

#[test]
fn a_session_written_before_origin_defaults_to_user() {
    let data_dir = scratch("origin-legacy");
    let store = SessionStore::new(&data_dir);
    // A session_created event in the pre-origin shape, written by hand.
    let log = data_dir.join("sessions").join("legacy.jsonl");
    std::fs::create_dir_all(log.parent().unwrap()).unwrap();
    std::fs::write(
        &log,
        "{\"eventId\":\"e\",\"seq\":1,\"timestampMs\":1,\"eventType\":\"session_created\",\"payload\":{\"id\":\"legacy\",\"repositoryId\":\"repo_1\",\"title\":\"Old\",\"createdAtMs\":1,\"updatedAtMs\":1,\"summary\":\"\"}}\n",
    )
    .unwrap();

    let read = store.read_session("legacy").unwrap().unwrap();
    assert_eq!(read.origin, "user");
}
