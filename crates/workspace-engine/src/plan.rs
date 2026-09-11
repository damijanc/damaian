//! The plan a turn works through, and the evidence that says a step is done.
//!
//! See `docs/specs/21_task_plan_progress_and_budget/proposal.md`. This module
//! is pure — no session, no model, no filesystem — because the rules in it
//! *are* requirement 6, and a rule worth stating is worth testing without a
//! turn built around it.
//!
//! A plan belongs to one task, and a task is one turn
//! (`context.md` §3.1), so a step here is a step of the agent loop rather than
//! a stage of a multi-turn project.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StepStatus {
    Pending,
    InProgress,
    Completed,
    Blocked,
    Skipped,
}

/// A file a patch wrote, and the hash of what landed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PatchedFile {
    pub path: String,
    /// What was actually written, which for a partial-hunk accept differs from
    /// the proposal's own `new_hash` — see `patch_engine.rs`, `applied_hash`.
    /// Recording the proposal's hash instead would claim the repository is in
    /// a state it is not.
    pub applied_hash: String,
}

/// Something Damaian observed itself.
///
/// There is deliberately no `ModelAsserted` variant. Requirement 6 exists
/// because the model's claim is precisely what must not count, and a variant
/// carrying one would let a step be marked complete by assertion through a
/// type whose whole purpose is to mean the opposite.
///
/// Every variant carries the **value** it observed, not only a reference to
/// where the value is stored. A `CommandExecution` id reaches the audit log and
/// nowhere else, and the audit log expires on `audit_retention_days`, so a plan
/// read back afterwards would rest on a pointer to nothing. `marker_id` is
/// spec 17's marker, which lives in the session log beside this very event; it
/// is the breadcrumb, and the exit code is the evidence. See `context.md` §3.5
/// before normalising the value away in favour of the reference.
///
/// `#[non_exhaustive]` because `Findings` joins this enum when spec 22 exists
/// to produce the finding ids it would hold (`context.md` §3.6), and that must
/// not be a breaking change for the shell.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
#[non_exhaustive]
pub enum Evidence {
    /// A command ran and exited.
    ///
    /// `exit_code` is `None` when the command was killed or signalled. That is
    /// not a zero and must never be read as one.
    #[serde(rename_all = "camelCase")]
    CommandExit {
        marker_id: String,
        exit_code: Option<i32>,
    },
    /// A patch was applied, with the hash actually written per file.
    #[serde(rename_all = "camelCase")]
    PatchApplied {
        marker_id: String,
        files: Vec<PatchedFile>,
    },
    /// A file was read, with its hash at read time. The weakest evidence here,
    /// and deliberately still evidence: it says the step looked at a known
    /// version of a known file rather than at nothing.
    FileRead { path: String, hash: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlanStep {
    pub id: String,
    pub title: String,
    pub detail: Option<String>,
    pub status: StepStatus,
    /// Recorded so a step whose prerequisite failed can be blocked. Not a
    /// dependency solver: nothing here reorders or optimises a plan (§4).
    pub depends_on: Vec<String>,
    pub started_at_ms: Option<u128>,
    pub completed_at_ms: Option<u128>,
    /// Empty is meaningful, not missing: the step is done as far as the plan is
    /// concerned and nothing observable confirms it. The completion report says
    /// so rather than hiding it — see [`PlanStep::is_unverified`].
    pub evidence: Vec<Evidence>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskPlan {
    pub task_id: String,
    pub created_at_ms: u128,
    pub steps: Vec<PlanStep>,
}

impl TaskPlan {
    pub fn new(task_id: impl Into<String>, created_at_ms: u128) -> Self {
        Self {
            task_id: task_id.into(),
            created_at_ms,
            steps: Vec::new(),
        }
    }

    /// Requirement 2, as a question rather than an invariant enforced in a
    /// setter.
    ///
    /// A setter could only guard the writes it sees; a plan is replayed from an
    /// append-only log, and the violation that matters is one that shows up in
    /// the *replayed* plan after a crash or an out-of-order append. So the
    /// panel and the tests both ask the assembled plan, which is where the
    /// answer is load-bearing.
    pub fn violates_single_in_progress(&self) -> bool {
        self.steps
            .iter()
            .filter(|step| step.status == StepStatus::InProgress)
            .count()
            > 1
    }
}
