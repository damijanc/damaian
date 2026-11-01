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
//! prose. Here a check is required because it was selected, and a failure is
//! reported as a failure without assigning provenance.
//!
//! The only waiver is an explicit user decision. A check the user declined is
//! named in [`CompletionWaiver`] and is still never represented as passed. A
//! question answer, a model statement or an absent result is never a waiver.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::clarification::PendingDecision;
use crate::error::{ClientError, Result};
use crate::hash::{file_hash, repository_id_for_root};
use crate::plan::{PlanStep, TaskPlan};
use crate::session::{SessionStore, Task, TaskStatus};
use crate::verification::{
    CheckId, CheckOutcome, CheckRun, SkipReason, VerificationReport, VerificationState,
    fold_verification,
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
    /// A verification is due and has not finished.
    VerificationDue,
    /// A verification run started but recorded no finished event.
    VerificationUnfinished,
    /// A proposal or question is still waiting for the user.
    PendingDecision,
    /// A selected check has no recorded result at all.
    RequiredCheckMissing,
    /// A selected check ran and did not pass.
    RequiredCheckFailed,
    /// A selected check ran, but the files it covered have changed since.
    RequiredCheckStale,
    /// A selected check was skipped for a reason other than an explicit user
    /// decline, so it does not verify the current state.
    RequiredCheckSkipped,
}

impl CompletionReasonCode {
    /// The wire name, matching the serde representation.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::OpenPlanSteps => "open_plan_steps",
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

/// A selected check the user explicitly declined.
///
/// A waiver is a named exception, not a pass: it says the check did not run and
/// that the user chose that. Only [`SkipReason::DeclinedByUser`] produces one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CompletionWaiver {
    pub check: CheckId,
    pub explanation: String,
}

/// The engine's verdict on whether a task is closed (spec 101 §4).
///
/// `eligible` is the whole verdict. `reasons` explains every fact that kept it
/// false, and `waivers` names declined checks the user has already accepted.
/// Both are computed here so a shell, CLI or worker never has to interpret a
/// plan or a verification report itself.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CompletionGateReport {
    pub task_id: String,
    pub eligible: bool,
    pub reasons: Vec<CompletionReason>,
    pub waivers: Vec<CompletionWaiver>,
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
}

impl CompletionGateReport {
    /// Compute the verdict from durable facts.
    pub fn evaluate(facts: CompletionFacts<'_>) -> Self {
        let empty_plan = TaskPlan::new(facts.task_id, 0);
        let plan = facts.plan.unwrap_or(&empty_plan);
        let verification =
            VerificationReport::build(facts.verification, plan, &[], facts.current_hashes);

        let mut reasons = Vec::new();
        let mut waivers = Vec::new();

        let open: Vec<&PlanStep> = plan
            .steps
            .iter()
            .filter(|step| {
                !step.id.starts_with(VERIFICATION_STEP_PREFIX) && !step.status.is_terminal()
            })
            .collect();
        if !open.is_empty() {
            reasons.push(CompletionReason {
                code: CompletionReasonCode::OpenPlanSteps,
                explanation: open_steps_explanation(&open),
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

        for check in &facts.verification.selected {
            let run = facts.verification.checks.get(check);
            if verification.stale.contains(check) {
                reasons.push(CompletionReason {
                    code: CompletionReasonCode::RequiredCheckStale,
                    explanation: format!(
                        "Required check {} ran, but the files it covered have changed since.",
                        check.label()
                    ),
                });
                continue;
            }
            let Some(run) = run else {
                reasons.push(CompletionReason {
                    code: CompletionReasonCode::RequiredCheckMissing,
                    explanation: format!(
                        "Required check {} has no recorded result.",
                        check.label()
                    ),
                });
                continue;
            };
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
                CheckOutcome::Ran { .. } => {
                    let reason = verification
                        .failed
                        .iter()
                        .find(|failed| &failed.check == check)
                        .map(|failed| failed.reason.clone())
                        .unwrap_or_else(|| "did not pass".to_string());
                    reasons.push(CompletionReason {
                        code: CompletionReasonCode::RequiredCheckFailed,
                        explanation: format!(
                            "Required check {} failed: {}.",
                            check.label(),
                            reason
                        ),
                    });
                }
            }
        }

        Self {
            task_id: facts.task_id.to_string(),
            eligible: reasons.is_empty(),
            reasons,
            waivers,
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
    matches!(
        run.outcome,
        CheckOutcome::Ran {
            exit_code: Some(0),
            ..
        }
    )
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

        Ok(CompletionGateReport::evaluate(CompletionFacts {
            task_id,
            plan: plan.as_ref(),
            verification: &verification,
            pending: pending.as_ref(),
            current_hashes: &current_hashes,
        }))
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
