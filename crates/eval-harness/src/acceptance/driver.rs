//! The acceptance driver (spec 106 `context.md` D2, D5, D6).
//!
//! The driver answers only the decisions the UI would present, through a
//! [`Transport`] that both tiers implement: the deterministic tier over the
//! in-process worker host, the live tier over the real worker's socket.
//!
//! The forbidden interventions — **Continue task**, **Continue repair**, a
//! follow-up message, a note, a round-budget "Continue", a task reset, editing
//! the fixture or running Git in it — are not methods of [`Transport`]. The
//! driver cannot send them. When the worker reaches a state that only a
//! forbidden intervention could move, the run ends with
//! [`FailureReason::NeedsForbiddenIntervention`] naming it.

use std::path::{Path, PathBuf};

use super::policy::{CommandDecision, PatchDecision, Policy};
use workspace_engine::Result;

/// What the worker is waiting on, as both transports report it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Pending {
    /// Busy; nothing to decide yet. The transport waits before reporting
    /// this again.
    Working,
    PlanReview {
        proposal_id: String,
        step_titles: Vec<String>,
    },
    Command {
        proposal_id: String,
        command: String,
        working_directory: String,
    },
    /// Selected checks waiting for approval (a check batch).
    Checks {
        batch_id: String,
        checks: Vec<PendingCheck>,
    },
    Question {
        question_id: String,
        question: String,
        options: Vec<String>,
        allow_free_text: bool,
    },
    /// A proposed patch. The worker is idle until it is applied or rejected.
    Patch {
        patch_id: String,
        paths: Vec<String>,
    },
    /// The round-budget pause. Approving it is the generic "Continue", which
    /// spec 106 forbids.
    RoundBudget { proposal_id: String },
    /// The task ended with this status (`complete`, `incomplete`, `failed`,
    /// a bounded stop …).
    Ended { status: String },
    /// A decision the acceptance does not know how to answer (for example a
    /// provider-fallback card). The run stops rather than guess.
    Unrecognised { detail: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingCheck {
    pub id: String,
    pub command: String,
    pub working_directory: String,
}

/// The pre-apply baseline gate's answer to an apply request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApplyStep {
    /// The patch is applied and its verification has been requested, as the
    /// desktop shell does after a patch settles.
    Applied,
    /// Baselines must be decided before the patch can apply.
    Baselines {
        batch_id: String,
        checks: Vec<PendingCheck>,
    },
}

/// The allowed decisions, and nothing else (requirement 7, D5).
pub trait Transport {
    /// The fixture repository the worker works in.
    fn repository_root(&self) -> &Path;
    /// The next thing to decide, or [`Pending::Ended`].
    fn next(&mut self) -> Result<Pending>;
    /// Approve the reviewed plan with the budget the engine proposed.
    fn approve_plan(&mut self, proposal_id: &str) -> Result<()>;
    fn decide_command(&mut self, proposal_id: &str, approve: bool) -> Result<()>;
    fn decide_checks(&mut self, batch_id: &str, decisions: &[(String, bool)]) -> Result<()>;
    /// Answer a question with text, or with one of its options.
    fn answer_question(
        &mut self,
        question_id: &str,
        answer: &str,
        option: Option<usize>,
    ) -> Result<()>;
    /// Apply a proposed patch the way the desktop shell does, then request its
    /// verification.
    fn apply_patch(&mut self, patch_id: &str) -> Result<ApplyStep>;
    fn decide_baselines(&mut self, batch_id: &str, decisions: &[(String, bool)]) -> Result<()>;
    fn reject_patch(&mut self, patch_id: &str) -> Result<()>;
}

/// One decision the driver made, in order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecisionRecord {
    pub sequence: usize,
    pub kind: DecisionKind,
    pub subject: String,
    pub outcome: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum DecisionKind {
    PlanReview,
    Command,
    Checks,
    Baselines,
    Question,
    Patch,
}

impl DecisionKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::PlanReview => "plan_review",
            Self::Command => "command",
            Self::Checks => "checks",
            Self::Baselines => "baselines",
            Self::Question => "question",
            Self::Patch => "patch",
        }
    }
}

/// Something unusual the driver answered anyway (D6), reported to the main
/// session (D9).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Deviation {
    pub sequence: usize,
    pub kind: DecisionKind,
    pub detail: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FailureReason {
    /// Requirement 3.
    ProtectedPathProposed(String),
    /// Requirement 7: the worker stopped where only a forbidden intervention
    /// could move it on.
    NeedsForbiddenIntervention(String),
    /// The decision cap was reached; the worker did not finish.
    TooManyDecisions(usize),
    /// The worker asked for a decision the acceptance cannot answer.
    UnrecognisedDecision(String),
    /// The transport failed.
    Transport(String),
}

impl FailureReason {
    pub fn describe(&self) -> String {
        match self {
            Self::ProtectedPathProposed(detail) => format!("protected path proposed: {detail}"),
            Self::NeedsForbiddenIntervention(detail) => {
                format!("would need a forbidden intervention: {detail}")
            }
            Self::TooManyDecisions(count) => {
                format!("stopped after {count} decisions without the task ending")
            }
            Self::UnrecognisedDecision(detail) => {
                format!("the worker asked for a decision the acceptance cannot answer: {detail}")
            }
            Self::Transport(detail) => format!("transport failed: {detail}"),
        }
    }
}

/// The driver's own account of a run. Success is judged separately, from
/// durable records ([`super::criteria`]).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct DriveOutcome {
    pub decisions: Vec<DecisionRecord>,
    pub deviations: Vec<Deviation>,
    /// Question ids the driver answered as the material clarification.
    pub clarification_question_ids: Vec<String>,
    pub ended_status: Option<String>,
    pub failure: Option<FailureReason>,
    pub repository_root: PathBuf,
}

impl DriveOutcome {
    pub fn count(&self, kind: DecisionKind) -> usize {
        self.decisions
            .iter()
            .filter(|decision| decision.kind == kind)
            .count()
    }
}

/// How many decisions a run may take before the driver gives up.
pub const DEFAULT_MAX_DECISIONS: usize = 400;

/// Drive one task to its end.
pub fn drive(transport: &mut dyn Transport, policy: &Policy, max_decisions: usize) -> DriveOutcome {
    let mut outcome = DriveOutcome {
        repository_root: transport.repository_root().to_path_buf(),
        ..DriveOutcome::default()
    };
    if let Err(error) = drive_inner(transport, policy, max_decisions, &mut outcome) {
        outcome.failure = Some(FailureReason::Transport(error.to_string()));
    }
    outcome
}

fn drive_inner(
    transport: &mut dyn Transport,
    policy: &Policy,
    max_decisions: usize,
    outcome: &mut DriveOutcome,
) -> Result<()> {
    loop {
        if outcome.decisions.len() >= max_decisions {
            outcome.failure = Some(FailureReason::TooManyDecisions(outcome.decisions.len()));
            return Ok(());
        }
        let sequence = outcome.decisions.len() + 1;
        match transport.next()? {
            Pending::Working => continue,
            Pending::Ended { status } => {
                if status != "complete" {
                    // An ended, unfinished task moves on only through
                    // Continue task / Continue repair, which are forbidden.
                    outcome.failure = Some(FailureReason::NeedsForbiddenIntervention(format!(
                        "the task ended `{status}`; only Continue task or Continue repair could resume it"
                    )));
                }
                outcome.ended_status = Some(status);
                return Ok(());
            }
            Pending::Unrecognised { detail } => {
                outcome.failure = Some(FailureReason::UnrecognisedDecision(detail));
                return Ok(());
            }
            Pending::RoundBudget { proposal_id } => {
                outcome.failure = Some(FailureReason::NeedsForbiddenIntervention(format!(
                    "round-budget pause {proposal_id}: approving it is the generic Continue"
                )));
                return Ok(());
            }
            Pending::PlanReview {
                proposal_id,
                step_titles,
            } => {
                transport.approve_plan(&proposal_id)?;
                record(
                    outcome,
                    sequence,
                    DecisionKind::PlanReview,
                    format!("{} steps", step_titles.len()),
                    "approved with the proposed budget",
                );
            }
            Pending::Command {
                proposal_id,
                command,
                working_directory: _,
            } => match policy.decide_command(&command, transport.repository_root()) {
                CommandDecision::Approve => {
                    transport.decide_command(&proposal_id, true)?;
                    record(
                        outcome,
                        sequence,
                        DecisionKind::Command,
                        command,
                        "approved",
                    );
                }
                CommandDecision::ApproveAsDeviation(detail) => {
                    transport.decide_command(&proposal_id, true)?;
                    record(
                        outcome,
                        sequence,
                        DecisionKind::Command,
                        command,
                        "approved",
                    );
                    deviate(outcome, sequence, DecisionKind::Command, detail);
                }
                CommandDecision::Reject(detail) => {
                    transport.decide_command(&proposal_id, false)?;
                    record(
                        outcome,
                        sequence,
                        DecisionKind::Command,
                        command,
                        "rejected",
                    );
                    deviate(outcome, sequence, DecisionKind::Command, detail);
                }
            },
            Pending::Checks { batch_id, checks } => {
                let decisions = approve_all(&checks);
                transport.decide_checks(&batch_id, &decisions)?;
                record(
                    outcome,
                    sequence,
                    DecisionKind::Checks,
                    describe_checks(&checks),
                    "approved",
                );
            }
            Pending::Question {
                question_id,
                question,
                options,
                allow_free_text,
            } => {
                let decision = policy.answer_question(&question, &options, allow_free_text);
                transport.answer_question(&question_id, &decision.answer, decision.option)?;
                if let Some(note) = &decision.note {
                    deviate(outcome, sequence, DecisionKind::Question, note.clone());
                }
                record(
                    outcome,
                    sequence,
                    DecisionKind::Question,
                    question.clone(),
                    decision.answer.clone(),
                );
                if decision.is_clarification {
                    outcome.clarification_question_ids.push(question_id);
                } else {
                    deviate(
                        outcome,
                        sequence,
                        DecisionKind::Question,
                        format!(
                            "unexpected question \"{question}\", answered \"{}\"",
                            decision.answer
                        ),
                    );
                }
            }
            Pending::Patch { patch_id, paths } => match policy.decide_patch(&paths) {
                PatchDecision::Fail(detail) => {
                    transport.reject_patch(&patch_id)?;
                    record(
                        outcome,
                        sequence,
                        DecisionKind::Patch,
                        paths.join(", "),
                        "rejected",
                    );
                    outcome.failure = Some(FailureReason::ProtectedPathProposed(detail));
                    return Ok(());
                }
                PatchDecision::Reject(detail) => {
                    transport.reject_patch(&patch_id)?;
                    record(
                        outcome,
                        sequence,
                        DecisionKind::Patch,
                        paths.join(", "),
                        "rejected",
                    );
                    deviate(outcome, sequence, DecisionKind::Patch, detail);
                }
                PatchDecision::Apply => {
                    apply_with_baselines(transport, outcome, &patch_id)?;
                    record(
                        outcome,
                        sequence,
                        DecisionKind::Patch,
                        paths.join(", "),
                        "applied",
                    );
                }
            },
        }
    }
}

/// Apply a patch, answering every baseline card the pre-apply gate raises
/// (D6: checks are approved).
fn apply_with_baselines(
    transport: &mut dyn Transport,
    outcome: &mut DriveOutcome,
    patch_id: &str,
) -> Result<()> {
    loop {
        match transport.apply_patch(patch_id)? {
            ApplyStep::Applied => return Ok(()),
            ApplyStep::Baselines { batch_id, checks } => {
                let decisions = approve_all(&checks);
                transport.decide_baselines(&batch_id, &decisions)?;
                let sequence = outcome.decisions.len() + 1;
                record(
                    outcome,
                    sequence,
                    DecisionKind::Baselines,
                    describe_checks(&checks),
                    "approved",
                );
            }
        }
    }
}

fn approve_all(checks: &[PendingCheck]) -> Vec<(String, bool)> {
    checks
        .iter()
        .map(|check| (check.id.clone(), true))
        .collect()
}

fn describe_checks(checks: &[PendingCheck]) -> String {
    checks
        .iter()
        .map(|check| {
            if check.working_directory.is_empty() {
                check.command.clone()
            } else {
                format!("{} ({})", check.command, check.working_directory)
            }
        })
        .collect::<Vec<_>>()
        .join(", ")
}

fn record(
    outcome: &mut DriveOutcome,
    sequence: usize,
    kind: DecisionKind,
    subject: impl Into<String>,
    result: impl Into<String>,
) {
    outcome.decisions.push(DecisionRecord {
        sequence,
        kind,
        subject: subject.into(),
        outcome: result.into(),
    });
}

fn deviate(outcome: &mut DriveOutcome, sequence: usize, kind: DecisionKind, detail: String) {
    outcome.deviations.push(Deviation {
        sequence,
        kind,
        detail,
    });
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;

    use super::*;
    use crate::acceptance::AcceptanceFixture;

    /// A scripted transport: hands out `script` in order and records every
    /// call the driver makes.
    struct FakeTransport {
        root: PathBuf,
        script: VecDeque<Pending>,
        apply_steps: VecDeque<ApplyStep>,
        calls: Vec<String>,
    }

    impl FakeTransport {
        fn new(script: Vec<Pending>) -> Self {
            Self {
                root: PathBuf::from("/tmp/damaian-eval-fixture"),
                script: script.into(),
                apply_steps: VecDeque::new(),
                calls: Vec::new(),
            }
        }
    }

    impl Transport for FakeTransport {
        fn repository_root(&self) -> &Path {
            &self.root
        }
        fn next(&mut self) -> Result<Pending> {
            Ok(self.script.pop_front().unwrap_or(Pending::Ended {
                status: "complete".to_string(),
            }))
        }
        fn approve_plan(&mut self, proposal_id: &str) -> Result<()> {
            self.calls.push(format!("approve_plan {proposal_id}"));
            Ok(())
        }
        fn decide_command(&mut self, proposal_id: &str, approve: bool) -> Result<()> {
            self.calls
                .push(format!("decide_command {proposal_id} {approve}"));
            Ok(())
        }
        fn decide_checks(&mut self, batch_id: &str, decisions: &[(String, bool)]) -> Result<()> {
            self.calls
                .push(format!("decide_checks {batch_id} {}", decisions.len()));
            Ok(())
        }
        fn answer_question(
            &mut self,
            question_id: &str,
            answer: &str,
            _option: Option<usize>,
        ) -> Result<()> {
            self.calls.push(format!("answer {question_id} {answer}"));
            Ok(())
        }
        fn apply_patch(&mut self, patch_id: &str) -> Result<ApplyStep> {
            self.calls.push(format!("apply {patch_id}"));
            Ok(self.apply_steps.pop_front().unwrap_or(ApplyStep::Applied))
        }
        fn decide_baselines(&mut self, batch_id: &str, decisions: &[(String, bool)]) -> Result<()> {
            self.calls
                .push(format!("decide_baselines {batch_id} {}", decisions.len()));
            Ok(())
        }
        fn reject_patch(&mut self, patch_id: &str) -> Result<()> {
            self.calls.push(format!("reject {patch_id}"));
            Ok(())
        }
    }

    fn policy() -> Policy {
        Policy::new(AcceptanceFixture::load("single_worker_spec").expect("acceptance data"))
    }

    fn check(id: &str, command: &str) -> PendingCheck {
        PendingCheck {
            id: id.to_string(),
            command: command.to_string(),
            working_directory: String::new(),
        }
    }

    #[test]
    fn a_clean_run_answers_every_allowed_decision_and_ends_complete() {
        let mut transport = FakeTransport::new(vec![
            Pending::Working,
            Pending::PlanReview {
                proposal_id: "plan_1".to_string(),
                step_titles: vec!["Change pricing".to_string(), "Update invoice".to_string()],
            },
            Pending::Question {
                question_id: "q_1".to_string(),
                question: "How should a tie such as 2.345 round?".to_string(),
                options: Vec::new(),
                allow_free_text: true,
            },
            Pending::Command {
                proposal_id: "cmd_1".to_string(),
                command: "cargo test -p pricing -p invoice".to_string(),
                working_directory: String::new(),
            },
            Pending::Patch {
                patch_id: "patch_1".to_string(),
                paths: vec!["crates/pricing/src/lib.rs".to_string()],
            },
            Pending::Checks {
                batch_id: "batch_1".to_string(),
                checks: vec![check("c1", "cargo test")],
            },
            Pending::Ended {
                status: "complete".to_string(),
            },
        ]);
        transport.apply_steps.push_back(ApplyStep::Baselines {
            batch_id: "base_1".to_string(),
            checks: vec![check("b1", "cargo test"), check("b2", "cargo test")],
        });
        let outcome = drive(&mut transport, &policy(), DEFAULT_MAX_DECISIONS);

        assert_eq!(outcome.failure, None, "{outcome:?}");
        assert_eq!(outcome.ended_status.as_deref(), Some("complete"));
        assert!(outcome.deviations.is_empty(), "{:?}", outcome.deviations);
        assert_eq!(outcome.clarification_question_ids, vec!["q_1".to_string()]);
        assert_eq!(
            transport.calls,
            vec![
                "approve_plan plan_1".to_string(),
                format!("answer q_1 {}", policy().acceptance().clarification.answer),
                "decide_command cmd_1 true".to_string(),
                "apply patch_1".to_string(),
                "decide_baselines base_1 2".to_string(),
                "apply patch_1".to_string(),
                "decide_checks batch_1 1".to_string(),
            ]
        );
        assert_eq!(outcome.count(DecisionKind::Baselines), 1);
        assert_eq!(outcome.count(DecisionKind::Patch), 1);
    }

    #[test]
    fn a_round_budget_pause_fails_the_run_without_continuing() {
        let mut transport = FakeTransport::new(vec![Pending::RoundBudget {
            proposal_id: "round_1".to_string(),
        }]);
        let outcome = drive(&mut transport, &policy(), DEFAULT_MAX_DECISIONS);

        assert!(matches!(
            outcome.failure,
            Some(FailureReason::NeedsForbiddenIntervention(_))
        ));
        assert!(
            transport.calls.is_empty(),
            "nothing may be sent to a round-budget pause: {:?}",
            transport.calls
        );
    }

    #[test]
    fn an_incomplete_task_fails_the_run_instead_of_continuing_it() {
        let mut transport = FakeTransport::new(vec![Pending::Ended {
            status: "incomplete".to_string(),
        }]);
        let outcome = drive(&mut transport, &policy(), DEFAULT_MAX_DECISIONS);

        assert_eq!(outcome.ended_status.as_deref(), Some("incomplete"));
        match outcome.failure {
            Some(FailureReason::NeedsForbiddenIntervention(detail)) => {
                assert!(detail.contains("Continue task"), "{detail}");
            }
            other => panic!("expected a forbidden-intervention failure: {other:?}"),
        }
        assert!(transport.calls.is_empty());
    }

    #[test]
    fn a_protected_path_patch_is_rejected_and_fails_the_run() {
        let mut transport = FakeTransport::new(vec![
            Pending::Patch {
                patch_id: "patch_bad".to_string(),
                paths: vec!["crates/legacy/src/lib.rs".to_string()],
            },
            Pending::Ended {
                status: "complete".to_string(),
            },
        ]);
        let outcome = drive(&mut transport, &policy(), DEFAULT_MAX_DECISIONS);

        assert!(matches!(
            outcome.failure,
            Some(FailureReason::ProtectedPathProposed(_))
        ));
        assert_eq!(transport.calls, vec!["reject patch_bad".to_string()]);
        assert_eq!(
            outcome.ended_status, None,
            "the run stops at the protected proposal"
        );
    }

    #[test]
    fn unusual_decisions_are_answered_and_recorded_without_failing() {
        let mut transport = FakeTransport::new(vec![
            Pending::Question {
                question_id: "q_extra".to_string(),
                question: "Should I add a changelog entry?".to_string(),
                options: Vec::new(),
                allow_free_text: true,
            },
            Pending::Command {
                proposal_id: "cmd_odd".to_string(),
                command: "python3 tools/report.py".to_string(),
                working_directory: String::new(),
            },
            Pending::Command {
                proposal_id: "cmd_net".to_string(),
                command: "curl https://example.com".to_string(),
                working_directory: String::new(),
            },
            Pending::Patch {
                patch_id: "patch_out".to_string(),
                paths: vec!["../outside.txt".to_string()],
            },
        ]);
        let outcome = drive(&mut transport, &policy(), DEFAULT_MAX_DECISIONS);

        assert_eq!(outcome.failure, None, "{outcome:?}");
        assert_eq!(outcome.ended_status.as_deref(), Some("complete"));
        assert_eq!(outcome.deviations.len(), 4, "{:?}", outcome.deviations);
        assert!(outcome.clarification_question_ids.is_empty());
        assert!(
            transport
                .calls
                .contains(&"decide_command cmd_odd true".to_string())
        );
        assert!(
            transport
                .calls
                .contains(&"decide_command cmd_net false".to_string())
        );
        assert!(transport.calls.contains(&"reject patch_out".to_string()));
        assert!(
            transport
                .calls
                .iter()
                .any(|call| call.starts_with("answer q_extra No further preference"))
        );
    }

    #[test]
    fn a_worker_that_never_ends_stops_at_the_decision_cap() {
        let script = (0..10)
            .map(|index| Pending::Command {
                proposal_id: format!("cmd_{index}"),
                command: "cargo test".to_string(),
                working_directory: String::new(),
            })
            .collect();
        let mut transport = FakeTransport::new(script);
        let outcome = drive(&mut transport, &policy(), 3);

        assert_eq!(outcome.failure, Some(FailureReason::TooManyDecisions(3)));
    }
}
