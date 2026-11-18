use super::PausedTurns;
use std::fs;

/// Spec 110: the shell resolves a resume request's session from the paused
/// turn before running anything; an unknown id resolves to nothing.
#[test]
fn a_paused_turn_names_its_session_and_an_unknown_id_names_none() {
    let data_dir = std::env::temp_dir().join(format!(
        "damaian-paused-session-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let pending = data_dir.join("chat").join("pending");
    fs::create_dir_all(&pending).unwrap();
    fs::write(
        pending.join("proposal_1.json"),
        r#"{"session":{"id":"session_abc","title":"t"},"proposal_id":"proposal_1"}"#,
    )
    .unwrap();
    let paused = PausedTurns::new(&data_dir);
    assert_eq!(
        paused.session_id("proposal_1").as_deref(),
        Some("session_abc")
    );
    assert_eq!(paused.session_id("proposal_missing"), None);
    let _ = fs::remove_dir_all(&data_dir);
}
