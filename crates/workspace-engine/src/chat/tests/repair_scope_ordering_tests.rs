/// Spec 102 Task 5: the first persisted patch must already carry its
/// repair-scope review. Attaching it after the turn returns would leave a
/// crash window where recovery restores a repair card without the scope
/// fact the user is supposed to approve.
#[test]
fn a_repair_patch_is_stored_after_its_scope_review_is_attached() {
    // Both tool branches this test reads live in `run_agentic_turn`.
    let source = include_str!("../agentic_turn.rs");
    for (name, start, end) in [
        (
            "propose_patch",
            "ToolAction::ProposePatch(generated_edit) => {",
            "ToolAction::ProposePlan(steps) => {",
        ),
        (
            "edit_file",
            "ToolAction::EditFile { summary, edits } => {",
            "ToolAction::FetchUrl { url } => {",
        ),
    ] {
        let branch = source
            .split(start)
            .nth(1)
            .and_then(|rest| rest.split(end).next())
            .unwrap_or_else(|| panic!("the {name} branch"));
        let position = |needle: &str| {
            branch
                .find(needle)
                .unwrap_or_else(|| panic!("{needle} in the {name} branch"))
        };
        let protected = position("ensure_patch_not_protected");
        let review = position("repair_scope_review_for_patch");
        let save = position("self.patch_store.save(&patch)?");
        let response = position("patch_proposal_response(&patch)");
        let card = position("agent_patch_proposal(&patch)");
        let finish = position("finish_action(action_marker, \"awaiting_review\")");
        assert!(
            protected < review && review < save && save < response && save < card && save < finish,
            "the {name} branch must attach scope before its first save and every later step"
        );
    }
}
