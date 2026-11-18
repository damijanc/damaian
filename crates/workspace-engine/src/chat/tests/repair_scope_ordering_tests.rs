/// Spec 102 Task 5: the first persisted patch must already carry its
/// repair-scope review. Attaching it after the turn returns would leave a
/// crash window where recovery restores a repair card without the scope
/// fact the user is supposed to approve.
#[test]
fn a_repair_patch_is_stored_after_its_scope_review_is_attached() {
    let source = include_str!("../dispatch.rs");
    let body = |name: &str| {
        let start = format!("fn {name}(");
        source
            .split(start.as_str())
            .nth(1)
            .and_then(|rest| rest.split("\n    fn ").next())
            .unwrap_or_else(|| panic!("the body of {name}"))
    };
    let position = |name: &str, text: &str, needle: &str| {
        text.find(needle)
            .unwrap_or_else(|| panic!("{needle} in {name}"))
    };

    // Both tools check protected paths before handing the patch on, so the
    // stop below only ever sees a patch that passed.
    for name in ["handle_propose_patch", "handle_edit_file"] {
        let text = body(name);
        let protected = position(name, text, "ensure_patch_not_protected");
        let handoff = position(
            name,
            text,
            "self.stop_for_patch_review(cx, patch, action_marker)",
        );
        assert!(
            protected < handoff,
            "{name} must refuse a protected patch before stopping for review"
        );
    }

    let name = "stop_for_patch_review";
    let text = body(name);
    let review = position(name, text, "repair_scope_review_for_patch");
    let save = position(name, text, "self.patch_store.save(&patch)?");
    let response = position(name, text, "patch_proposal_response(&patch)");
    let card = position(name, text, "agent_patch_proposal(&patch)");
    let finish = position(
        name,
        text,
        "finish_action(action_marker, \"awaiting_review\")",
    );
    assert!(
        review < save && save < response && save < card && save < finish,
        "the patch stop must attach scope before its first save and every later step"
    );
}
