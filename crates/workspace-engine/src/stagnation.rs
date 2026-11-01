//! Repair progress detection and typed stall reasons (spec 102 Task 4).
//!
//! Automatic repair is bounded by the numeric attempt limit, but a count is not
//! progress. This module compares one repair attempt's canonical state with the
//! task-local window that preceded it, and names the reason the engine stopped
//! before spending the remaining attempts.
//!
//! The state is deliberately content-safe: a failed result is represented by
//! Task 1's [`FailureFingerprint`], a proposed patch by a canonical digest of
//! its effective content, and the task's net change by a digest of the changed
//! files' hashes. Raw output, transient ids, timestamps and machine-local paths
//! are not inputs, and no raw content is stored.

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::hash::{file_hash, sha256};
use crate::patch_engine::ProposedFilePatch;
use crate::provenance::{RepairTarget, VerificationAction, VerificationActionKind};
use crate::secret_scanner::SecretScanner;
use crate::verification::{
    CheckId, FailureFingerprint, RepairStateEntry, normalized_repository_path,
};

/// Why automatic repair stopped before the numeric limit.
///
/// The variants are ordered by [`RepairStallReason::PRECEDENCE`]. Several
/// conditions can hold at once — an identical state is also a repeated failure,
/// an equivalent patch and an unchanged diff — so the engine chooses the most
/// specific one through one fixed order rather than the order checks happen to
/// run in. Tests pin both the order and each condition.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RepairStallReason {
    /// All three values are the same as the previous attempt.
    IdenticalState,
    /// The failure fingerprint is the same as the previous attempt.
    RepeatedFailure,
    /// The proposed repair is equivalent to the previous repair.
    EquivalentPatch,
    /// The task's net diff did not change since the previous attempt.
    UnchangedTaskDiff,
    /// A fingerprint/diff pair from earlier in the bounded window repeated
    /// after an intervening state, including A → B → A.
    Oscillation,
}

impl RepairStallReason {
    /// Highest priority first. `IdenticalState` is the strongest statement;
    /// the three single-value repeats follow in the order the repair controller
    /// observes them, and `Oscillation` is the window-level deduction.
    pub const PRECEDENCE: [Self; 5] = [
        Self::IdenticalState,
        Self::RepeatedFailure,
        Self::EquivalentPatch,
        Self::UnchangedTaskDiff,
        Self::Oscillation,
    ];

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::IdenticalState => "identical_state",
            Self::RepeatedFailure => "repeated_failure",
            Self::EquivalentPatch => "equivalent_patch",
            Self::UnchangedTaskDiff => "unchanged_task_diff",
            Self::Oscillation => "oscillation",
        }
    }

    /// The engine's heading for this stop.
    pub fn label(&self) -> &'static str {
        "Repair stalled"
    }

    /// The engine's one-sentence explanation. A renderer prints this verbatim.
    pub fn explanation(&self) -> &'static str {
        match self {
            Self::IdenticalState => {
                "The latest repair attempt produced the same failure, the same patch and no change to the task's files."
            }
            Self::RepeatedFailure => {
                "Two consecutive repair attempts left the same failure, so another automatic attempt would repeat the same work."
            }
            Self::EquivalentPatch => {
                "The latest repair patch is equivalent to the previous repair, so applying it would repeat the same change."
            }
            Self::UnchangedTaskDiff => {
                "Repair attempts have not changed the task's files, so another automatic attempt cannot make progress."
            }
            Self::Oscillation => {
                "The failure and the task's files returned to an earlier state, so repair is oscillating instead of progressing."
            }
        }
    }

    /// The engine-owned user action for a stalled repair. It deliberately
    /// offers no automatic retry and no Spec 96 *Continue repair* action.
    pub fn action(&self) -> VerificationAction {
        VerificationAction {
            kind: VerificationActionKind::ReviewStalledRepair,
            label: "Review the failure and choose the next step".to_string(),
            detail: "Automatic repair stopped before the attempt limit. Fix the failure yourself, change the task, or start a new task; Damaian will not retry automatically.".to_string(),
        }
    }
}

/// A typed repair-stall fact in the durable report.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RepairStall {
    pub reason: RepairStallReason,
    /// The engine's heading. A renderer prints this verbatim.
    pub label: String,
    /// The engine's explanation. A renderer prints this verbatim.
    pub explanation: String,
    pub action: VerificationAction,
}

impl RepairStall {
    pub fn new(reason: RepairStallReason) -> Self {
        Self {
            reason,
            label: reason.label().to_string(),
            explanation: reason.explanation().to_string(),
            action: reason.action(),
        }
    }
}

/// The canonical digest of a proposed repair's effective content.
///
/// Files are sorted by normalised repository path. Only the path, status and
/// content that would be written are inputs: transient patch/marker ids,
/// timestamps, the display diff, hashes derived from the base checkout and the
/// ordering the model emitted do not change an equivalent repair's value. The
/// content is redacted first, so a credential that happens to appear in a
/// proposed file is never hashed as raw secret text.
pub fn proposed_patch_digest(files: &[ProposedFilePatch], scanner: &SecretScanner) -> String {
    let mut rows = files
        .iter()
        .map(|file| {
            (
                normalized_repository_path(&file.path),
                file.status.clone(),
                scanner.redact(&file.new_content).text,
            )
        })
        .collect::<Vec<_>>();
    rows.sort();
    let canonical = serde_json::json!({
        "version": 1,
        "files": rows
            .iter()
            .map(|(path, status, content)| {
                serde_json::json!({
                    "path": path,
                    "status": status,
                    "content": content,
                })
            })
            .collect::<Vec<_>>(),
    });
    sha256(serde_json::to_vec(&canonical).expect("patch digest input is serializable"))
}

/// The canonical digest of the task's net change.
///
/// Paths are normalised and sorted; each existing file contributes its content
/// hash and a deleted or unreadable file contributes a stable marker. Only
/// digests are stored, so the value cannot reveal file content or a
/// machine-local checkout path.
pub fn task_diff_digest(repository_root: &Path, changed: &[String]) -> String {
    let mut rows = changed
        .iter()
        .map(|path| {
            let path = normalized_repository_path(path);
            let hash =
                file_hash(repository_root.join(&path)).unwrap_or_else(|_| "absent".to_string());
            (path, hash)
        })
        .collect::<Vec<_>>();
    rows.sort();
    rows.dedup();
    let canonical = serde_json::json!({
        "version": 1,
        "files": rows
            .iter()
            .map(|(path, hash)| {
                serde_json::json!({
                    "path": path,
                    "hash": hash,
                })
            })
            .collect::<Vec<_>>(),
    });
    sha256(serde_json::to_vec(&canonical).expect("task diff digest input is serializable"))
}

/// The complete comparable state of one repair attempt.
pub fn repair_state_entry(
    repository_root: &Path,
    changed: &[String],
    targets: &[RepairTarget],
    files: &[ProposedFilePatch],
    scanner: &SecretScanner,
) -> RepairStateEntry {
    let failures = targets
        .iter()
        .map(|target| {
            let fingerprint = target.fingerprint.clone().unwrap_or_else(|| {
                FailureFingerprint::compute(&target.check, &target.evidence, &target.termination)
            });
            (target.check.clone(), fingerprint)
        })
        .collect::<Vec<(CheckId, FailureFingerprint)>>();
    RepairStateEntry {
        failure_fingerprint: FailureFingerprint::for_attempt(&failures),
        patch_digest: proposed_patch_digest(files, scanner),
        task_diff_digest: task_diff_digest(repository_root, changed),
    }
}

/// Decide whether adding `candidate` to `history` is progress or a stall.
///
/// `history` is the task-local bounded window, oldest first, and does not yet
/// contain `candidate`. The first attempt has no previous state and therefore
/// never stalls; the numeric limit remains the only bound until a second
/// attempt has something to compare with.
pub fn detect_repair_stall(
    history: &[RepairStateEntry],
    candidate: &RepairStateEntry,
) -> Option<RepairStallReason> {
    let previous = history.last()?;
    let conditions = [
        previous == candidate,
        previous.failure_fingerprint == candidate.failure_fingerprint,
        previous.patch_digest == candidate.patch_digest,
        previous.task_diff_digest == candidate.task_diff_digest,
        history[..history.len() - 1].iter().any(|entry| {
            entry.failure_fingerprint == candidate.failure_fingerprint
                && entry.task_diff_digest == candidate.task_diff_digest
        }),
    ];
    RepairStallReason::PRECEDENCE
        .into_iter()
        .zip(conditions)
        .find_map(|(reason, holds)| holds.then_some(reason))
}
