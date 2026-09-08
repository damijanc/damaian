//! Classifying what a crash left behind, per
//! `docs/specs/17_durable_task_state_and_crash_recovery/proposal.md` §5.4.
//!
//! Classification is a pure function of a replayed session log: for every task
//! whose latest status is not terminal, a dangling `action_started` with
//! `sideEffecting: true` means the outcome is unknown, and anything else means
//! the task was merely interrupted.
//!
//! The module's reason for existing is requirement 5 — no action whose outcome
//! is unknown is ever automatically repeated. That decision is made *here*
//! rather than by a caller, because spec 45 will present it from a webview and
//! a guarantee enforced only in a webview is not enforced.

use crate::audit::AuditLog;
use crate::error::Result;
use crate::session::{DanglingAction, SessionStore, TaskStatus};

/// A task that was in flight when the process stopped.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecoveredTask {
    pub task_id: String,
    pub session_id: String,
    /// Always [`TaskStatus::Interrupted`] or
    /// [`TaskStatus::UnknownExternalOutcome`] — the classifier's two outputs.
    pub classification: TaskStatus,
    /// The action that started and never finished, when there was one. This is
    /// the evidence: spec 45 renders "a patch application was in progress" from
    /// it rather than re-deriving anything.
    pub dangling: Option<DanglingAction>,
    /// The status the task was left in, before classification.
    pub previous_status: String,
    /// Whether this task may be resumed without asking. Decided here, so a
    /// caller cannot widen it.
    pub auto_resume_permitted: bool,
}

/// Classifies every non-terminal task in one session.
///
/// A torn log tail is audited rather than swallowed: a discarded line means a
/// write was interrupted, which is evidence of the crash being classified.
/// `SessionStore` has no `AuditLog` of its own, so the audit happens here — the
/// caller that actually cares.
pub fn classify_session(
    store: &SessionStore,
    audit: &AuditLog,
    session_id: &str,
) -> Result<Vec<RecoveredTask>> {
    let unreadable = store.unreadable_event_count(session_id)?;
    if unreadable > 0 {
        audit.record(
            "session_log_truncated_tail",
            &[
                ("actor", "system".to_string()),
                ("sessionId", session_id.to_string()),
                ("discardedLines", unreadable.to_string()),
            ],
        )?;
    }

    let statuses = store.read_task_statuses(session_id)?;
    let dangling = store.dangling_actions(session_id)?;
    let mut recovered = Vec::new();

    for (task_id, raw_status) in statuses {
        let status = TaskStatus::parse(&raw_status);
        // An unrecognised status is not assumed benign. It is treated as
        // non-terminal and not auto-resumable, because a status this version
        // does not understand is precisely the case where guessing is unsafe.
        let is_terminal = status.as_ref().is_some_and(TaskStatus::is_terminal);
        if is_terminal {
            continue;
        }
        // §5.4 rule 3: a task waiting for a human was not interrupted
        // mid-action. It keeps its status and its proposal is reattached
        // (§5.5), so it is not a recovered task.
        if status == Some(TaskStatus::WaitingForApproval) {
            continue;
        }

        let task_dangling = dangling
            .iter()
            .find(|action| action.task_id == task_id)
            .cloned();

        // §5.4 rules 1 and 2.
        let classification = match &task_dangling {
            Some(action) if action.side_effecting => TaskStatus::UnknownExternalOutcome,
            _ => TaskStatus::Interrupted,
        };

        // Requirement 6 permits auto-resume only for read-only work. Three
        // conditions, each blocking on its own:
        //
        // 1. The classification must be `Interrupted` — an unknown outcome is
        //    never resumed automatically.
        // 2. The *status* must not itself say a side effect could be in flight.
        //    This is a backstop for a marker that never reached the log at all:
        //    the marker is the primary signal, but a task left in `running_tool`
        //    with no marker is not evidence of safety, it is absence of
        //    evidence.
        // 3. A legacy `running` task is never auto-resumed. §5.6: it carries
        //    exactly the information this work exists to eliminate — something
        //    was in flight and nothing recorded what — so "resumable" cannot be
        //    concluded from it, even though it classifies as `Interrupted`.
        let status_allows = status
            .as_ref()
            .is_some_and(|status| !status.may_have_side_effect_in_flight());
        let auto_resume_permitted =
            classification == TaskStatus::Interrupted && status_allows && raw_status != "running";

        let task = RecoveredTask {
            task_id: task_id.clone(),
            session_id: session_id.to_string(),
            classification: classification.clone(),
            dangling: task_dangling.clone(),
            previous_status: raw_status.clone(),
            auto_resume_permitted,
        };

        // Recorded so a recovery decision is auditable rather than a conclusion
        // something reached once and forgot (§5.4).
        audit.record(
            "task_recovered",
            &[
                ("actor", "system".to_string()),
                ("sessionId", session_id.to_string()),
                ("taskId", task_id.clone()),
                ("previousStatus", raw_status.clone()),
                ("classification", classification.as_str().to_string()),
                (
                    "danglingAction",
                    task_dangling
                        .as_ref()
                        .map(|action| action.action.clone())
                        .unwrap_or_default(),
                ),
                (
                    "danglingRef",
                    task_dangling
                        .as_ref()
                        .map(|action| action.reference.clone())
                        .unwrap_or_default(),
                ),
                (
                    "danglingSeq",
                    task_dangling
                        .as_ref()
                        .map(|action| action.seq.to_string())
                        .unwrap_or_default(),
                ),
                ("autoResumePermitted", auto_resume_permitted.to_string()),
            ],
        )?;

        recovered.push(task);
    }

    recovered.sort_by(|left, right| left.task_id.cmp(&right.task_id));
    Ok(recovered)
}

/// Classifies every non-terminal task across every session in the data
/// directory. This is the launch-time sweep.
pub fn classify_all(store: &SessionStore, audit: &AuditLog) -> Result<Vec<RecoveredTask>> {
    let mut recovered = Vec::new();
    for session in store.list_sessions(None)? {
        recovered.extend(classify_session(store, audit, &session.id)?);
    }
    Ok(recovered)
}
