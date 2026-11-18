use super::*;

fn command() -> ToolAction {
    ToolAction::Command(CommandRequest {
        command: "cargo test".to_string(),
        reason: "test".to_string(),
        working_directory: None,
    })
}

fn ask() -> ToolAction {
    ToolAction::AskUser(crate::clarification::AskUserInput {
        question: "Which one?".to_string(),
        options: vec!["The CLI".to_string(), "The shell".to_string()],
        allow_free_text: false,
    })
}

/// Criterion 5: `request_approval` is the only verdict that can add an
/// approval gate, and no verdict removes one (spec 32 `context.md` §4).
/// The exhaustive list is deliberate: a fifth verdict must decide here.
#[test]
fn only_request_approval_adds_a_gate_and_none_removes_one() {
    let verdicts = [
        Verdict::Allow,
        Verdict::Warn,
        Verdict::RequestApproval,
        Verdict::Deny,
    ];
    let mut requires_approval = true;
    for verdict in verdicts {
        let effect = before_tool_effect(&command(), verdict);
        requires_approval = requires_approval || effect == BeforeToolEffect::ForceApproval;
        assert!(
            requires_approval,
            "{verdict:?} removed an approval that was already required"
        );
        if verdict == Verdict::RequestApproval {
            assert_eq!(effect, BeforeToolEffect::ForceApproval);
        }
    }
}

/// Spec 50: `ask_user` has no approval step and its answer authorises
/// nothing, so a hook asking for approval refuses the question rather than
/// turning it into a card.
#[test]
fn request_approval_on_a_question_is_refused() {
    assert_eq!(
        before_tool_effect(&ask(), Verdict::RequestApproval),
        BeforeToolEffect::Refuse
    );
}

#[test]
fn request_approval_on_a_patch_is_already_satisfied() {
    let patch = ToolAction::ProposePatch(GeneratedEdit {
        summary: "Change".to_string(),
        changes: Vec::new(),
    });
    assert_eq!(
        before_tool_effect(&patch, Verdict::RequestApproval),
        BeforeToolEffect::SatisfiedApproval
    );
}
