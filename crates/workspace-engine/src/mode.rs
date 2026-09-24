use crate::chat::ToolAction;
use crate::command_policy::{CommandClassification, CommandRisk};
use serde::{Deserialize, Serialize};

/// A session's working mode. Bounds what the model can do this turn,
/// structurally rather than by instruction — `mode_permits` is the single
/// place mode and tool identity are crossed; every enforcement layer calls
/// through it rather than re-implementing any part of the matrix
/// (`proposal.md` §5.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum SessionMode {
    Ask,
    Plan,
    Code,
    Review,
}

impl SessionMode {
    /// The wire form embedded in a hand-built session-log JSON payload,
    /// matching `CommandRisk::as_str()`'s convention (`command_policy.rs`).
    pub(crate) fn as_str(&self) -> &'static str {
        match self {
            Self::Ask => "ask",
            Self::Plan => "plan",
            Self::Code => "code",
            Self::Review => "review",
        }
    }

    /// The name a person reads, as the mode control labels it.
    pub(crate) fn label(&self) -> &'static str {
        match self {
            Self::Ask => "Ask",
            Self::Plan => "Plan",
            Self::Code => "Code",
            Self::Review => "Review",
        }
    }
}

/// The result of asking whether a mode permits an action. `Refused` names
/// both the mode that blocked it and the most permissive mode that would
/// allow it, so a refusal message never has to re-derive that pairing from
/// the matrix itself (`proposal.md` §5.6).
///
/// For `run_command`, `allowed_in` on a refusal is the most permissive mode
/// that allows *this command's classification* — not the most permissive
/// mode for `run_command` in general. A mutating command's `allowed_in` is
/// always `Code`, even though Plan would allow a read-only command; this is
/// the classification-dependent nature of that row, not an approximation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Permission {
    Allowed,
    Refused {
        blocked_by: SessionMode,
        allowed_in: SessionMode,
    },
}

impl Permission {
    pub(crate) fn is_allowed(&self) -> bool {
        matches!(self, Permission::Allowed)
    }
}

/// The one wording every Layer 3 refusal point uses (`proposal.md` §5.6:
/// "which mode blocked it and what mode would allow it"), so nine call sites
/// cannot drift into nine phrasings. Only ever called on a refusal — every
/// call site has just matched `Permission::Refused` — so an `Allowed` here is
/// a caller bug, and it panics rather than inventing a message for it.
pub(crate) fn refusal_message(refused: Permission) -> String {
    let Permission::Refused {
        blocked_by,
        allowed_in,
    } = refused
    else {
        unreachable!("refusal_message called on an allowed permission")
    };
    format!(
        "Refused: {} mode does not allow this. Switch to {} mode to allow it.",
        blocked_by.label(),
        allowed_in.label()
    )
}

/// The permission matrix from `proposal.md` §5.1, extended per
/// `context.md` §1 for the tools added since the flat spec was written. The
/// single place mode and tool identity are crossed — every other layer
/// (Layer 1's tool-list filtering, Layer 2's envelope omission, Layer 3's
/// per-action refusal) calls this rather than re-implementing any part of
/// it.
///
/// `command` must be `Some` for a `ToolAction::Command` — every call site
/// must classify the command before asking whether the mode permits it, so
/// this panics rather than silently defaulting when it is `None`. Every
/// other variant ignores `command`.
///
/// `mcp_tool_read_only` is the server's `annotations.readOnlyHint` for a
/// `ToolAction::McpCall`'s specific tool (`McpRuntime::tool_read_only_hint`).
/// Only `Some(true)` widens the call beyond Code — `Some(false)` and `None`
/// (no claim made) both stay mutation-class, the same "silence is not a
/// green light" posture spec 49 used for cache reporting (`context.md` §2).
/// Every other variant ignores it.
pub(crate) fn mode_permits(
    mode: SessionMode,
    action: &ToolAction,
    command: Option<&CommandClassification>,
    mcp_tool_read_only: Option<bool>,
) -> Permission {
    use SessionMode::*;

    match action {
        ToolAction::ReadFile { .. }
        | ToolAction::ListDirectory { .. }
        | ToolAction::SearchContent { .. }
        | ToolAction::SearchCodebase { .. }
        | ToolAction::ReadGitStatus
        | ToolAction::ReadGitDiff { .. } => Permission::Allowed,

        ToolAction::ProposePatch(_) | ToolAction::EditFile { .. } => {
            if mode == Code {
                Permission::Allowed
            } else {
                Permission::Refused {
                    blocked_by: mode,
                    allowed_in: Code,
                }
            }
        }

        ToolAction::ProposePlan(_) | ToolAction::CompleteStep => match mode {
            Plan | Code => Permission::Allowed,
            Ask | Review => Permission::Refused {
                blocked_by: mode,
                allowed_in: Plan,
            },
        },

        ToolAction::Command(_) => {
            let classification = command.expect(
                "mode_permits called with ToolAction::Command and no \
                 CommandClassification — every Command call site must \
                 classify the command before asking whether the mode \
                 permits it",
            );
            let read_only_no_approval =
                classification.risk == CommandRisk::Low && !classification.requires_approval;
            match mode {
                Ask => Permission::Refused {
                    blocked_by: Ask,
                    allowed_in: Plan,
                },
                Code => Permission::Allowed,
                Plan | Review if read_only_no_approval => Permission::Allowed,
                Plan | Review => Permission::Refused {
                    blocked_by: mode,
                    allowed_in: Code,
                },
            }
        }

        ToolAction::WebDiagnostic(_) => match mode {
            Code | Review => Permission::Allowed,
            Ask | Plan => Permission::Refused {
                blocked_by: mode,
                allowed_in: Code,
            },
        },

        ToolAction::McpCall { .. } => {
            if mcp_tool_read_only == Some(true) || mode == Code {
                Permission::Allowed
            } else {
                Permission::Refused {
                    blocked_by: mode,
                    allowed_in: Code,
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::chat::{CommandRequest, ToolAction};
    use crate::command_policy::{CommandClassification, CommandRisk};
    use crate::edit::GeneratedEdit;
    use crate::mode::{Permission, SessionMode, mode_permits, refusal_message};
    use crate::web_diagnostics::{WebDiagnosticCall, WebDiagnosticKind};

    fn read_only_command() -> CommandClassification {
        CommandClassification {
            command: "git status".to_string(),
            risk: CommandRisk::Low,
            blocked: false,
            requires_approval: false,
            reasons: vec![],
            expected_effects: String::new(),
            may_use_network: false,
        }
    }

    fn approval_required_command() -> CommandClassification {
        CommandClassification {
            command: "npm install".to_string(),
            risk: CommandRisk::Medium,
            blocked: false,
            requires_approval: true,
            reasons: vec![],
            expected_effects: String::new(),
            may_use_network: true,
        }
    }

    /// The work package's primary artifact per `proposal.md` §6: every
    /// mode crossed with every current tool class, asserting allowed or
    /// refused. Table source: `proposal.md` §5.1 plus `context.md` §1's
    /// extension for the five tools added since the flat spec was written.
    #[test]
    fn the_permission_matrix_matches_the_spec_table() {
        use SessionMode::*;
        let read_actions = [
            ToolAction::ReadFile {
                path: "x".into(),
                range: None,
            },
            ToolAction::ListDirectory {
                dir: None,
                depth: None,
            },
            ToolAction::SearchContent {
                pattern: "x".into(),
                path_glob: None,
                max_matches: None,
            },
            ToolAction::SearchCodebase {
                query: "x".into(),
                semantic: false,
                limit: 8,
            },
            ToolAction::ReadGitStatus,
            ToolAction::ReadGitDiff { staged: false },
        ];
        for action in &read_actions {
            for mode in [Ask, Plan, Code, Review] {
                assert!(
                    mode_permits(mode, action, None, None).is_allowed(),
                    "{mode:?} should permit {action:?}"
                );
            }
        }

        let mutation_actions = [
            ToolAction::ProposePatch(GeneratedEdit {
                summary: "x".into(),
                changes: vec![],
            }),
            ToolAction::EditFile {
                summary: "x".into(),
                edits: vec![],
            },
        ];
        for action in &mutation_actions {
            assert!(!mode_permits(Ask, action, None, None).is_allowed());
            assert!(!mode_permits(Plan, action, None, None).is_allowed());
            assert!(mode_permits(Code, action, None, None).is_allowed());
            assert!(!mode_permits(Review, action, None, None).is_allowed());
        }

        let planning_actions = [ToolAction::ProposePlan(vec![]), ToolAction::CompleteStep];
        for action in &planning_actions {
            assert!(!mode_permits(Ask, action, None, None).is_allowed());
            assert!(mode_permits(Plan, action, None, None).is_allowed());
            assert!(mode_permits(Code, action, None, None).is_allowed());
            assert!(!mode_permits(Review, action, None, None).is_allowed());
        }

        let web_action = ToolAction::WebDiagnostic(WebDiagnosticCall {
            kind: WebDiagnosticKind::Inspect,
            url: "http://localhost".into(),
            arguments_json: "{}".into(),
            session_id: None,
            task_id: None,
        });
        assert!(!mode_permits(Ask, &web_action, None, None).is_allowed());
        assert!(!mode_permits(Plan, &web_action, None, None).is_allowed());
        assert!(mode_permits(Code, &web_action, None, None).is_allowed());
        assert!(mode_permits(Review, &web_action, None, None).is_allowed());

        let mcp_action = ToolAction::McpCall {
            server_id: "sentry".into(),
            tool_name: "search_issues".into(),
            arguments_json: "{}".into(),
        };
        for mode in [Ask, Plan, Code, Review] {
            assert!(
                mode_permits(mode, &mcp_action, None, Some(true)).is_allowed(),
                "{mode:?} should permit an MCP call annotated read-only"
            );
        }
        for hint in [Some(false), None] {
            assert!(!mode_permits(Ask, &mcp_action, None, hint).is_allowed());
            assert!(!mode_permits(Plan, &mcp_action, None, hint).is_allowed());
            assert!(mode_permits(Code, &mcp_action, None, hint).is_allowed());
            assert!(!mode_permits(Review, &mcp_action, None, hint).is_allowed());
        }
    }

    /// Silence is not a green light (`context.md` §2, the same posture spec
    /// 49 used for cache reporting): a server that made no read-only claim
    /// stays mutation-class, not "assume safe."
    #[test]
    fn an_mcp_call_with_no_read_only_signal_is_treated_as_mutation_class() {
        let action = ToolAction::McpCall {
            server_id: "sentry".into(),
            tool_name: "search_issues".into(),
            arguments_json: "{}".into(),
        };
        assert!(!mode_permits(SessionMode::Ask, &action, None, None).is_allowed());
        assert!(!mode_permits(SessionMode::Plan, &action, None, None).is_allowed());
        assert!(!mode_permits(SessionMode::Review, &action, None, None).is_allowed());
        assert!(mode_permits(SessionMode::Code, &action, None, None).is_allowed());
    }

    #[test]
    fn ask_offers_no_commands_at_all_not_even_read_only_ones() {
        let action = ToolAction::Command(CommandRequest {
            command: "git status".into(),
            reason: String::new(),
        });
        assert!(
            !mode_permits(SessionMode::Ask, &action, Some(&read_only_command()), None).is_allowed()
        );
    }

    #[test]
    fn plan_permits_a_read_only_command_that_needs_no_approval() {
        let action = ToolAction::Command(CommandRequest {
            command: "git status".into(),
            reason: String::new(),
        });
        assert!(
            mode_permits(SessionMode::Plan, &action, Some(&read_only_command()), None).is_allowed()
        );
    }

    /// §5.3's sharpest case: a command that would need approval is refused
    /// outright in Plan, never turned into an approval card.
    #[test]
    fn plan_refuses_a_command_that_would_require_approval_even_if_low_risk() {
        let mut classification = read_only_command();
        classification.requires_approval = true;
        let action = ToolAction::Command(CommandRequest {
            command: "git status".into(),
            reason: String::new(),
        });
        assert!(
            !mode_permits(SessionMode::Plan, &action, Some(&classification), None).is_allowed()
        );
    }

    #[test]
    fn plan_refuses_a_mutating_command() {
        let action = ToolAction::Command(CommandRequest {
            command: "npm install".into(),
            reason: String::new(),
        });
        assert!(
            !mode_permits(
                SessionMode::Plan,
                &action,
                Some(&approval_required_command()),
                None
            )
            .is_allowed()
        );
    }

    #[test]
    fn code_permits_any_command_classification() {
        let action = ToolAction::Command(CommandRequest {
            command: "npm install".into(),
            reason: String::new(),
        });
        assert!(
            mode_permits(
                SessionMode::Code,
                &action,
                Some(&approval_required_command()),
                None
            )
            .is_allowed()
        );
    }

    #[test]
    fn review_permits_a_read_only_command_but_not_a_mutating_one() {
        let read = ToolAction::Command(CommandRequest {
            command: "git status".into(),
            reason: String::new(),
        });
        let mutate = ToolAction::Command(CommandRequest {
            command: "npm install".into(),
            reason: String::new(),
        });
        assert!(
            mode_permits(SessionMode::Review, &read, Some(&read_only_command()), None).is_allowed()
        );
        assert!(
            !mode_permits(
                SessionMode::Review,
                &mutate,
                Some(&approval_required_command()),
                None
            )
            .is_allowed()
        );
    }

    /// A refusal names both the mode that blocked the action and the mode
    /// that would allow it (`proposal.md` §5.6), so the UI never has to
    /// re-derive that pairing from the matrix itself.
    #[test]
    fn a_refusal_names_the_blocking_mode_and_the_permitting_mode() {
        let action = ToolAction::ProposePatch(GeneratedEdit {
            summary: "x".into(),
            changes: vec![],
        });
        let Permission::Refused {
            blocked_by,
            allowed_in,
        } = mode_permits(SessionMode::Ask, &action, None, None)
        else {
            panic!("expected a refusal");
        };
        assert_eq!(blocked_by, SessionMode::Ask);
        assert_eq!(allowed_in, SessionMode::Code);
    }

    #[test]
    fn a_refusal_message_names_both_modes_in_the_words_a_person_reads() {
        let message = refusal_message(Permission::Refused {
            blocked_by: SessionMode::Review,
            allowed_in: SessionMode::Code,
        });
        assert_eq!(
            message,
            "Refused: Review mode does not allow this. Switch to Code mode to allow it."
        );
    }

    /// Every call site has just matched a refusal, so an `Allowed` reaching
    /// the helper is a caller bug — loud, not a made-up message.
    #[test]
    fn a_refusal_message_for_an_allowed_permission_panics() {
        assert!(std::panic::catch_unwind(|| refusal_message(Permission::Allowed)).is_err());
    }

    /// `run_command` needs the command's own classification to decide,
    /// unlike every other tool — the compile-time proof that a caller
    /// cannot omit it for a `Command` action and get a silent default.
    #[test]
    fn a_command_action_without_a_classification_panics_rather_than_defaulting() {
        let action = ToolAction::Command(CommandRequest {
            command: "git status".into(),
            reason: String::new(),
        });
        let result =
            std::panic::catch_unwind(|| mode_permits(SessionMode::Plan, &action, None, None));
        assert!(result.is_err());
    }
}
