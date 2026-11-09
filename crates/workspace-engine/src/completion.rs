//! The completion gate: whether a task's work is actually closed.
//!
//! See `docs/specs/101_closed_task_completion.md`. A model answer is not proof
//! that its task is finished, so the verdict is computed from durable state
//! instead: the active plan, the folded verification events, a pending
//! approval or question, and the latest known disposition of every selected
//! check. The result is a typed [`CompletionGateReport`] with stable reason
//! codes and engine-owned explanations; adapters transport or render it and
//! never reconstruct what it means.
//!
//! Two boundaries stay outside this module. Spec 102 decides whether a failed
//! check was *caused* by the task, and spec 105 derives required checks from
//! the approved plan's structured [`crate::plan::PlannedCheck`]s. Here a check
//! is required because the plan marks it `required: true`, and a failure is
//! reported as a failure without assigning provenance.
//!
//! The only waiver is an explicit user decision: a check declined on its card
//! (`SkipReason::DeclinedByUser`), or the user declining the exact command a
//! deferred required check would run (`Evidence::CommandDeclined`, spec 105
//! review fix R8). A waiver is named in [`CompletionWaiver`] and is still never
//! represented as passed. A policy refusal, a question answer, a model
//! statement, a timeout or an absent result is never a waiver.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::clarification::PendingDecision;
use crate::error::{ClientError, Result};
use crate::hash::{file_hash, repository_id_for_root, sha256};
use crate::plan::{Evidence, PlanStep, StepStatus, TaskPlan};
use crate::repository_map::load_stored_map;
use crate::session::{Session, SessionStore, Task, TaskStatus};
use crate::verification::{
    CheckBaseline, CheckId, CheckOutcome, CheckRun, FailureProvenance, SkipReason,
    VerificationState, floor_checks, fold_verification,
};

/// A plan step whose id starts with this is the verification loop's own step,
/// not work the task promised. The gate requires the promised steps to be
/// terminal; a `verify_` step is accounted for by the verification events.
const VERIFICATION_STEP_PREFIX: &str = "verify_";

/// The durable action a settlement records before writing its status.
///
/// It is deliberately not side-effecting: a crash while a status is being
/// written has no external consequence, and recovery only uses the marker to
/// find a settlement whose verdict may not have landed. The reference names the
/// repository root so recovery can recompute the gate against the checkout that
/// produced it.
pub const COMPLETION_SETTLEMENT_ACTION: &str = "completion_settlement";

/// Why the completion gate did not consider a task closed (spec 101 §4).
///
/// The names are stable: a report written today is read by a later build, and
/// adapters branch on the code rather than on the explanation sentence.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CompletionReasonCode {
    /// At least one non-verification plan step is still pending or in progress.
    OpenPlanSteps,
    /// At least one non-verification plan step is blocked by a failed command.
    ///
    /// `Blocked` is terminal for a step (`StepStatus::is_terminal`), because the
    /// step's own outcome is settled and a plan revision must not delete it.
    /// It is not finished work, though: the command the step ran did not
    /// succeed, so the task cannot be closed. It gets its own code rather than
    /// joining [`Self::OpenPlanSteps`], whose set is [`TaskPlan::open_steps`]
    /// and the #66 reminder's set (spec 66 §5.1 deliberately keeps blocked
    /// steps out of the reminder: the model was already told the step blocked).
    BlockedPlanSteps,
    /// A verification is due and has not finished.
    VerificationDue,
    /// A verification run started but recorded no finished event.
    VerificationUnfinished,
    /// A proposal or question is still waiting for the user.
    PendingDecision,
    /// A required check has no recorded result at all.
    RequiredCheckMissing,
    /// A required check ran and did not pass.
    RequiredCheckFailed,
    /// A required check ran, but the files it covered have changed since.
    RequiredCheckStale,
    /// A required check was skipped for a reason other than an explicit user
    /// decline, so it does not verify the current state.
    RequiredCheckSkipped,
}

impl CompletionReasonCode {
    /// The wire name, matching the serde representation.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::OpenPlanSteps => "open_plan_steps",
            Self::BlockedPlanSteps => "blocked_plan_steps",
            Self::VerificationDue => "verification_due",
            Self::VerificationUnfinished => "verification_unfinished",
            Self::PendingDecision => "pending_decision",
            Self::RequiredCheckMissing => "required_check_missing",
            Self::RequiredCheckFailed => "required_check_failed",
            Self::RequiredCheckStale => "required_check_stale",
            Self::RequiredCheckSkipped => "required_check_skipped",
        }
    }
}

/// One blocking fact, with the sentence a user should see.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CompletionReason {
    pub code: CompletionReasonCode,
    pub explanation: String,
}

/// A required check the user explicitly declined.
///
/// A waiver is a named exception, not a pass: it says the check did not run and
/// that the user chose that. Only an explicit user decline produces one:
/// [`SkipReason::DeclinedByUser`] on a check card, or a matching
/// [`Evidence::CommandDeclined`] for a deferred check run as an ordinary
/// command (spec 105 review fix R8).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CompletionWaiver {
    pub check: CheckId,
    pub explanation: String,
}

/// A required check that failed only with the failure the task started with
/// (spec 106 requirement 11, context D1).
///
/// The engine, not the user, accepts it: the latest run is classified
/// `pre_existing` or `unchanged_failure` against a real task-start `Failed`
/// baseline, and nothing in it was introduced. It is never a pass and never a
/// waiver; it is listed so the user sees exactly which known failure the task
/// completed beside.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CompletionAcceptance {
    pub check: CheckId,
    pub provenance: FailureProvenance,
    pub explanation: String,
}

/// The engine's verdict on whether a task is closed (spec 101 §4).
///
/// `eligible` is the whole verdict. `reasons` explains every fact that kept it
/// false, `waivers` names declined checks the user has already accepted, and
/// `accepted_failures` names required checks that still fail only as they did
/// before the task started. All are computed here so a shell, CLI or worker
/// never has to interpret a plan or a verification report itself.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CompletionGateReport {
    pub task_id: String,
    pub eligible: bool,
    pub reasons: Vec<CompletionReason>,
    pub waivers: Vec<CompletionWaiver>,
    /// Absent from every report written before spec 106; reads as empty.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub accepted_failures: Vec<CompletionAcceptance>,
}

/// Which status the shared settlement operation is allowed to write.
///
/// Completion is never chosen by an adapter. `Gate` is the answered-turn and
/// successful-verification path, where the report alone decides between
/// [`TaskStatus::Complete`] and [`TaskStatus::Incomplete`]. `VerificationFailed`
/// keeps the verification's own `failed` status while still recording the
/// report, so a failed run cannot bypass the gate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CompletionSettlement {
    Gate,
    VerificationFailed,
}

/// The durable reference a completion-settlement marker carries.
///
/// The reference is written before the status event and read back by recovery
/// after a crash. It holds both facts recovery needs: the repository root to
/// recompute the gate against, and the typed settlement kind, so a crashed
/// failed verification is settled `failed` again rather than reinterpreted as
/// an answered turn. Recovery never reads an `eligible` boolean from it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CompletionSettlementMarker {
    root: PathBuf,
    settlement: CompletionSettlement,
}

impl CompletionSettlementMarker {
    pub fn new(root: impl Into<PathBuf>, settlement: CompletionSettlement) -> Self {
        Self {
            root: root.into(),
            settlement,
        }
    }

    /// The JSON reference stored on the `action_started` event.
    pub fn reference(&self) -> Result<String> {
        serde_json::to_string(self).map_err(|error| {
            ClientError::InvalidInput(format!(
                "Completion settlement marker could not be written: {error}"
            ))
        })
    }

    fn parse(reference: &str) -> Result<Self> {
        serde_json::from_str(reference).map_err(|error| {
            ClientError::InvalidInput(format!(
                "Completion settlement marker is not readable: {error}"
            ))
        })
    }
}

/// A task after completion settlement, with the report it was settled under.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SettledCompletion {
    pub task: Task,
    pub report: CompletionGateReport,
}

/// The durable facts [`CompletionGateReport::evaluate`] reads.
///
/// Kept as a plain struct so the reason matrix is testable without a session,
/// a model or a filesystem. [`CompletionGateService`] is what assembles these
/// facts from the session log and the checkout.
#[derive(Debug, Clone, Copy)]
pub struct CompletionFacts<'a> {
    pub task_id: &'a str,
    /// The active plan. `None` means the task carries no plan, which is not
    /// itself a blocker: a task with no planned steps can still close.
    pub plan: Option<&'a TaskPlan>,
    pub verification: &'a VerificationState,
    pub pending: Option<&'a PendingDecision>,
    /// Content hashes of every file a check run recorded, as the checkout holds
    /// them now. A missing entry means the file was deleted, which is stale.
    pub current_hashes: &'a BTreeMap<String, String>,
    /// The discovered-check floor: the detected checks of the roots the task
    /// touched (spec 101/105 review fix R15). It is added to the plan's
    /// required checks and merged by [`CheckId`]; it never replaces them.
    pub floor_checks: &'a [CheckId],
}

impl CompletionGateReport {
    /// Compute the verdict from durable facts.
    pub fn evaluate(facts: CompletionFacts<'_>) -> Self {
        let empty_plan = TaskPlan::new(facts.task_id, 0);
        let plan = facts.plan.unwrap_or(&empty_plan);
        // Required checks are sourced from the approved plan (spec 105 §5),
        // not from `verification.selected`, so currentness is decided here from
        // the plan's mutation-batch generation and the checkout's content.
        let current_generation = plan.mutation_generation();

        let mut reasons = Vec::new();
        let mut waivers = Vec::new();
        let mut accepted_failures = Vec::new();

        // `open_steps` is the reminder's set: `Pending` or `InProgress`
        // (spec 66 §5.1). Using it here keeps the gate's `OpenPlanSteps` and
        // the #66 reminder describing the same steps; a `Blocked` step is
        // reported separately below because its outcome is settled but the
        // work is not finished.
        let open: Vec<&PlanStep> = plan
            .open_steps()
            .into_iter()
            .filter(|step| !step.id.starts_with(VERIFICATION_STEP_PREFIX))
            .collect();
        if !open.is_empty() {
            reasons.push(CompletionReason {
                code: CompletionReasonCode::OpenPlanSteps,
                explanation: open_steps_explanation(&open),
            });
        }
        let blocked: Vec<&PlanStep> = plan
            .steps
            .iter()
            .filter(|step| {
                !step.id.starts_with(VERIFICATION_STEP_PREFIX) && step.status == StepStatus::Blocked
            })
            .collect();
        if !blocked.is_empty() {
            reasons.push(CompletionReason {
                code: CompletionReasonCode::BlockedPlanSteps,
                explanation: blocked_steps_explanation(&blocked),
            });
        }

        if facts.verification.due.is_some() {
            reasons.push(CompletionReason {
                code: CompletionReasonCode::VerificationDue,
                explanation: "Verification is due and has not run.".to_string(),
            });
        }
        if facts.verification.in_progress {
            reasons.push(CompletionReason {
                code: CompletionReasonCode::VerificationUnfinished,
                explanation: "A verification run started but has not finished.".to_string(),
            });
        }

        if let Some(pending) = facts.pending {
            let explanation = match pending {
                PendingDecision::Approval(_) => {
                    "A proposal is still waiting for your approval.".to_string()
                }
                PendingDecision::Question(_) => {
                    "A question is still waiting for your answer.".to_string()
                }
            };
            reasons.push(CompletionReason {
                code: CompletionReasonCode::PendingDecision,
                explanation,
            });
        }

        // The gate's required set is the plan's required checks plus the
        // discovered-check floor (spec 101/105 review fix R15, decision D1).
        // They are merged by `CheckId`: a plan check identical to a floor check
        // is one obligation, and a plan's own checks are added to the floor
        // rather than replacing it, so a trivial declared check cannot escape
        // the root checks the task's changes selected.
        let mut required: Vec<(CheckId, Option<&PlanStep>)> = Vec::new();
        let mut required_ids = BTreeSet::new();
        for step in plan
            .steps
            .iter()
            .filter(|step| !step.id.starts_with(VERIFICATION_STEP_PREFIX))
        {
            let Some(planned) = step.check.as_ref() else {
                continue;
            };
            if !planned.required {
                continue;
            }
            let check = CheckId::new(planned.working_directory.as_str(), planned.command.as_str());
            if required_ids.insert(check.clone()) {
                required.push((check, Some(step)));
            }
        }
        for check in facts.floor_checks {
            // The floor is a task-completion obligation, not a per-change one
            // (review fix R16). While primary plan work is still open or
            // blocked the task is ineligible anyway, so requiring the floor
            // there would only add noise and, worse, would stop a mid-task
            // meeting its own open steps.
            if !open.is_empty() || !blocked.is_empty() {
                continue;
            }
            if required_ids.insert(check.clone()) {
                required.push((check.clone(), None));
            }
        }

        for (check, step) in required {
            // The evidence for a required check is one of two things. A check
            // that ran through the verification loop has a `CheckRun` keyed by
            // identity; a check deferred to its own plan step was run by the
            // worker as an ordinary command, which records a `CommandExit` on
            // the step instead — or, when the user declines that exact command,
            // a `CommandDeclined` (spec 105 review fix R8). A current `CheckRun`
            // is authoritative; for a floor check current command evidence may
            // also satisfy it, including after an older run went stale (review
            // follow-up to R16).
            if let Some(run) = facts.verification.checks.get(&check) {
                if run.is_stale(facts.current_hashes, current_generation) {
                    // A floor check may also be satisfied by the model's own
                    // current command of the identical identity, or waived by
                    // the user's decline of it, even when an older loop run is
                    // stale (review follow-up to R16 + review fix R8).
                    if step.is_none()
                        && let Some((evidence_step, index, evidence)) =
                            latest_matching_evidence_in_plan(plan, &check)
                        && deferred_evidence_is_current(plan, evidence_step, index)
                    {
                        record_command_evidence(&check, evidence, &mut reasons, &mut waivers);
                        continue;
                    }
                    reasons.push(CompletionReason {
                        code: CompletionReasonCode::RequiredCheckStale,
                        explanation: format!(
                            "Required check {} ran, but the files it covered have changed since.",
                            check.label()
                        ),
                    });
                    continue;
                }
                match &run.outcome {
                    CheckOutcome::Skipped(SkipReason::DeclinedByUser) => {
                        waivers.push(CompletionWaiver {
                            check: check.clone(),
                            explanation: format!(
                                "{} was declined by you, so it is waived for this task.",
                                check.label()
                            ),
                        });
                    }
                    CheckOutcome::Skipped(reason) => {
                        reasons.push(CompletionReason {
                            code: CompletionReasonCode::RequiredCheckSkipped,
                            explanation: format!(
                                "Required check {} was skipped ({}), so it does not verify the current state.",
                                check.label(),
                                reason.description()
                            ),
                        });
                    }
                    CheckOutcome::Ran { .. } if passing(run) => {}
                    CheckOutcome::Environment { termination } => {
                        reasons.push(CompletionReason {
                            code: CompletionReasonCode::RequiredCheckFailed,
                            explanation: format!(
                                "Required check {} could not run: {}.",
                                check.label(),
                                termination.description()
                            ),
                        });
                    }
                    CheckOutcome::Ran { exit_code, .. } => {
                        if let Some(acceptance) =
                            accepted_pre_existing_failure(&check, facts.verification)
                        {
                            accepted_failures.push(acceptance);
                        } else {
                            reasons.push(CompletionReason {
                                code: CompletionReasonCode::RequiredCheckFailed,
                                explanation: format!(
                                    "Required check {} failed: {}.",
                                    check.label(),
                                    command_failure_reason(*exit_code)
                                ),
                            });
                        }
                    }
                }
                continue;
            }

            // A floor check that no plan step carries may be satisfied by a
            // command the model ran itself, as well as by a `CheckRun` from the
            // verification loop (review follow-up to R16); a plan-declared
            // check reads only its own step. Both use the same identity matcher
            // and the same R4 currentness rule, never a second mechanism. The
            // latest matching evidence decides: a later approved run supersedes
            // an earlier decline of the same command (review fix R8).
            let evidence = match step {
                Some(step) => latest_matching_evidence_on_step(step, &check)
                    .map(|(index, evidence)| (step, index, evidence)),
                None => latest_matching_evidence_in_plan(plan, &check),
            };
            let Some((evidence_step, evidence_index, evidence)) = evidence else {
                reasons.push(CompletionReason {
                    code: CompletionReasonCode::RequiredCheckMissing,
                    explanation: format!(
                        "Required check {} has no recorded result.",
                        check.label()
                    ),
                });
                continue;
            };
            // Command evidence records no generation, so currentness is read
            // from the durable plan record: the batch boundary and any mutation
            // recorded after this evidence (spec 105 requirement 5).
            if !deferred_evidence_is_current(plan, evidence_step, evidence_index) {
                reasons.push(CompletionReason {
                    code: CompletionReasonCode::RequiredCheckStale,
                    explanation: format!(
                        "Required check {} ran, but the files it covered have changed since.",
                        check.label()
                    ),
                });
                continue;
            }
            record_command_evidence(&check, evidence, &mut reasons, &mut waivers);
        }

        Self {
            task_id: facts.task_id.to_string(),
            eligible: reasons.is_empty(),
            reasons,
            waivers,
            accepted_failures,
        }
    }

    /// The stable codes in report order.
    pub fn reason_codes(&self) -> Vec<CompletionReasonCode> {
        self.reasons.iter().map(|reason| reason.code).collect()
    }
}

/// Whether a check's latest recorded run passed. `Some(0)` and nothing else;
/// `None` means killed or signalled, which is unknown, not zero.
fn passing(run: &CheckRun) -> bool {
    run.outcome.is_passed()
}

/// The report's wording for a failed run's exit: an exit code, or that the
/// command was killed or signalled (which is unknown, never a zero).
/// Whether a required check's failing run is only the failure the task started
/// with (spec 106 requirement 11, context D1).
///
/// All three must hold: the classification is the one written after the
/// check's latest run (an older verdict never vouches for a newer run), it is
/// `pre_existing` or `unchanged_failure` with nothing introduced, and the
/// task-start baseline actually ran and failed. A declined, unavailable or
/// `uncertain` baseline never qualifies: without a real starting point the
/// engine cannot know the task did not cause the failure.
pub(crate) fn accepted_pre_existing_failure(
    check: &CheckId,
    verification: &VerificationState,
) -> Option<CompletionAcceptance> {
    let record = verification.current_provenance(check)?;
    if !matches!(
        record.provenance,
        FailureProvenance::PreExisting | FailureProvenance::UnchangedFailure
    ) || !record.introduced.is_empty()
    {
        return None;
    }
    if !matches!(
        verification.baselines.get(check),
        Some(CheckBaseline::Failed { .. })
    ) {
        return None;
    }
    Some(CompletionAcceptance {
        check: check.clone(),
        provenance: record.provenance,
        explanation: format!(
            "{} still fails, but only as it did before this task started; the task did not cause it and did not change it.",
            check.label()
        ),
    })
}

fn command_failure_reason(exit_code: Option<i32>) -> String {
    match exit_code {
        Some(code) => format!("exit code {code}"),
        None => "killed or signalled".to_string(),
    }
}

/// The command evidence a plan step carries for one check identity.
///
/// A required check deferred to its step runs as an ordinary command, so its
/// evidence is the step's `CommandExit` — or, since review fix R8, the user's
/// explicit `CommandDeclined` of that same command. One identity matcher serves
/// both the plan-declared step and the anywhere-in-plan floor lookup.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CommandEvidence {
    Exit(Option<i32>),
    Declined,
}

/// The command evidence with this check's identity.
///
/// Identity is the full [`CheckId`] — command hash and normalized working
/// directory — exactly as the deferred path has always matched it. The decline
/// and the exit are matched by the same function so they cannot disagree about
/// what "the same check" means (spec 105 review fix R8; no second matcher).
fn matching_command_evidence(evidence: &Evidence, check: &CheckId) -> Option<CommandEvidence> {
    let (command_hash, working_directory) = match evidence {
        Evidence::CommandExit {
            command_hash,
            working_directory,
            ..
        }
        | Evidence::CommandDeclined {
            command_hash,
            working_directory,
            ..
        } => (command_hash.as_ref()?, working_directory),
        _ => return None,
    };
    if command_hash != &sha256(&check.command) || working_directory != &check.working_directory {
        return None;
    }
    match evidence {
        Evidence::CommandExit { exit_code, .. } => Some(CommandEvidence::Exit(*exit_code)),
        Evidence::CommandDeclined { .. } => Some(CommandEvidence::Declined),
        _ => None,
    }
}

/// The latest matching command evidence on one plan step.
fn latest_matching_evidence_on_step(
    step: &PlanStep,
    check: &CheckId,
) -> Option<(usize, CommandEvidence)> {
    step.evidence
        .iter()
        .enumerate()
        .rev()
        .find_map(|(index, evidence)| {
            matching_command_evidence(evidence, check)
                .map(|command_evidence| (index, command_evidence))
        })
}

/// The latest matching command evidence anywhere in the plan, in durable plan
/// order.
///
/// A floor check has no single plan step to read, so a current command exit the
/// model ran itself can satisfy it wherever it appears (review follow-up to
/// R16); the same lookup now also finds the user's decline of that command.
fn latest_matching_evidence_in_plan<'a>(
    plan: &'a TaskPlan,
    check: &CheckId,
) -> Option<(&'a PlanStep, usize, CommandEvidence)> {
    plan.steps.iter().rev().find_map(|step| {
        latest_matching_evidence_on_step(step, check)
            .map(|(index, evidence)| (step, index, evidence))
    })
}

/// Record what the latest matching command evidence means for the gate.
///
/// A passing exit satisfies the check, a non-zero exit fails it, and an explicit
/// decline is the named waiver — never a pass (spec 105 requirement 6).
fn record_command_evidence(
    check: &CheckId,
    evidence: CommandEvidence,
    reasons: &mut Vec<CompletionReason>,
    waivers: &mut Vec<CompletionWaiver>,
) {
    match evidence {
        CommandEvidence::Declined => waivers.push(CompletionWaiver {
            check: check.clone(),
            explanation: format!(
                "{} was declined by you, so it is waived for this task.",
                check.label()
            ),
        }),
        CommandEvidence::Exit(Some(0)) => {}
        CommandEvidence::Exit(exit_code) => reasons.push(CompletionReason {
            code: CompletionReasonCode::RequiredCheckFailed,
            explanation: format!(
                "Required check {} failed: {}.",
                check.label(),
                command_failure_reason(exit_code)
            ),
        }),
    }
}

/// Whether a deferred check's evidence at `evidence_index` is current.
///
/// A deferred required check runs as an ordinary command at its plan step, so
/// its evidence is the step's [`Evidence::CommandExit`] or the explicit
/// [`Evidence::CommandDeclined`] of that command, rather than a [`CheckRun`].
/// Unlike a `CheckRun`, that evidence records no generation, so currentness is
/// read from two durable plan facts:
///
/// 1. [`step_is_current`] — the evidence still belongs to the current mutation
///    batch. A later primary step beginning is the boundary
///    [`TaskPlan::mutation_generation`] counts, so a check from an earlier
///    batch is stale even before the later step mutates anything.
/// 2. no [`Evidence::PatchApplied`] follows the evidence in the plan's own
///    order — not in wall-clock order, which can tie, and not only in a later
///    step: a patch can land after the check and before the same step closes
///    (spec 105 requirement 5).
fn deferred_evidence_is_current(plan: &TaskPlan, step: &PlanStep, evidence_index: usize) -> bool {
    step_is_current(plan, step) && !mutation_after(plan, step, evidence_index)
}

/// Whether a patch was applied after `evidence_index` on `step`.
///
/// The order is the durable plan record: evidence within a step is in the
/// order it was observed, and a later step in the plan comes after every
/// earlier step. A `PatchApplied` is the only mutation the plan records, so it
/// is the mutation-generation signal available to deferred evidence.
fn mutation_after(plan: &TaskPlan, step: &PlanStep, evidence_index: usize) -> bool {
    let is_mutation = |evidence: &Evidence| matches!(evidence, Evidence::PatchApplied { .. });
    let mut after_step = false;
    for candidate in &plan.steps {
        if candidate.id == step.id {
            after_step = true;
            if candidate
                .evidence
                .iter()
                .skip(evidence_index + 1)
                .any(is_mutation)
            {
                return true;
            }
            continue;
        }
        if after_step && candidate.evidence.iter().any(is_mutation) {
            return true;
        }
    }
    false
}

/// Whether a deferred check's step still holds the current mutation batch.
///
/// The evidence is current only while the step is terminal and no later
/// primary step has begun, because a later primary step leaving `Pending` is
/// exactly what advances [`TaskPlan::mutation_generation`]. This is the batch
/// half of currentness; [`mutation_after`] adds the within-batch mutations that
/// the step boundary cannot see.
fn step_is_current(plan: &TaskPlan, step: &PlanStep) -> bool {
    if !step.status.is_terminal() {
        return false;
    }
    let mut seen = false;
    for candidate in plan
        .steps
        .iter()
        .filter(|candidate| !candidate.id.starts_with(VERIFICATION_STEP_PREFIX))
    {
        if candidate.id == step.id {
            seen = true;
            continue;
        }
        if seen && candidate.status != StepStatus::Pending {
            return false;
        }
    }
    seen
}

fn open_steps_explanation(open: &[&PlanStep]) -> String {
    let titles = open
        .iter()
        .map(|step| format!("\"{}\"", step.title))
        .collect::<Vec<_>>()
        .join(", ");
    if open.len() == 1 {
        format!("1 planned step is still open: {titles}.")
    } else {
        format!("{} planned steps are still open: {titles}.", open.len())
    }
}

fn blocked_steps_explanation(blocked: &[&PlanStep]) -> String {
    let titles = blocked
        .iter()
        .map(|step| format!("\"{}\"", step.title))
        .collect::<Vec<_>>()
        .join(", ");
    let noun = if blocked.len() == 1 {
        "planned step is blocked"
    } else {
        "planned steps are blocked"
    };
    format!(
        "{} {noun}: {titles}. A blocked step's command did not succeed, so the work is not finished.",
        blocked.len()
    )
}

/// The completion gate bound to one repository checkout and session log.
///
/// This is the only place the durable facts are assembled. The repository is
/// checked before the session is read, and the task must belong to the session,
/// so a report cannot describe repository A's work while the user is looking at
/// repository B. Check currentness is decided here from the checkout's actual
/// content, never by an adapter.
#[derive(Debug, Clone, Copy)]
pub struct CompletionGateService<'a> {
    session_store: &'a SessionStore,
    repository_root: &'a Path,
}

impl<'a> CompletionGateService<'a> {
    pub fn new(session_store: &'a SessionStore, repository_root: &'a Path) -> Self {
        Self {
            session_store,
            repository_root,
        }
    }

    /// The gate's verdict for one task.
    ///
    /// Reads the active plan, the verification events in the task's
    /// continuation chain, the newest pending decision, and the current content
    /// of every file a check run recorded.
    pub fn report(&self, session_id: &str, task_id: &str) -> Result<CompletionGateReport> {
        let Some(session) = self.session_store.read_session(session_id)? else {
            return Err(ClientError::InvalidInput(format!(
                "Unknown session: {session_id}"
            )));
        };
        if repository_id_for_root(self.repository_root) != session.repository_id {
            return Err(ClientError::AccessDenied(format!(
                "Session {session_id} does not belong to repository {}",
                self.repository_root.display()
            )));
        }
        self.session_store.task(session_id, task_id)?;

        let plan = self.session_store.read_task_plan(session_id, task_id)?;
        let chain = self
            .session_store
            .task_continuation_chain(session_id, task_id)?;
        let ids = chain.iter().map(String::as_str).collect::<Vec<_>>();
        let events = self
            .session_store
            .read_verification_events(session_id, &ids)?;
        let verification = fold_verification(events.iter());
        let pending = self
            .session_store
            .pending_decision_for(session_id, task_id)?;
        let current_hashes = current_hashes(self.repository_root, &verification);
        let floor = self.floor_checks(&session, task_id)?;

        Ok(CompletionGateReport::evaluate(CompletionFacts {
            task_id,
            plan: plan.as_ref(),
            verification: &verification,
            pending: pending.as_ref(),
            current_hashes: &current_hashes,
            floor_checks: &floor,
        }))
    }

    /// The discovered checks of the roots this task's files touched.
    ///
    /// The persisted repository map is read, never rebuilt: a gate report must
    /// not write a map or store command proposals. A repository with no map has
    /// no detected checks to require, so the floor is empty; an unreadable map
    /// is an error rather than a silent empty floor.
    fn floor_checks(&self, session: &Session, task_id: &str) -> Result<Vec<CheckId>> {
        let Some(map) = load_stored_map(self.session_store.data_dir(), &session.repository_id)?
        else {
            return Ok(Vec::new());
        };
        let touched = self
            .session_store
            .task_touched_paths(&session.id, task_id)?;
        Ok(floor_checks(self.repository_root, &map, &touched))
    }

    /// The floor checks a completion verification must run now.
    ///
    /// Review follow-up to R16: a plan-declared required check must not suppress
    /// the completion run for the detected floor. This returns the floor checks
    /// that are missing or stale and are **not** plan-declared; the answer-time
    /// caller runs exactly these through the ordinary approval and command
    /// policy. Plan-declared checks remain the model's responsibility. A floor
    /// check is not missing when the model already ran its exact command in the
    /// exact directory and that exit is current under R4's rule.
    ///
    /// This executes nothing and stores no proposal; it reads the same facts the
    /// gate report reads.
    pub fn floor_checks_needing_verification(
        &self,
        session_id: &str,
        task_id: &str,
    ) -> Result<Vec<CheckId>> {
        let Some(session) = self.session_store.read_session(session_id)? else {
            return Err(ClientError::InvalidInput(format!(
                "Unknown session: {session_id}"
            )));
        };
        if repository_id_for_root(self.repository_root) != session.repository_id {
            return Err(ClientError::AccessDenied(format!(
                "Session {session_id} does not belong to repository {}",
                self.repository_root.display()
            )));
        }
        self.session_store.task(session_id, task_id)?;

        let plan = self.session_store.read_task_plan(session_id, task_id)?;
        let chain = self
            .session_store
            .task_continuation_chain(session_id, task_id)?;
        let ids = chain.iter().map(String::as_str).collect::<Vec<_>>();
        let events = self
            .session_store
            .read_verification_events(session_id, &ids)?;
        let verification = fold_verification(events.iter());
        let current_hashes = current_hashes(self.repository_root, &verification);
        let floor = self.floor_checks(&session, task_id)?;
        let empty_plan;
        let plan = match plan.as_ref() {
            Some(plan) => plan,
            None => {
                empty_plan = TaskPlan::new(task_id, 0);
                &empty_plan
            }
        };
        let generation = plan.mutation_generation();
        // Only a task at completion asks this question. While primary plan work
        // is open or blocked the everyday gate already stops the task, and the
        // completion floor is not yet an obligation (spec 101/105 R15/R16).
        let at_completion = plan.steps.iter().all(|step| {
            step.id.starts_with(VERIFICATION_STEP_PREFIX)
                || (step.status.is_terminal() && step.status != StepStatus::Blocked)
        });
        if !at_completion {
            return Ok(Vec::new());
        }
        let plan_required: BTreeSet<CheckId> = plan
            .steps
            .iter()
            .filter(|step| !step.id.starts_with(VERIFICATION_STEP_PREFIX))
            .filter_map(|step| step.check.as_ref())
            .filter(|check| check.required)
            .map(|check| CheckId::new(check.working_directory.as_str(), check.command.as_str()))
            .collect();

        let mut needed = Vec::new();
        for check in floor {
            if plan_required.contains(&check) {
                continue;
            }
            if let Some(run) = verification.checks.get(&check) {
                if run.is_stale(&current_hashes, generation) {
                    let handled = latest_matching_evidence_in_plan(plan, &check).is_some_and(
                        |(step, index, _)| deferred_evidence_is_current(plan, step, index),
                    );
                    if !handled {
                        needed.push(check);
                    }
                }
                continue;
            }
            // A current command exit or the user's explicit decline of that
            // exact command both dispose of the floor check here; the gate
            // reports a decline as a waiver rather than a pass (review fix R8).
            let handled = latest_matching_evidence_in_plan(plan, &check)
                .is_some_and(|(step, index, _)| deferred_evidence_is_current(plan, step, index));
            if !handled {
                needed.push(check);
            }
        }
        Ok(needed)
    }

    /// Settle a task through the gate, recording the verdict beside its status.
    ///
    /// This is the only engine path from newly settled work to
    /// [`TaskStatus::Complete`]. A durable, non-side-effecting marker is written
    /// *before* the status event so recovery can tell a crash window from an
    /// ordinary interruption; it is finished only after the report and the
    /// status are durable. `Gate` lets the report decide, while
    /// `VerificationFailed` keeps a failed verification failed with the same
    /// report attached.
    pub fn settle(
        &self,
        task: &Task,
        settlement: CompletionSettlement,
    ) -> Result<SettledCompletion> {
        // The answer and plan are already durable here, so a crash during
        // report computation is inside the answer-to-status window. The typed
        // marker is written first so recovery can recognize that window and
        // recompute the same verdict.
        let marker = CompletionSettlementMarker::new(self.repository_root, settlement);
        let marker = self.session_store.start_action(
            task,
            COMPLETION_SETTLEMENT_ACTION,
            &marker.reference()?,
            false,
        )?;
        let report = self.report(&task.session_id, &task.id)?;
        let settled = self.write_settlement(task, settlement, report)?;
        self.session_store
            .finish_action(marker, settlement_outcome(&settled.report))?;
        Ok(settled)
    }

    fn write_settlement(
        &self,
        task: &Task,
        settlement: CompletionSettlement,
        report: CompletionGateReport,
    ) -> Result<SettledCompletion> {
        let status = match settlement {
            CompletionSettlement::Gate if report.eligible => TaskStatus::Complete,
            CompletionSettlement::Gate => TaskStatus::Incomplete,
            CompletionSettlement::VerificationFailed => TaskStatus::Failed,
        };
        let task = self
            .session_store
            .update_task_status_with_completion(task, status, &report)?;
        Ok(SettledCompletion { task, report })
    }
}

/// Finish a settlement whose marker survived a crash.
///
/// The marker is engine-owned data, not adapter input: it carries the validated
/// root and the original settlement kind. Recovery calls this rather than
/// choosing a status itself, so a crashed failed verification stays `failed`
/// and an answered turn is still decided by a freshly recomputed report. The
/// durable marker is the one recovery already holds, so no second marker is
/// created.
pub(crate) fn settle_recovered(
    session_store: &SessionStore,
    task: &Task,
    marker: &crate::session::DanglingAction,
) -> Result<SettledCompletion> {
    let marker_data = CompletionSettlementMarker::parse(&marker.reference)?;
    if !marker_data.root.is_dir() {
        return Err(ClientError::InvalidInput(format!(
            "Completion settlement repository is unavailable: {}",
            marker_data.root.display()
        )));
    }
    let service = CompletionGateService::new(session_store, &marker_data.root);
    let report = service.report(&task.session_id, &task.id)?;
    let settled = service.write_settlement(task, marker_data.settlement, report)?;
    session_store.finish_recorded_action(
        &task.session_id,
        marker,
        settlement_outcome(&settled.report),
    )?;
    Ok(settled)
}

fn settlement_outcome(report: &CompletionGateReport) -> &'static str {
    if report.eligible {
        "eligible"
    } else {
        "ineligible"
    }
}

/// Hash every file a check run recorded, so staleness is decided against the
/// checkout as it is now. A file that no longer exists is simply absent, which
/// the existing staleness rule reads as a mismatch.
fn current_hashes(root: &Path, state: &VerificationState) -> BTreeMap<String, String> {
    let mut paths = BTreeSet::new();
    for run in state.checks.values() {
        paths.extend(run.file_hashes.keys().cloned());
    }
    paths
        .into_iter()
        .filter_map(|path| file_hash(root.join(&path)).ok().map(|hash| (path, hash)))
        .collect()
}
