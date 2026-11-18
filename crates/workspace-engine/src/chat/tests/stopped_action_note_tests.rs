use super::stopped_action_note;

#[test]
fn the_note_names_the_stopped_action_and_says_its_outcome_is_unknown() {
    let note = stopped_action_note("run_command", "cargo test");
    assert!(note.contains("the command `cargo test`"), "{note}");
    assert!(note.contains("outcome is unknown"), "{note}");
    assert!(note.contains("did not retry"), "{note}");
    let hook = stopped_action_note("hook", "");
    assert!(hook.contains("`hook`"), "{hook}");
}
