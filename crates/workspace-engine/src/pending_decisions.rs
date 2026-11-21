//! One pending decision per session
//! (`docs/specs/113_one_pending_decision/proposal.md` requirement 3).
//!
//! A task that pauses on a decision leaves a `WaitingForApproval` status and,
//! for most kinds, a paused turn in `chat/pending/`. Nothing used to settle an
//! older pause when a newer task paused, so a session could pile up decisions
//! the user could no longer reach, and a restart reattached all of them.
//!
//! Settling an older pause takes its paused turn off disk through the same
//! store method the resume routes use, sets the task `Cancelled` and audits
//! `pending_decision_superseded`. Nothing is approved, run or resumed: the
//! decision is simply no longer asked (AGENTS.md, "a paused turn is resumed
//! only by the route for what it is paused for"). A worker session is left
//! alone, because its pauses belong to the worker (spec 57).

use crate::audit::AuditLog;
use crate::clarification::PendingDecision;
use crate::error::Result;
use crate::session::{SessionStore, Task, TaskStatus};

/// One pause settled because a newer decision replaced it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SupersededDecision {
    pub task_id: String,
    pub superseded_by: String,
    /// The approval kind (`command`, `patch`, `plan`, …), `question`, or
    /// `unknown` for a pause written before its link was recorded.
    pub kind: String,
}

/// Settles every task of `session_id` that still waits on a decision, except
/// `newer_task_id`, the task that has just paused (spec 113 D3).
///
/// Called right after the newer pause is recorded, so the session's one
/// pending decision is the one the user is looking at.
pub fn settle_older_pending_decisions(
    store: &SessionStore,
    audit: &AuditLog,
    session_id: &str,
    newer_task_id: &str,
) -> Result<Vec<SupersededDecision>> {
    if store.worker_record(session_id)?.is_some() {
        return Ok(Vec::new());
    }
    let mut settled = Vec::new();
    for task in store.read_tasks(session_id)? {
        if task.status != TaskStatus::WaitingForApproval || task.id == newer_task_id {
            continue;
        }
        settled.push(settle(store, audit, &task, newer_task_id)?);
    }
    Ok(settled)
}

/// Keeps only the newest pending decision of a session that already holds
/// several, as the startup sweep finds them (spec 113 D4). "Newest" is the
/// task created last.
pub fn keep_newest_pending_decision(
    store: &SessionStore,
    audit: &AuditLog,
    session_id: &str,
) -> Result<Vec<SupersededDecision>> {
    if store.worker_record(session_id)?.is_some() {
        return Ok(Vec::new());
    }
    let newest = store
        .read_tasks(session_id)?
        .into_iter()
        .rfind(|task| task.status == TaskStatus::WaitingForApproval);
    match newest {
        Some(newest) => settle_older_pending_decisions(store, audit, session_id, &newest.id),
        None => Ok(Vec::new()),
    }
}

fn settle(
    store: &SessionStore,
    audit: &AuditLog,
    task: &Task,
    newer_task_id: &str,
) -> Result<SupersededDecision> {
    let (kind, paused_turn_id) = match store.pending_decision_for(&task.session_id, &task.id)? {
        Some(PendingDecision::Approval(link)) => (link.kind, Some(link.proposal_id)),
        Some(PendingDecision::Question(question)) => (
            "question".to_string(),
            Some(question.question_id().to_string()),
        ),
        None => ("unknown".to_string(), None),
    };
    // A patch pause saves no paused turn (its proposal stays in the patch
    // store, where it is history), so "nothing taken" is not an error.
    let paused_turn_taken = paused_turn_id
        .as_deref()
        .is_some_and(|id| crate::chat::take_superseded_paused_turn(store.data_dir(), id));
    store.update_task_status(task, TaskStatus::Cancelled, None)?;
    audit.record(
        "pending_decision_superseded",
        &[
            ("actor", "system".to_string()),
            ("sessionId", task.session_id.clone()),
            ("taskId", task.id.clone()),
            ("supersededBy", newer_task_id.to_string()),
            ("kind", kind.clone()),
            ("pausedTurnTaken", paused_turn_taken.to_string()),
        ],
    )?;
    Ok(SupersededDecision {
        task_id: task.id.clone(),
        superseded_by: newer_task_id.to_string(),
        kind,
    })
}
